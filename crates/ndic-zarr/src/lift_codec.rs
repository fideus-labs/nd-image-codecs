//! The `nd_lift` Zarr v3 **array-to-array** codec, registered into the
//! `zarrs` plugin registry.
//!
//! Wraps [`ndic_lift`]'s chunk transforms: on encode the chunk is widened
//! into its coefficient plane (`int32` for input data types of at most 32
//! bits, `int64` for 64-bit input), decorrelated along the configured axes,
//! and handed on; decode narrows back after the inverse transform. The codec
//! composes with stock `zarrs` codecs — the Phase 2 validation series is
//! `transpose → nd_lift → bytes → blosc` — and refuses configurations whose
//! version this build does not implement.

use std::num::NonZeroU64;
use std::sync::Arc;

use zarrs::array::codec::api::{
    ArrayBytes, ArrayCodecTraits, ArrayToArrayCodecTraits, Codec, CodecError, CodecMetadataOptions,
    CodecOptions, CodecPluginV3, CodecTraits, CodecTraitsV3, PartialDecoderCapability,
    PartialEncoderCapability, RecommendedConcurrency,
};
use zarrs::array::data_type::{
    Int8DataType, Int16DataType, Int32DataType, Int64DataType, UInt8DataType, UInt16DataType,
    UInt32DataType, UInt64DataType,
};
use zarrs::array::{DataType, FillValue, data_type};
use zarrs::metadata::Configuration;
use zarrs::metadata::v3::MetadataV3;
use zarrs::plugin::{PluginCreateError, ZarrVersion};

use ndic_lift::NdLiftConfig;

/// The `nd_lift` codec: explicit cross-axis integer lifting, specified by
/// `docs/architecture/nd-transform.md` (never JPEG 2000 Part 2 MCT syntax).
#[derive(Clone, Debug)]
pub struct NdLiftCodec {
    config: NdLiftConfig,
}

zarrs::plugin::impl_extension_aliases!(NdLiftCodec, v3: "nd_lift", []);

// Register into the zarrs Zarr v3 codec plugin registry at link time.
inventory::submit! {
    CodecPluginV3::new::<NdLiftCodec>()
}

impl CodecTraitsV3 for NdLiftCodec {
    fn create(metadata: &MetadataV3) -> Result<Codec, PluginCreateError> {
        let configuration: Configuration = metadata.configuration().cloned().unwrap_or_default();
        let codec = Arc::new(Self::new_with_configuration(&configuration)?);
        Ok(Codec::ArrayToArray(codec))
    }
}

impl NdLiftCodec {
    /// Create the codec from a parsed [`NdLiftConfig`].
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration version is not
    /// implemented by this build or a lifting transform has `levels == 0`.
    pub fn new(config: NdLiftConfig) -> Result<Self, PluginCreateError> {
        config
            .validate_semantics()
            .map_err(|err| PluginCreateError::Other(err.to_string()))?;
        Ok(Self { config })
    }

    /// Create the codec from Zarr v3 `configuration` metadata.
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration does not parse or
    /// is not implemented by this build.
    pub fn new_with_configuration(
        configuration: &Configuration,
    ) -> Result<Self, PluginCreateError> {
        let config: NdLiftConfig = configuration
            .to_typed()
            .map_err(|err| PluginCreateError::Other(format!("nd_lift configuration: {err}")))?;
        Self::new(config)
    }
}

/// The widened coefficient plane a decoded data type transforms in.
enum Plane {
    I32,
    I64,
}

fn plane_of(data_type: &DataType) -> Result<Plane, CodecError> {
    if data_type.is::<UInt8DataType>()
        || data_type.is::<Int8DataType>()
        || data_type.is::<UInt16DataType>()
        || data_type.is::<Int16DataType>()
        || data_type.is::<UInt32DataType>()
        || data_type.is::<Int32DataType>()
    {
        Ok(Plane::I32)
    } else if data_type.is::<UInt64DataType>() || data_type.is::<Int64DataType>() {
        Ok(Plane::I64)
    } else {
        Err(CodecError::UnsupportedDataType(
            data_type.clone(),
            ndic_lift::CODEC_NAME.to_string(),
        ))
    }
}

fn shape_usize(shape: &[NonZeroU64]) -> Result<Vec<usize>, CodecError> {
    shape
        .iter()
        .map(|d| {
            usize::try_from(d.get())
                .map_err(|_| CodecError::Other(format!("chunk extent {d} exceeds usize")))
        })
        .collect()
}

/// Bulk conversion between an array element type and its widened coefficient
/// plane.
///
/// Whole-slice operations so they compile to vectorized loops: the
/// infallible directions are straight casts, and the only value checks that
/// exist — `u32`/`u64` widening into the signed plane, and every narrow on
/// decode — run as min/max reductions *before* a bulk cast, never as a
/// branch per element.
trait PlaneConvert<P>: Sized {
    /// Read native-endian samples from `bytes`, widened into the plane.
    ///
    /// # Errors
    /// [`CodecError`] when a sample does not fit the plane (unsigned input
    /// at or above the plane's sign bit) — the overflow-budget refusal.
    fn widen_bytes(bytes: &[u8]) -> Result<Vec<P>, CodecError>;

    /// Narrow the plane back to samples.
    ///
    /// # Errors
    /// [`CodecError`] when a coefficient is outside the element type's range
    /// (a corrupt or mismatched chunk).
    fn narrow(plane: &[P]) -> Result<Vec<Self>, CodecError>;
}

/// The elements the identity pairs (`i32 → i32`, `i64 → i64`) memcpy.
macro_rules! plane_convert_identity {
    ($t:ty) => {
        impl PlaneConvert<$t> for $t {
            fn widen_bytes(bytes: &[u8]) -> Result<Vec<$t>, CodecError> {
                Ok(bytemuck::pod_collect_to_vec(bytes))
            }
            fn narrow(plane: &[$t]) -> Result<Vec<$t>, CodecError> {
                Ok(plane.to_vec())
            }
        }
    };
}

/// Element types strictly narrower than the plane: widening cannot fail;
/// narrowing range-checks via one min/max scan, then bulk-casts.
macro_rules! plane_convert_narrower {
    ($in:ty => $p:ty) => {
        impl PlaneConvert<$p> for $in {
            fn widen_bytes(bytes: &[u8]) -> Result<Vec<$p>, CodecError> {
                Ok(bytes
                    .chunks_exact(size_of::<$in>())
                    .map(|c| <$p>::from(bytemuck::pod_read_unaligned::<$in>(c)))
                    .collect())
            }
            fn narrow(plane: &[$p]) -> Result<Vec<$in>, CodecError> {
                const LO: $p = <$in>::MIN as $p;
                const HI: $p = <$in>::MAX as $p;
                let lo = plane.iter().copied().min().unwrap_or(LO);
                let hi = plane.iter().copied().max().unwrap_or(HI);
                if lo < LO || hi > HI {
                    return Err(narrow_error(lo.into(), hi.into(), stringify!($in)));
                }
                // Range-checked above, so the truncating cast is exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let out = plane.iter().map(|&v| v as $in).collect();
                Ok(out)
            }
        }
    };
}

/// Unsigned element types as wide as the plane (`u32 → i32`, `u64 → i64`):
/// both directions range-check via one min/max scan, then bulk-cast.
macro_rules! plane_convert_unsigned_full_width {
    ($in:ty => $p:ty) => {
        impl PlaneConvert<$p> for $in {
            fn widen_bytes(bytes: &[u8]) -> Result<Vec<$p>, CodecError> {
                #[allow(clippy::cast_sign_loss)]
                const PLANE_MAX: $in = <$p>::MAX as $in;
                let decode = |c| bytemuck::pod_read_unaligned::<$in>(c);
                let max = bytes
                    .chunks_exact(size_of::<$in>())
                    .map(decode)
                    .max()
                    .unwrap_or(0);
                if max > PLANE_MAX {
                    return Err(CodecError::Other(format!(
                        "nd_lift overflow budget: input value {max} does not fit the widened \
                         {} coefficient plane",
                        stringify!($p),
                    )));
                }
                // Range-checked above, so the wrapping cast is exact.
                #[allow(clippy::cast_possible_wrap)]
                let out = bytes
                    .chunks_exact(size_of::<$in>())
                    .map(|c| decode(c) as $p)
                    .collect();
                Ok(out)
            }
            fn narrow(plane: &[$p]) -> Result<Vec<$in>, CodecError> {
                let lo = plane.iter().copied().min().unwrap_or(0);
                if lo < 0 {
                    let hi = plane.iter().copied().max().unwrap_or(0);
                    return Err(narrow_error(lo.into(), hi.into(), stringify!($in)));
                }
                // Non-negative per the check above, so the cast is exact.
                #[allow(clippy::cast_sign_loss)]
                let out = plane.iter().map(|&v| v as $in).collect();
                Ok(out)
            }
        }
    };
}

plane_convert_identity!(i32);
plane_convert_identity!(i64);
plane_convert_narrower!(u8 => i32);
plane_convert_narrower!(i8 => i32);
plane_convert_narrower!(u16 => i32);
plane_convert_narrower!(i16 => i32);
plane_convert_unsigned_full_width!(u32 => i32);
plane_convert_unsigned_full_width!(u64 => i64);

fn narrow_error(lo: i128, hi: i128, dtype: &str) -> CodecError {
    CodecError::Other(format!(
        "nd_lift decode: coefficient range [{lo}, {hi}] does not narrow back to {dtype} \
         (corrupt or mismatched chunk)"
    ))
}

/// Reinterpret native-endian chunk bytes as `In` elements, widen to the
/// plane type `P`, transform, and emit the plane's bytes (or the reverse).
fn transform_bytes<In, P>(
    bytes: &[u8],
    shape: &[usize],
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>, CodecError>
where
    In: bytemuck::Pod + PlaneConvert<P>,
    P: ndic_lift::PlaneSample + bytemuck::Pod,
{
    let n: usize = shape.iter().product();
    if forward {
        if bytes.len() != n * size_of::<In>() {
            return Err(CodecError::Other(format!(
                "nd_lift encode: got {} bytes for {n} elements of {} bytes",
                bytes.len(),
                size_of::<In>()
            )));
        }
        let mut plane = In::widen_bytes(bytes)?;
        ndic_lift::forward(&mut plane, shape, &config.transforms)
            .map_err(|err| CodecError::Other(err.to_string()))?;
        Ok(bytemuck::cast_slice(&plane).to_vec())
    } else {
        if bytes.len() != n * size_of::<P>() {
            return Err(CodecError::Other(format!(
                "nd_lift decode: got {} bytes for {n} coefficients of {} bytes",
                bytes.len(),
                size_of::<P>()
            )));
        }
        let mut plane: Vec<P> = bytemuck::pod_collect_to_vec(bytes);
        ndic_lift::inverse(&mut plane, shape, &config.transforms)
            .map_err(|err| CodecError::Other(err.to_string()))?;
        let output = In::narrow(&plane)?;
        Ok(bytemuck::cast_slice(&output).to_vec())
    }
}

/// Run [`transform_bytes`] with the element/plane pair for `data_type`.
fn transform_dispatch(
    bytes: &[u8],
    shape: &[usize],
    data_type: &DataType,
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>, CodecError> {
    if data_type.is::<UInt8DataType>() {
        transform_bytes::<u8, i32>(bytes, shape, config, forward)
    } else if data_type.is::<Int8DataType>() {
        transform_bytes::<i8, i32>(bytes, shape, config, forward)
    } else if data_type.is::<UInt16DataType>() {
        transform_bytes::<u16, i32>(bytes, shape, config, forward)
    } else if data_type.is::<Int16DataType>() {
        transform_bytes::<i16, i32>(bytes, shape, config, forward)
    } else if data_type.is::<UInt32DataType>() {
        transform_bytes::<u32, i32>(bytes, shape, config, forward)
    } else if data_type.is::<Int32DataType>() {
        transform_bytes::<i32, i32>(bytes, shape, config, forward)
    } else if data_type.is::<UInt64DataType>() {
        transform_bytes::<u64, i64>(bytes, shape, config, forward)
    } else if data_type.is::<Int64DataType>() {
        transform_bytes::<i64, i64>(bytes, shape, config, forward)
    } else {
        Err(CodecError::UnsupportedDataType(
            data_type.clone(),
            ndic_lift::CODEC_NAME.to_string(),
        ))
    }
}

impl CodecTraits for NdLiftCodec {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn configuration(
        &self,
        _version: ZarrVersion,
        _options: &CodecMetadataOptions,
    ) -> Option<Configuration> {
        match serde_json::to_value(&self.config) {
            Ok(serde_json::Value::Object(map)) => Some(Configuration::from(map)),
            _ => None,
        }
    }

    fn partial_decoder_capability(&self) -> PartialDecoderCapability {
        // Lifting couples samples along the transformed axes: the whole chunk
        // must be decoded (grouping bounds the coupling, not the codec I/O).
        PartialDecoderCapability {
            partial_read: false,
            partial_decode: false,
        }
    }

    fn partial_encoder_capability(&self) -> PartialEncoderCapability {
        PartialEncoderCapability {
            partial_encode: false,
        }
    }
}

impl ArrayCodecTraits for NdLiftCodec {
    fn recommended_concurrency(
        &self,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
    ) -> Result<RecommendedConcurrency, CodecError> {
        Ok(RecommendedConcurrency::new_maximum(1))
    }
}

impl ArrayToArrayCodecTraits for NdLiftCodec {
    fn into_dyn(self: Arc<Self>) -> Arc<dyn ArrayToArrayCodecTraits> {
        self as Arc<dyn ArrayToArrayCodecTraits>
    }

    fn encoded_data_type(&self, decoded_data_type: &DataType) -> Result<DataType, CodecError> {
        Ok(match plane_of(decoded_data_type)? {
            Plane::I32 => data_type::int32(),
            Plane::I64 => data_type::int64(),
        })
    }

    fn encoded_fill_value(
        &self,
        decoded_data_type: &DataType,
        decoded_fill_value: &FillValue,
    ) -> Result<FillValue, CodecError> {
        // A transform of a single element is the identity, so the fill value
        // only widens to the coefficient plane.
        let bytes = transform_dispatch(
            decoded_fill_value.as_ne_bytes(),
            &[1],
            decoded_data_type,
            &NdLiftConfig::new(Vec::new()),
            true,
        )?;
        Ok(FillValue::new(bytes))
    }

    fn encode<'a>(
        &self,
        bytes: ArrayBytes<'a>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'a>, CodecError> {
        let shape = shape_usize(shape)?;
        self.config
            .validate(shape.len())
            .map_err(|err| CodecError::Other(err.to_string()))?;
        let fixed = bytes.into_fixed()?;
        let out = transform_dispatch(&fixed, &shape, data_type, &self.config, true)?;
        Ok(ArrayBytes::from(out))
    }

    fn decode<'a>(
        &self,
        bytes: ArrayBytes<'a>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'a>, CodecError> {
        let shape = shape_usize(shape)?;
        self.config
            .validate(shape.len())
            .map_err(|err| CodecError::Other(err.to_string()))?;
        let fixed = bytes.into_fixed()?;
        let out = transform_dispatch(&fixed, &shape, data_type, &self.config, false)?;
        Ok(ArrayBytes::from(out))
    }
}
