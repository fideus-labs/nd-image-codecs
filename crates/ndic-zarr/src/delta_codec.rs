//! The `numcodecs.delta` Zarr v3 **array-to-array** codec, registered into
//! the `zarrs` plugin registry.
//!
//! `zarrs` ships no delta codec, but the nd-delta family's pipeline is
//! `transpose → numcodecs.delta → bytes → blosc`, so the Rust ecosystem
//! needs one to read and write nd-delta stores at all. This implements
//! exactly the semantics of numcodecs' `Delta` as zarr-python v3 registers
//! it (`numcodecs.zarr3.Delta`): the chunk bytes are viewed as flattened
//! `dtype` elements, `enc[0] = arr[0]`, `enc[i] = arr[i] - arr[i-1]` with
//! NumPy-style modular (wrapping) integer arithmetic; decode is the running
//! sum. `astype` is accepted only when it equals `dtype` — the codec-series
//! builder never emits it, and a widening `astype` would change the encoded
//! data type.

use std::num::NonZeroU64;
use std::sync::Arc;

use zarrs::array::codec::api::{
    ArrayBytes, ArrayCodecTraits, ArrayPartialDecoderTraits, ArrayToArrayCodecTraits, Codec,
    CodecError, CodecMetadataOptions, CodecOptions, CodecPluginV3, CodecTraits, CodecTraitsV3,
    PartialDecoderCapability, PartialEncoderCapability, RecommendedConcurrency,
};
use zarrs::array::{ArraySubset, DataType, FillValue, Indexer};
use zarrs::metadata::Configuration;
use zarrs::metadata::v3::MetadataV3;
use zarrs::plugin::{PluginCreateError, ZarrVersion};

/// The registered Zarr v3 codec identifier (zarr-python's numcodecs wrapper
/// naming).
pub const DELTA_CODEC_NAME: &str = "numcodecs.delta";

/// The `numcodecs.delta` codec `configuration` object.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaConfig {
    /// NumPy-style dtype descr the chunk bytes are viewed as, e.g. `"<u2"`.
    pub dtype: String,
    /// Output dtype; only `astype == dtype` is supported (the default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astype: Option<String>,
}

/// `numcodecs.delta`: first-order differencing of the flattened chunk.
#[derive(Clone, Debug)]
pub struct DeltaCodec {
    config: DeltaConfig,
}

zarrs::plugin::impl_extension_aliases!(DeltaCodec, v3: "numcodecs.delta", []);

// Register into the zarrs Zarr v3 codec plugin registry at link time.
inventory::submit! {
    CodecPluginV3::new::<DeltaCodec>()
}

impl CodecTraitsV3 for DeltaCodec {
    fn create(metadata: &MetadataV3) -> Result<Codec, PluginCreateError> {
        let configuration: Configuration = metadata.configuration().cloned().unwrap_or_default();
        let codec = Arc::new(Self::new_with_configuration(&configuration)?);
        Ok(Codec::ArrayToArray(codec))
    }
}

impl DeltaCodec {
    /// Create the codec from a parsed [`DeltaConfig`].
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the dtype descr is not supported or
    /// `astype` differs from `dtype`.
    pub fn new(config: DeltaConfig) -> Result<Self, PluginCreateError> {
        if delta_elem(&config.dtype).is_none() {
            return Err(PluginCreateError::Other(format!(
                "numcodecs.delta: unsupported dtype {:?}",
                config.dtype
            )));
        }
        if let Some(astype) = &config.astype
            && *astype != config.dtype
        {
            return Err(PluginCreateError::Other(format!(
                "numcodecs.delta: astype {:?} != dtype {:?} is not supported (it would \
                 change the encoded data type)",
                astype, config.dtype
            )));
        }
        Ok(Self { config })
    }

    /// Create the codec from Zarr v3 `configuration` metadata.
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration does not parse or
    /// is not supported.
    pub fn new_with_configuration(
        configuration: &Configuration,
    ) -> Result<Self, PluginCreateError> {
        let config: DeltaConfig = configuration.to_typed().map_err(|err| {
            PluginCreateError::Other(format!("numcodecs.delta configuration: {err}"))
        })?;
        Self::new(config)
    }
}

/// One supported element type: differencing and running-sum over
/// little-endian chunk bytes, in place.
struct DeltaElem {
    size: usize,
    diff: fn(&mut [u8]),
    cumsum: fn(&mut [u8]),
}

/// Wrapping (modular) arithmetic for the integer types — NumPy-style
/// `subtract` on integer arrays wraps, and the running sum on decode must undo it for
/// any stored chunk without panicking under `overflow-checks`.
macro_rules! delta_int {
    ($t:ty) => {
        DeltaElem {
            size: size_of::<$t>(),
            diff: |bytes| {
                let mut prev: $t = 0;
                let mut first = true;
                for chunk in bytes.chunks_exact_mut(size_of::<$t>()) {
                    let v = <$t>::from_le_bytes(chunk.try_into().unwrap());
                    let enc = if first { v } else { v.wrapping_sub(prev) };
                    first = false;
                    prev = v;
                    chunk.copy_from_slice(&enc.to_le_bytes());
                }
            },
            cumsum: |bytes| {
                let mut acc: $t = 0;
                for chunk in bytes.chunks_exact_mut(size_of::<$t>()) {
                    let v = <$t>::from_le_bytes(chunk.try_into().unwrap());
                    acc = acc.wrapping_add(v);
                    chunk.copy_from_slice(&acc.to_le_bytes());
                }
            },
        }
    };
}

/// Plain IEEE arithmetic for the float types, matching NumPy-style sequential
/// `subtract`/`cumsum` exactly (both are element-order left folds).
macro_rules! delta_float {
    ($t:ty) => {
        DeltaElem {
            size: size_of::<$t>(),
            diff: |bytes| {
                let mut prev: $t = 0.0;
                let mut first = true;
                for chunk in bytes.chunks_exact_mut(size_of::<$t>()) {
                    let v = <$t>::from_le_bytes(chunk.try_into().unwrap());
                    let enc = if first { v } else { v - prev };
                    first = false;
                    prev = v;
                    chunk.copy_from_slice(&enc.to_le_bytes());
                }
            },
            cumsum: |bytes| {
                let mut acc: $t = 0.0;
                for chunk in bytes.chunks_exact_mut(size_of::<$t>()) {
                    let v = <$t>::from_le_bytes(chunk.try_into().unwrap());
                    acc += v;
                    chunk.copy_from_slice(&acc.to_le_bytes());
                }
            },
        }
    };
}

/// The element ops for a NumPy-style dtype descr (or bare Zarr name, which the
/// builder never emits but costs nothing to accept).
fn delta_elem(dtype: &str) -> Option<DeltaElem> {
    Some(match dtype {
        "|u1" | "u1" | "uint8" => delta_int!(u8),
        "|i1" | "i1" | "int8" => delta_int!(i8),
        "<u2" | "u2" | "uint16" => delta_int!(u16),
        "<i2" | "i2" | "int16" => delta_int!(i16),
        "<u4" | "u4" | "uint32" => delta_int!(u32),
        "<i4" | "i4" | "int32" => delta_int!(i32),
        "<u8" | "u8" | "uint64" => delta_int!(u64),
        "<i8" | "i8" | "int64" => delta_int!(i64),
        "<f4" | "f4" | "float32" => delta_float!(f32),
        "<f8" | "f8" | "float64" => delta_float!(f64),
        _ => return None,
    })
}

impl DeltaCodec {
    fn transform(&self, bytes: &[u8], forward: bool) -> Result<Vec<u8>, CodecError> {
        let elem = delta_elem(&self.config.dtype).ok_or_else(|| {
            CodecError::Other(format!(
                "numcodecs.delta: unsupported dtype {:?}",
                self.config.dtype
            ))
        })?;
        if !bytes.len().is_multiple_of(elem.size) {
            return Err(CodecError::Other(format!(
                "numcodecs.delta: {} bytes is not a whole number of {:?} elements",
                bytes.len(),
                self.config.dtype
            )));
        }
        let mut out = bytes.to_vec();
        if forward {
            (elem.diff)(&mut out);
        } else {
            (elem.cumsum)(&mut out);
        }
        Ok(out)
    }
}

impl CodecTraits for DeltaCodec {
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
        // Both true for the same reason as `NdLiftCodec`: the partial decoder
        // below decodes the whole chunk once, up front (the running sum
        // couples every element to the ones before it), and then answers
        // every indexer out of that buffer.
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

impl ArrayCodecTraits for DeltaCodec {
    fn recommended_concurrency(
        &self,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
    ) -> Result<RecommendedConcurrency, CodecError> {
        Ok(RecommendedConcurrency::new_maximum(1))
    }
}

/// Serves chunk subsets out of one full-chunk decode, exactly like
/// `NdLiftPartialDecoder` (see that type for why the codec owns this rather
/// than relying on the chain's generic cache under `transpose`).
struct DeltaPartialDecoder {
    shape: Vec<u64>,
    data_type: DataType,
    chunk: ArrayBytes<'static>,
}

impl DeltaPartialDecoder {
    fn new(
        codec: &DeltaCodec,
        input_handle: &dyn ArrayPartialDecoderTraits,
        shape: &[NonZeroU64],
        data_type: &DataType,
        fill_value: &FillValue,
        options: &CodecOptions,
    ) -> Result<Self, CodecError> {
        let shape_u64: Vec<u64> = shape.iter().map(|d| d.get()).collect();
        let encoded = input_handle
            .partial_decode(&ArraySubset::new_with_shape(shape_u64.clone()), options)?;
        let chunk = codec
            .decode(encoded, shape, data_type, fill_value, options)?
            .into_owned();
        Ok(Self {
            shape: shape_u64,
            data_type: data_type.clone(),
            chunk,
        })
    }
}

impl ArrayPartialDecoderTraits for DeltaPartialDecoder {
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

impl ArrayToArrayCodecTraits for DeltaCodec {
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
        Ok(Arc::new(DeltaPartialDecoder::new(
            &self,
            &*input_handle,
            shape,
            data_type,
            fill_value,
            options,
        )?))
    }

    fn encoded_data_type(&self, decoded_data_type: &DataType) -> Result<DataType, CodecError> {
        // `astype == dtype`, so differencing preserves the data type.
        Ok(decoded_data_type.clone())
    }

    fn encoded_fill_value(
        &self,
        _decoded_data_type: &DataType,
        decoded_fill_value: &FillValue,
    ) -> Result<FillValue, CodecError> {
        // A delta of a single element is the identity (`enc[0] = arr[0]`).
        Ok(decoded_fill_value.clone())
    }

    fn encode<'a>(
        &self,
        bytes: ArrayBytes<'a>,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'a>, CodecError> {
        let fixed = bytes.into_fixed()?;
        Ok(ArrayBytes::from(self.transform(&fixed, true)?))
    }

    fn decode<'a>(
        &self,
        bytes: ArrayBytes<'a>,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'a>, CodecError> {
        let fixed = bytes.into_fixed()?;
        Ok(ArrayBytes::from(self.transform(&fixed, false)?))
    }
}
