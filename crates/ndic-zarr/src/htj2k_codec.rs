//! The `htj2k` Zarr v3 **array-to-bytes** codec, registered into the
//! `zarrs` plugin registry.
//!
//! Wraps the feature-free chunk core in [`crate::htj2k`]: each trailing 2D
//! plane of the chunk becomes an independent RPCL Part 1/15 codestream, and
//! the chunk bytes are `[header | coefficient-plane index | codestreams…]`.
//! The partial decoder exploits that layout: an
//! [`ArraySubset`](zarrs::array::ArraySubset) read touching a few planes
//! fetches the index plus exactly those planes' byte ranges from the store
//! — the same access pattern `RangeIndex` plans for HTTP clients.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use zarrs::array::codec::api::{
    ArrayBytes, ArrayBytesRaw, ArrayCodecTraits, ArrayPartialDecoderTraits,
    ArrayToBytesCodecTraits, BytesPartialDecoderTraits, BytesRepresentation, Codec, CodecError,
    CodecMetadataOptions, CodecOptions, CodecPluginV3, CodecTraits, CodecTraitsV3,
    PartialDecoderCapability, PartialEncoderCapability, RecommendedConcurrency,
};
use zarrs::array::data_type::{
    Int8DataType, Int16DataType, Int32DataType, UInt8DataType, UInt16DataType, UInt32DataType,
};
use zarrs::array::{DataType, FillValue, Indexer};
use zarrs::metadata::Configuration;
use zarrs::metadata::v3::MetadataV3;
use zarrs::plugin::{PluginCreateError, ZarrVersion};

use ndic_core::SampleType;
use ndic_codestream::container::ChunkHeader;

use crate::htj2k::{Htj2kConfig, decode_chunk, decode_plane, encode_chunk};

/// The `htj2k` codec: independent HTJ2K codestreams per trailing 2D plane
/// plus the coefficient-plane byte index
/// (`docs/architecture/range-access.md`).
#[derive(Clone, Debug)]
pub struct Htj2kCodec {
    config: Htj2kConfig,
}

zarrs::plugin::impl_extension_aliases!(Htj2kCodec, v3: "htj2k", []);

// Register into the zarrs Zarr v3 codec plugin registry at link time.
inventory::submit! {
    CodecPluginV3::new::<Htj2kCodec>()
}

impl CodecTraitsV3 for Htj2kCodec {
    fn create(metadata: &MetadataV3) -> Result<Codec, PluginCreateError> {
        let configuration: Configuration = metadata.configuration().cloned().unwrap_or_default();
        let codec = Arc::new(Self::new_with_configuration(&configuration)?);
        Ok(Codec::ArrayToBytes(codec))
    }
}

impl Htj2kCodec {
    /// Create the codec from a parsed [`Htj2kConfig`].
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration requests coding
    /// this build does not implement (e.g. `reversible: false`).
    pub fn new(config: Htj2kConfig) -> Result<Self, PluginCreateError> {
        config
            .validate()
            .map_err(|err| PluginCreateError::Other(err.to_string()))?;
        Ok(Self { config })
    }

    /// Create the codec from Zarr v3 `configuration` metadata.
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration does not parse
    /// or is not implemented by this build.
    pub fn new_with_configuration(
        configuration: &Configuration,
    ) -> Result<Self, PluginCreateError> {
        let config: Htj2kConfig = configuration
            .to_typed()
            .map_err(|err| PluginCreateError::Other(format!("htj2k configuration: {err}")))?;
        Self::new(config)
    }
}

/// The integer sample type behind a supported Zarr data type.
fn sample_type_of(data_type: &DataType) -> Result<SampleType, CodecError> {
    if data_type.is::<UInt8DataType>() {
        Ok(SampleType::U8)
    } else if data_type.is::<Int8DataType>() {
        Ok(SampleType::I8)
    } else if data_type.is::<UInt16DataType>() {
        Ok(SampleType::U16)
    } else if data_type.is::<Int16DataType>() {
        Ok(SampleType::I16)
    } else if data_type.is::<UInt32DataType>() {
        Ok(SampleType::U32)
    } else if data_type.is::<Int32DataType>() {
        Ok(SampleType::I32)
    } else {
        Err(CodecError::UnsupportedDataType(
            data_type.clone(),
            crate::HTJ2K_CODEC_NAME.to_string(),
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

// Passed by value so call sites stay `.map_err(codec_err)`.
#[allow(clippy::needless_pass_by_value)]
fn codec_err(err: ndic_core::Error) -> CodecError {
    CodecError::Other(err.to_string())
}

impl CodecTraits for Htj2kCodec {
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
        // partial_read: the decoder fetches only the index and the planes a
        // subset touches. partial_decode: it decodes only those planes.
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

impl ArrayCodecTraits for Htj2kCodec {
    fn recommended_concurrency(
        &self,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
    ) -> Result<RecommendedConcurrency, CodecError> {
        Ok(RecommendedConcurrency::new_maximum(1))
    }
}

impl ArrayToBytesCodecTraits for Htj2kCodec {
    fn into_dyn(self: Arc<Self>) -> Arc<dyn ArrayToBytesCodecTraits> {
        self as Arc<dyn ArrayToBytesCodecTraits>
    }

    fn encoded_representation(
        &self,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
        _fill_value: &FillValue,
    ) -> Result<BytesRepresentation, CodecError> {
        Ok(BytesRepresentation::UnboundedSize)
    }

    fn encode<'a>(
        &self,
        bytes: ArrayBytes<'a>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytesRaw<'a>, CodecError> {
        let shape = shape_usize(shape)?;
        let dtype = sample_type_of(data_type)?;
        let fixed = bytes.into_fixed()?;
        let out = encode_chunk(&fixed, &shape, dtype, &self.config).map_err(codec_err)?;
        Ok(ArrayBytesRaw::from(out))
    }

    fn decode<'a>(
        &self,
        bytes: ArrayBytesRaw<'a>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        _fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<ArrayBytes<'a>, CodecError> {
        let shape = shape_usize(shape)?;
        let dtype = sample_type_of(data_type)?;
        let out = decode_chunk(&bytes, &shape, dtype).map_err(codec_err)?;
        Ok(ArrayBytes::from(out))
    }

    fn partial_decoder(
        self: Arc<Self>,
        input_handle: Arc<dyn BytesPartialDecoderTraits>,
        shape: &[NonZeroU64],
        data_type: &DataType,
        fill_value: &FillValue,
        _options: &CodecOptions,
    ) -> Result<Arc<dyn ArrayPartialDecoderTraits>, CodecError> {
        Ok(Arc::new(Htj2kPartialDecoder {
            input_handle,
            shape: shape_usize(shape)?,
            shape_u64: shape.iter().map(|d| d.get()).collect(),
            data_type: data_type.clone(),
            dtype: sample_type_of(data_type)?,
            fill_value: fill_value.clone(),
            header: Mutex::new(None),
        }))
    }
}

/// Serves array subsets by fetching and decoding only the planes they
/// touch, located through the coefficient-plane index.
struct Htj2kPartialDecoder {
    input_handle: Arc<dyn BytesPartialDecoderTraits>,
    shape: Vec<usize>,
    shape_u64: Vec<u64>,
    data_type: DataType,
    dtype: SampleType,
    fill_value: FillValue,
    /// The chunk header, fetched once on first use (`None` until then).
    header: Mutex<Option<Arc<ChunkHeader>>>,
}

type StorageByteRange = zarrs::storage::byte_range::ByteRange;

impl Htj2kPartialDecoder {
    /// Fetches `[header | index]` with two ranged reads (fixed header, then
    /// the declared remainder). `Ok(None)` when the chunk does not exist.
    fn fetch_header(&self, options: &CodecOptions) -> Result<Option<Arc<ChunkHeader>>, CodecError> {
        if let Some(header) = self.header.lock().expect("not poisoned").as_ref() {
            return Ok(Some(header.clone()));
        }
        let fixed_len = ChunkHeader::fixed_len(self.shape.len());
        let Some(fixed) = self
            .input_handle
            .partial_decode(StorageByteRange::FromStart(0, Some(fixed_len as u64)), options)?
        else {
            return Ok(None);
        };
        let need = ChunkHeader::required_len(&fixed).map_err(codec_err)?;
        let header = if need <= fixed.len() {
            ChunkHeader::parse(&fixed).map_err(codec_err)?
        } else {
            let Some(full) = self
                .input_handle
                .partial_decode(StorageByteRange::FromStart(0, Some(need as u64)), options)?
            else {
                return Ok(None);
            };
            ChunkHeader::parse(&full).map_err(codec_err)?
        };
        let header = Arc::new(header);
        *self.header.lock().expect("not poisoned") = Some(header.clone());
        Ok(Some(header))
    }

    /// Fill-value bytes for `len` elements.
    fn fill_bytes(&self, len: u64) -> ArrayBytes<'static> {
        let element = self.fill_value.as_ne_bytes();
        let mut out = Vec::with_capacity(element.len() * usize::try_from(len).unwrap_or(0));
        for _ in 0..len {
            out.extend_from_slice(element);
        }
        ArrayBytes::from(out)
    }

    /// Whole-chunk fallback: fetch everything, decode, extract.
    fn decode_all(
        &self,
        indexer: &dyn Indexer,
        options: &CodecOptions,
    ) -> Result<ArrayBytes<'_>, CodecError> {
        let Some(chunk) = self.input_handle.decode(options)? else {
            return Ok(self.fill_bytes(indexer.len()));
        };
        let bytes = decode_chunk(&chunk, &self.shape, self.dtype).map_err(codec_err)?;
        let extracted = ArrayBytes::from(bytes)
            .extract_array_subset(indexer, &self.shape_u64, &self.data_type)?
            .into_owned();
        Ok(extracted)
    }
}

impl ArrayPartialDecoderTraits for Htj2kPartialDecoder {
    fn data_type(&self) -> &DataType {
        &self.data_type
    }

    fn exists(&self) -> Result<bool, zarrs::storage::StorageError> {
        self.input_handle.exists()
    }

    fn size_held(&self) -> usize {
        self.input_handle.size_held()
    }

    fn supports_partial_decode(&self) -> bool {
        true
    }

    fn partial_decode(
        &self,
        indexer: &dyn Indexer,
        options: &CodecOptions,
    ) -> Result<ArrayBytes<'_>, CodecError> {
        // Plane-selective reads need an axis-aligned subset and an on-chunk
        // index; anything else decodes the whole chunk.
        let Some(subset) = indexer.as_array_subset() else {
            return self.decode_all(indexer, options);
        };
        let Some(header) = self.fetch_header(options)? else {
            return Ok(self.fill_bytes(indexer.len()));
        };
        let Some(entries) = header.planes.as_ref() else {
            return self.decode_all(indexer, options);
        };

        let ndim = self.shape.len();
        let to_usize = |values: &[u64]| -> Option<Vec<usize>> {
            values.iter().map(|&v| usize::try_from(v).ok()).collect()
        };
        let (Some(start), Some(sel_shape)) =
            (to_usize(&subset.start()), to_usize(&subset.shape()))
        else {
            return self.decode_all(indexer, options);
        };
        if start.len() != ndim {
            return self.decode_all(indexer, options);
        }
        let plane_h = header.plane_height() as usize;
        let plane_w = header.plane_width() as usize;
        let (y0, x0) = (start[ndim - 2], start[ndim - 1]);
        let (sel_h, sel_w) = (sel_shape[ndim - 2], sel_shape[ndim - 1]);
        if y0 + sel_h > plane_h || x0 + sel_w > plane_w {
            return Err(CodecError::Other(
                "htj2k partial decode: subset exceeds the chunk".into(),
            ));
        }

        // The planes the subset touches, in C order of the leading dims.
        let mut planes: Vec<usize> = vec![0];
        for d in 0..ndim - 2 {
            let extent: usize = self.shape[..ndim - 2][d + 1..].iter().product();
            let mut next = Vec::with_capacity(planes.len() * sel_shape[d]);
            for base in &planes {
                for i in 0..sel_shape[d] {
                    next.push(base + (start[d] + i) * extent);
                }
            }
            planes = next;
        }

        // One ranged read per plane; decode and copy the (y, x) window.
        let element = self.dtype.size_bytes();
        let mut out = Vec::with_capacity(planes.len() * sel_h * sel_w * element);
        for &p in &planes {
            let entry = entries.get(p).ok_or_else(|| {
                CodecError::Other(format!("htj2k chunk index has no plane {p}"))
            })?;
            let Some(stream) = self.input_handle.partial_decode(
                StorageByteRange::FromStart(entry.offset, Some(u64::from(entry.len))),
                options,
            )?
            else {
                return Ok(self.fill_bytes(indexer.len()));
            };
            let samples = decode_plane(&header, &stream).map_err(codec_err)?;
            let mut window = Vec::with_capacity(sel_h * sel_w);
            for row in 0..sel_h {
                let at = (y0 + row) * plane_w + x0;
                window.extend_from_slice(&samples[at..at + sel_w]);
            }
            let mut bytes = Vec::with_capacity(window.len() * element);
            crate::htj2k::narrow_samples(&window, self.dtype, &mut bytes).map_err(codec_err)?;
            out.extend_from_slice(&bytes);
        }
        Ok(ArrayBytes::from(out))
    }
}
