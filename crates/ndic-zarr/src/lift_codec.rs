//! The `nd_lift` Zarr v3 **array-to-array** codec, registered into the
//! `zarrs` plugin registry.
//!
//! Wraps [`ndic_lift`]'s chunk transforms: on encode the chunk is widened
//! into its coefficient plane (`int32` for input data types of at most 32
//! bits, `int64` for 64-bit input), decorrelated along the configured axes,
//! and handed on; decode narrows back after the inverse transform. The codec
//! composes with stock `zarrs` codecs — the validation series is
//! `transpose → nd_lift → bytes → blosc` — and refuses configurations whose
//! version this build does not implement.

use std::num::NonZeroU64;
use std::sync::Arc;

use zarrs::array::codec::api::{
    ArrayBytes, ArrayCodecTraits, ArrayPartialDecoderTraits, ArrayToArrayCodecTraits, Codec,
    CodecError, CodecMetadataOptions, CodecOptions, CodecPluginV3, CodecTraits, CodecTraitsV3,
    PartialDecoderCapability, PartialEncoderCapability, RecommendedConcurrency,
};
use zarrs::array::data_type::{
    Int8DataType, Int16DataType, Int32DataType, Int64DataType, UInt8DataType, UInt16DataType,
    UInt32DataType, UInt64DataType,
};
use zarrs::array::{ArraySubset, DataType, FillValue, Indexer, data_type};
use zarrs::metadata::Configuration;
use zarrs::metadata::v3::MetadataV3;
use zarrs::plugin::{PluginCreateError, ZarrVersion};

use ndic_lift::NdLiftConfig;

use crate::lift::LiftDtype;

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

/// Map a `zarrs` data type onto the chunk core's element type.
fn lift_dtype(data_type: &DataType) -> Result<LiftDtype, CodecError> {
    if data_type.is::<UInt8DataType>() {
        Ok(LiftDtype::U8)
    } else if data_type.is::<Int8DataType>() {
        Ok(LiftDtype::I8)
    } else if data_type.is::<UInt16DataType>() {
        Ok(LiftDtype::U16)
    } else if data_type.is::<Int16DataType>() {
        Ok(LiftDtype::I16)
    } else if data_type.is::<UInt32DataType>() {
        Ok(LiftDtype::U32)
    } else if data_type.is::<Int32DataType>() {
        Ok(LiftDtype::I32)
    } else if data_type.is::<UInt64DataType>() {
        Ok(LiftDtype::U64)
    } else if data_type.is::<Int64DataType>() {
        Ok(LiftDtype::I64)
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

/// Run the chunk core's transform with the element type for `data_type`.
///
/// The core works on little-endian bytes while `zarrs` hands over
/// native-endian element bytes; every supported target is little-endian (the
/// same assumption `htj2k_codec` and `zfp_codec` make), so the two agree.
fn transform_dispatch(
    bytes: &[u8],
    shape: &[usize],
    data_type: &DataType,
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>, CodecError> {
    let dtype = lift_dtype(data_type)?;
    let run = if forward {
        crate::lift::forward_chunk
    } else {
        crate::lift::inverse_chunk
    };
    run(bytes, shape, dtype, config).map_err(|err| CodecError::Other(err.to_string()))
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
        // Both true because [`NdLiftPartialDecoder`] decodes the whole chunk
        // once, up front, and then answers every indexer out of that buffer:
        // it needs no cache above it (it *is* one) and none below it (it
        // reads its input exactly once). Lifting couples samples along the
        // transformed axes, so a genuinely partial decode is impossible —
        // grouping bounds the coupling, not the codec I/O.
        PartialDecoderCapability {
            partial_read: true,
            partial_decode: true,
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

/// Serves chunk subsets out of one full-chunk decode.
///
/// The codec cannot decode a subset — every lifting kind couples samples
/// along its axis — so a partial read has to invert the whole chunk and slice
/// the result. Owning that here rather than leaving it to the codec chain's
/// generic cache is what makes `transpose → nd_lift` work: the chain sizes an
/// inserted cache from the *decoded* representation of the codec it precedes
/// while the handle it wraps produces the *encoded* one, which for a codec
/// under `transpose` are different shapes, and the read fails with
/// `IncompatibleIndexer`. No stock array-to-array codec reports
/// `partial_decode: false`, so nothing upstream exercises that path.
struct NdLiftPartialDecoder {
    /// The decoded chunk shape (`nd_lift` does not reshape).
    shape: Vec<u64>,
    data_type: DataType,
    chunk: ArrayBytes<'static>,
}

impl NdLiftPartialDecoder {
    fn new(
        codec: &NdLiftCodec,
        input_handle: &dyn ArrayPartialDecoderTraits,
        shape: &[NonZeroU64],
        data_type: &DataType,
        fill_value: &FillValue,
        options: &CodecOptions,
    ) -> Result<Self, CodecError> {
        let shape_u64: Vec<u64> = shape.iter().map(|d| d.get()).collect();
        let coefficients = input_handle
            .partial_decode(&ArraySubset::new_with_shape(shape_u64.clone()), options)?;
        let chunk = codec
            .decode(coefficients, shape, data_type, fill_value, options)?
            .into_owned();
        Ok(Self {
            shape: shape_u64,
            data_type: data_type.clone(),
            chunk,
        })
    }
}

impl ArrayPartialDecoderTraits for NdLiftPartialDecoder {
    fn data_type(&self) -> &DataType {
        &self.data_type
    }

    fn exists(&self) -> Result<bool, zarrs::storage::StorageError> {
        Ok(true)
    }

    fn size_held(&self) -> usize {
        self.chunk.size()
    }

    fn partial_decode(
        &self,
        indexer: &dyn Indexer,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'_>, CodecError> {
        self.chunk
            .extract_array_subset(indexer, &self.shape, &self.data_type)
    }

    fn supports_partial_decode(&self) -> bool {
        true
    }
}

impl ArrayToArrayCodecTraits for NdLiftCodec {
    fn into_dyn(self: Arc<Self>) -> Arc<dyn ArrayToArrayCodecTraits> {
        self as Arc<dyn ArrayToArrayCodecTraits>
    }

    fn partial_decoder(
        self: Arc<Self>,
        input_handle: Arc<dyn ArrayPartialDecoderTraits>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        fill_value: &FillValue,
        options: &CodecOptions,
    ) -> Result<Arc<dyn ArrayPartialDecoderTraits>, CodecError> {
        Ok(Arc::new(NdLiftPartialDecoder::new(
            &self,
            &*input_handle,
            shape,
            data_type,
            fill_value,
            options,
        )?))
    }

    fn encoded_data_type(&self, decoded_data_type: &DataType) -> Result<DataType, CodecError> {
        Ok(match lift_dtype(decoded_data_type)?.plane_size_bytes() {
            8 => data_type::int64(),
            _ => data_type::int32(),
        })
    }

    fn encoded_fill_value(
        &self,
        decoded_data_type: &DataType,
        decoded_fill_value: &FillValue,
    ) -> Result<FillValue, CodecError> {
        // A transform of a single element is the identity, so the fill value
        // only widens to the coefficient plane.
        //
        // Not `forward` of a *filled chunk*: for a non-zero fill no scalar
        // could be, since a constant chunk lifts to something non-uniform
        // (`[v, 0, ...]` under delta). What the encoded fill value is asked
        // for is symmetry — a stored region equal to it is elided on write
        // and restored from it on read — which holds for any value. Absent
        // chunks are materialized in the decoded domain and never routed
        // through this codec. `non_zero_fill_value_round_trips` pins it.
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
        let fixed = bytes.into_fixed()?;
        let out = transform_dispatch(&fixed, &shape, data_type, &self.config, false)?;
        Ok(ArrayBytes::from(out))
    }
}
