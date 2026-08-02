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

/// Reinterpret little-endian-ordered native chunk bytes as `In` elements,
/// widen to the plane type `P`, transform, and emit the plane's bytes.
fn transform_bytes<In, P>(
    bytes: &[u8],
    shape: &[usize],
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>, CodecError>
where
    In: bytemuck::Pod + TryFrom<P> + std::fmt::Display + Copy,
    P: ndic_lift::PlaneSample + bytemuck::Pod + TryFrom<In>,
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
        let input: Vec<In> = bytemuck::pod_collect_to_vec(bytes);
        let mut plane: Vec<P> = Vec::with_capacity(n);
        for v in input {
            plane.push(P::try_from(v).map_err(|_| {
                CodecError::Other(format!(
                    "nd_lift overflow budget: input value {v} does not fit the widened \
                     coefficient plane"
                ))
            })?);
        }
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
        let mut output: Vec<In> = Vec::with_capacity(n);
        for v in plane {
            output.push(In::try_from(v).map_err(|_| {
                CodecError::Other(
                    "nd_lift decode: coefficient does not narrow back to the array data type \
                     (corrupt or mismatched chunk)"
                        .to_string(),
                )
            })?);
        }
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
