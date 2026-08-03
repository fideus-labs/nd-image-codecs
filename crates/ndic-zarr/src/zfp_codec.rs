//! The `nd_zfp` Zarr v3 **array-to-bytes** codec, registered into the
//! `zarrs` plugin registry.
//!
//! Wraps the chunk core in [`ndic_zfp`]: the chunk's singleton dimensions
//! are squeezed away and the remainder is compressed as a
//! `dims`-dimensional ZFP field with the full ZFP header — a stream
//! `zfpy`/`imagecodecs` can cross-decode. In **fixed-rate** mode every
//! `4^d` brick has a fixed bit budget at a computed offset, so the partial
//! decoder fetches only the byte ranges spanning the bricks an
//! [`ArraySubset`](zarrs::array::ArraySubset) touches and decodes those
//! bricks alone; the variable-size modes fall back to a whole-chunk
//! decode.

use std::num::NonZeroU64;
use std::sync::Arc;

use zarrs::array::codec::api::{
    ArrayBytes, ArrayBytesRaw, ArrayCodecTraits, ArrayPartialDecoderTraits,
    ArrayToBytesCodecTraits, BytesPartialDecoderTraits, BytesRepresentation, Codec, CodecError,
    CodecMetadataOptions, CodecOptions, CodecPluginV3, CodecTraits, CodecTraitsV3,
    PartialDecoderCapability, PartialEncoderCapability, RecommendedConcurrency,
};
use zarrs::array::data_type::{
    Float32DataType, Float64DataType, Int8DataType, Int16DataType, Int32DataType, Int64DataType,
    UInt8DataType, UInt16DataType,
};
use zarrs::array::{DataType, FillValue, Indexer};
use zarrs::metadata::Configuration;
use zarrs::metadata::v3::MetadataV3;
use zarrs::plugin::{PluginCreateError, ZarrVersion};

use ndic_zfp::{
    BrickIndex, NdZfpConfig, ZfpDtype, ZfpMode, decode_chunk, decode_chunk_brick, encode_chunk,
};

/// The `nd_zfp` codec: ZFP-compressed chunks with O(1) brick addressing in
/// fixed-rate mode (`docs/architecture/zfp.md`).
#[derive(Clone, Debug)]
pub struct NdZfpCodec {
    config: NdZfpConfig,
}

zarrs::plugin::impl_extension_aliases!(NdZfpCodec, v3: "nd_zfp", []);

// Register into the zarrs Zarr v3 codec plugin registry at link time.
inventory::submit! {
    CodecPluginV3::new::<NdZfpCodec>()
}

impl CodecTraitsV3 for NdZfpCodec {
    fn create(metadata: &MetadataV3) -> Result<Codec, PluginCreateError> {
        let configuration: Configuration = metadata.configuration().cloned().unwrap_or_default();
        let codec = Arc::new(Self::new_with_configuration(&configuration)?);
        Ok(Codec::ArrayToBytes(codec))
    }
}

impl NdZfpCodec {
    /// Create the codec from a parsed [`NdZfpConfig`].
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration is not a valid
    /// mode/parameter combination.
    pub fn new(config: NdZfpConfig) -> Result<Self, PluginCreateError> {
        config
            .validate()
            .map_err(|err| PluginCreateError::Other(err.to_string()))?;
        Ok(Self { config })
    }

    /// Create the codec from Zarr v3 `configuration` metadata.
    ///
    /// # Errors
    /// Returns [`PluginCreateError`] when the configuration does not parse
    /// or is not a valid mode/parameter combination.
    pub fn new_with_configuration(
        configuration: &Configuration,
    ) -> Result<Self, PluginCreateError> {
        let config: NdZfpConfig = configuration
            .to_typed()
            .map_err(|err| PluginCreateError::Other(format!("nd_zfp configuration: {err}")))?;
        Self::new(config)
    }
}

/// The ZFP sample type behind a supported Zarr data type.
fn zfp_dtype_of(data_type: &DataType) -> Result<ZfpDtype, CodecError> {
    if data_type.is::<UInt8DataType>() {
        Ok(ZfpDtype::U8)
    } else if data_type.is::<Int8DataType>() {
        Ok(ZfpDtype::I8)
    } else if data_type.is::<UInt16DataType>() {
        Ok(ZfpDtype::U16)
    } else if data_type.is::<Int16DataType>() {
        Ok(ZfpDtype::I16)
    } else if data_type.is::<Int32DataType>() {
        Ok(ZfpDtype::I32)
    } else if data_type.is::<Int64DataType>() {
        Ok(ZfpDtype::I64)
    } else if data_type.is::<Float32DataType>() {
        Ok(ZfpDtype::F32)
    } else if data_type.is::<Float64DataType>() {
        Ok(ZfpDtype::F64)
    } else {
        Err(CodecError::UnsupportedDataType(
            data_type.clone(),
            ndic_zfp::CODEC_NAME.to_string(),
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

impl CodecTraits for NdZfpCodec {
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
        // partial_read/partial_decode: in fixed-rate mode the decoder
        // fetches and decodes only the bricks a subset touches.
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

impl ArrayCodecTraits for NdZfpCodec {
    fn recommended_concurrency(
        &self,
        _shape: &[NonZeroU64],
        _data_type: &DataType,
    ) -> Result<RecommendedConcurrency, CodecError> {
        Ok(RecommendedConcurrency::new_maximum(1))
    }
}

impl ArrayToBytesCodecTraits for NdZfpCodec {
    fn into_dyn(self: Arc<Self>) -> Arc<dyn ArrayToBytesCodecTraits> {
        self as Arc<dyn ArrayToBytesCodecTraits>
    }

    fn encoded_representation(
        &self,
        shape: &[NonZeroU64],
        data_type: &DataType,
        _fill_value: &FillValue,
    ) -> Result<BytesRepresentation, CodecError> {
        // Fixed-rate streams have a computable exact size; the other modes
        // are data-dependent.
        if let (Ok(ZfpMode::FixedRate(rate)), Ok(shape), Ok(dtype)) = (
            self.config.zfp_mode(),
            shape_usize(shape),
            zfp_dtype_of(data_type),
        ) && let Ok(effective) = self.config.effective_shape(&shape)
            && let Ok(index) = BrickIndex::fixed_rate(&effective, dtype.scalar_kind(), rate)
        {
            return Ok(BytesRepresentation::FixedSize(index.stream_len() as u64));
        }
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
        let dtype = zfp_dtype_of(data_type)?;
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
        let dtype = zfp_dtype_of(data_type)?;
        let out = decode_chunk(&bytes, &shape, dtype, &self.config).map_err(codec_err)?;
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
        Ok(Arc::new(NdZfpPartialDecoder {
            input_handle,
            shape: shape_usize(shape)?,
            shape_u64: shape.iter().map(|d| d.get()).collect(),
            data_type: data_type.clone(),
            dtype: zfp_dtype_of(data_type)?,
            fill_value: fill_value.clone(),
            config: self.config.clone(),
        }))
    }
}

/// Serves array subsets of fixed-rate chunks by fetching and decoding only
/// the `4^d` bricks they touch, at offsets computed from the rate; the
/// variable-size modes decode the whole chunk once and slice.
struct NdZfpPartialDecoder {
    input_handle: Arc<dyn BytesPartialDecoderTraits>,
    shape: Vec<usize>,
    shape_u64: Vec<u64>,
    data_type: DataType,
    dtype: ZfpDtype,
    fill_value: FillValue,
    config: NdZfpConfig,
}

type StorageByteRange = zarrs::storage::byte_range::ByteRange;

/// Call `f` with every coordinate vector in the axis-aligned box
/// `lo[i]..=hi[i]` (row-major order; last axis fastest). An empty box
/// yields the single empty coordinate vector.
fn for_each_coord(
    lo: &[usize],
    hi: &[usize],
    f: &mut dyn FnMut(&[usize]) -> Result<(), CodecError>,
) -> Result<(), CodecError> {
    let mut pos = lo.to_vec();
    loop {
        f(&pos)?;
        let mut axis = pos.len();
        loop {
            if axis == 0 {
                return Ok(());
            }
            axis -= 1;
            pos[axis] += 1;
            if pos[axis] <= hi[axis] {
                break;
            }
            pos[axis] = lo[axis];
        }
    }
}

/// Row-major linearization of `pos` within `shape`.
fn flatten(pos: &[usize], shape: &[usize]) -> usize {
    pos.iter().zip(shape).fold(0, |acc, (&p, &s)| acc * s + p)
}

impl NdZfpPartialDecoder {
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
        let bytes =
            decode_chunk(&chunk, &self.shape, self.dtype, &self.config).map_err(codec_err)?;
        let extracted = ArrayBytes::from(bytes)
            .extract_array_subset(indexer, &self.shape_u64, &self.data_type)?
            .into_owned();
        Ok(extracted)
    }

    /// The subset mapped from chunk axes onto the effective (squeezed,
    /// padded) ZFP field axes, or `None` when the mapping does not apply
    /// and the caller should fall back to a whole-chunk decode.
    fn effective_subset(
        &self,
        start: &[usize],
        sel: &[usize],
        effective_rank: usize,
    ) -> Option<(Vec<usize>, Vec<usize>)> {
        let squeezed_rank = self.shape.iter().filter(|&&d| d > 1).count();
        let pad = effective_rank - squeezed_rank;
        let mut eff_start = vec![0usize; pad];
        let mut eff_sel = vec![1usize; pad];
        for ((&extent, &s), &l) in self.shape.iter().zip(start).zip(sel) {
            if extent > 1 {
                eff_start.push(s);
                eff_sel.push(l);
            } else if s != 0 || l != 1 {
                // A singleton axis with a non-trivial selection: bail out.
                return None;
            }
        }
        Some((eff_start, eff_sel))
    }

    /// Brick-selective fixed-rate read: fetch the header plus one byte
    /// range per touched brick row, decode those bricks, assemble the
    /// subset.
    #[allow(clippy::too_many_lines)]
    fn decode_bricks(
        &self,
        rate: f64,
        eff_start: &[usize],
        eff_sel: &[usize],
        effective: &[usize],
        indexer_len: u64,
        options: &CodecOptions,
    ) -> Result<Option<ArrayBytes<'_>>, CodecError> {
        let rank = effective.len();
        let index =
            BrickIndex::fixed_rate(effective, self.dtype.scalar_kind(), rate).map_err(codec_err)?;

        // Sparse reconstruction of the stream: only the header and the
        // touched bricks' byte ranges are fetched.
        let mut stream = vec![0u8; index.stream_len()];
        let header_len = usize::try_from(index.header_bits().div_ceil(8)).expect("header fits");
        let Some(header) = self.input_handle.partial_decode(
            StorageByteRange::FromStart(0, Some(header_len as u64)),
            options,
        )?
        else {
            return Ok(None);
        };
        if header.len() < header_len {
            return Err(CodecError::Other(
                "nd_zfp partial decode: chunk shorter than the ZFP header".into(),
            ));
        }
        stream[..header_len].copy_from_slice(&header[..header_len]);

        let brick_lo: Vec<usize> = eff_start.iter().map(|&s| s / 4).collect();
        let brick_hi: Vec<usize> = eff_start
            .iter()
            .zip(eff_sel)
            .map(|(&s, &l)| (s + l - 1) / 4)
            .collect();

        // One ranged read per brick row (the last axis' bricks are
        // contiguous in the stream).
        let mut absent = false;
        for_each_coord(&brick_lo[..rank - 1], &brick_hi[..rank - 1], &mut |row| {
            let mut first = row.to_vec();
            first.push(brick_lo[rank - 1]);
            let mut last = row.to_vec();
            last.push(brick_hi[rank - 1]);
            let k0 = index.linear(&first).map_err(codec_err)?;
            let k1 = index.linear(&last).map_err(codec_err)?;
            let (off0, _) = index.byte_range(k0).map_err(codec_err)?;
            let (off1, len1) = index.byte_range(k1).map_err(codec_err)?;
            let span = off1 + len1 - off0;
            let Some(bytes) = self
                .input_handle
                .partial_decode(StorageByteRange::FromStart(off0, Some(span)), options)?
            else {
                absent = true;
                return Ok(());
            };
            let at = usize::try_from(off0).expect("offset fits");
            let span = usize::try_from(span).expect("span fits");
            if bytes.len() < span {
                return Err(CodecError::Other(
                    "nd_zfp partial decode: chunk shorter than a brick range".into(),
                ));
            }
            stream[at..at + span].copy_from_slice(&bytes[..span]);
            Ok(())
        })?;
        if absent {
            return Ok(Some(self.fill_bytes(indexer_len)));
        }

        // Decode each touched brick and copy its intersection window.
        let esize = self.dtype.size_bytes();
        let elements = usize::try_from(indexer_len).expect("subset fits");
        let mut out = vec![0u8; elements * esize];
        for_each_coord(&brick_lo, &brick_hi, &mut |brick| {
            let (brick_bytes, brick_shape) =
                decode_chunk_brick(&stream, &self.shape, self.dtype, &self.config, brick)
                    .map_err(codec_err)?;
            let origin: Vec<usize> = brick.iter().map(|&b| b * 4).collect();
            let lo: Vec<usize> = origin
                .iter()
                .zip(eff_start)
                .map(|(&o, &s)| o.max(s))
                .collect();
            let hi: Vec<usize> = origin
                .iter()
                .zip(&brick_shape)
                .zip(eff_start.iter().zip(eff_sel))
                .map(|((&o, &b), (&s, &l))| (o + b).min(s + l))
                .collect();
            if lo.iter().zip(&hi).any(|(&l, &h)| l >= h) {
                return Ok(());
            }
            // Copy one row (last-axis run) at a time.
            let run = hi[rank - 1] - lo[rank - 1];
            let hi_rows: Vec<usize> = hi[..rank - 1].iter().map(|&h| h - 1).collect();
            for_each_coord(&lo[..rank - 1], &hi_rows, &mut |row| {
                let mut src_pos: Vec<usize> =
                    row.iter().zip(&origin).map(|(&p, &o)| p - o).collect();
                src_pos.push(lo[rank - 1] - origin[rank - 1]);
                let mut dst_pos: Vec<usize> =
                    row.iter().zip(eff_start).map(|(&p, &s)| p - s).collect();
                dst_pos.push(lo[rank - 1] - eff_start[rank - 1]);
                let src = flatten(&src_pos, &brick_shape) * esize;
                let dst = flatten(&dst_pos, eff_sel) * esize;
                out[dst..dst + run * esize].copy_from_slice(&brick_bytes[src..src + run * esize]);
                Ok(())
            })
        })?;
        Ok(Some(ArrayBytes::from(out)))
    }
}

impl ArrayPartialDecoderTraits for NdZfpPartialDecoder {
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
        // Brick-selective reads need fixed-rate mode and an axis-aligned
        // subset; anything else decodes the whole chunk.
        let Some(subset) = indexer.as_array_subset() else {
            return self.decode_all(indexer, options);
        };
        let Ok(ZfpMode::FixedRate(rate)) = self.config.zfp_mode() else {
            return self.decode_all(indexer, options);
        };
        let Ok(effective) = self.config.effective_shape(&self.shape) else {
            return self.decode_all(indexer, options);
        };
        let to_usize = |values: &[u64]| -> Option<Vec<usize>> {
            values.iter().map(|&v| usize::try_from(v).ok()).collect()
        };
        let (Some(start), Some(sel)) = (to_usize(&subset.start()), to_usize(&subset.shape()))
        else {
            return self.decode_all(indexer, options);
        };
        if start.len() != self.shape.len() {
            return self.decode_all(indexer, options);
        }
        if sel.contains(&0) {
            return Ok(ArrayBytes::from(Vec::new()));
        }
        let Some((eff_start, eff_sel)) = self.effective_subset(&start, &sel, effective.len())
        else {
            return self.decode_all(indexer, options);
        };
        if eff_start
            .iter()
            .zip(&eff_sel)
            .zip(&effective)
            .any(|((&s, &l), &n)| s + l > n)
        {
            return Err(CodecError::Other(
                "nd_zfp partial decode: subset exceeds the chunk".into(),
            ));
        }
        match self.decode_bricks(
            rate,
            &eff_start,
            &eff_sel,
            &effective,
            indexer.len(),
            options,
        )? {
            Some(bytes) => Ok(bytes),
            None => Ok(self.fill_bytes(indexer.len())),
        }
    }
}
