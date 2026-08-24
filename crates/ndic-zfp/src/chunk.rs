//! The `zfp` chunk codec core: chunk bytes ⇄ ZFP stream.
//!
//! Feature-free on purpose (serde derives for [`NdZfpConfig`] sit behind
//! the `serde` feature): the `zarrs` codec, the Python (pyo3) binding, and
//! the WASM (TypeScript) binding all call these functions, so every
//! ecosystem produces byte-identical chunks.
//!
//! Under the registered `zfp` codec semantics (no `dims` in the
//! configuration), the chunk shape maps **directly** onto the ZFP field —
//! 1 to 4 dimensions, exactly as zarr-extensions specifies — and the
//! codec-series builder collapses singleton dimensions with a `reshape`
//! codec upstream. A configuration carrying the legacy `dims` member (data
//! written under the deprecated `nd_zfp` name) instead selects the old
//! in-codec mapping: singleton axes squeezed away, the remainder
//! left-padded with size-1 axes up to `dims` — so existing stores decode
//! byte-for-byte.
//!
//! `u8`/`i8`/`u16`/`i16` samples are promoted into `i32` exactly as the C
//! library's `zfp_promote_*` helpers do (shift into the high-order bits,
//! biasing unsigned types to signed), so lossy rates spend their bit
//! budget on the samples' actual dynamic range and reversible mode
//! round-trips bit-exactly through the matching demotion.

use ndic_core::{Error, Result};

use crate::{ZfpMode, ZfpScalarKind, compress, decompress};

/// Sample types the `nd_zfp` chunk codec accepts: ZFP's four native types
/// plus the narrower integers it promotes to `i32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZfpDtype {
    /// Unsigned 8-bit (promoted to `i32`).
    U8,
    /// Signed 8-bit (promoted to `i32`).
    I8,
    /// Unsigned 16-bit (promoted to `i32`).
    U16,
    /// Signed 16-bit (promoted to `i32`).
    I16,
    /// Signed 32-bit (native).
    I32,
    /// Signed 64-bit (native).
    I64,
    /// IEEE 754 single precision (native).
    F32,
    /// IEEE 754 double precision (native).
    F64,
}

impl ZfpDtype {
    /// Parse a Zarr data-type name (or NumPy-style string code).
    #[must_use]
    pub fn from_zarr_name(name: &str) -> Option<Self> {
        match name {
            "uint8" | "|u1" | "u1" => Some(Self::U8),
            "int8" | "|i1" | "i1" => Some(Self::I8),
            "uint16" | "<u2" | "u2" => Some(Self::U16),
            "int16" | "<i2" | "i2" => Some(Self::I16),
            "int32" | "<i4" | "i4" => Some(Self::I32),
            "int64" | "<i8" | "i8" => Some(Self::I64),
            "float32" | "<f4" | "f4" => Some(Self::F32),
            "float64" | "<f8" | "f8" => Some(Self::F64),
            _ => None,
        }
    }

    /// Bytes per stored sample.
    #[must_use]
    pub fn size_bytes(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::I32 | Self::F32 => 4,
            Self::I64 | Self::F64 => 8,
        }
    }

    /// The ZFP scalar type this dtype is coded as (narrow integers are
    /// promoted to `i32`).
    #[must_use]
    pub fn scalar_kind(self) -> ZfpScalarKind {
        match self {
            Self::U8 | Self::I8 | Self::U16 | Self::I16 | Self::I32 => ZfpScalarKind::I32,
            Self::I64 => ZfpScalarKind::I64,
            Self::F32 => ZfpScalarKind::F32,
            Self::F64 => ZfpScalarKind::F64,
        }
    }
}

/// The `nd_zfp` codec `configuration` object (Zarr v3), shared verbatim by
/// the Rust, Python, and TypeScript builders.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct NdZfpConfig {
    /// Compression mode: `reversible`, `fixed_rate`, `fixed_accuracy`, or
    /// `fixed_precision`.
    #[cfg_attr(feature = "serde", serde(default = "default_mode"))]
    pub mode: String,
    /// Bits per value (`fixed_rate` only).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub rate: Option<f64>,
    /// Absolute error tolerance (`fixed_accuracy` only).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub tolerance: Option<f64>,
    /// Uncompressed bit planes per value (`fixed_precision` only).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub precision: Option<u32>,
    /// Legacy `nd_zfp` field dimensionality (1–4). Present only in
    /// configurations written under the deprecated `nd_zfp` name, where it
    /// selects the old squeeze-and-pad chunk mapping; the registered `zfp`
    /// codec maps the chunk shape directly and never writes this.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub dims: Option<u8>,
}

fn default_mode() -> String {
    "reversible".into()
}

impl Default for NdZfpConfig {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            rate: None,
            tolerance: None,
            precision: None,
            dims: None,
        }
    }
}

impl NdZfpConfig {
    /// Validates the configuration's structure: a known mode, exactly its
    /// own parameter present, and `dims` in `1..=4`.
    ///
    /// # Errors
    /// [`Error::Unsupported`] for an unknown mode;
    /// [`Error::InvalidArgument`] for missing, extraneous, or out-of-range
    /// parameters.
    pub fn validate(&self) -> Result<()> {
        self.zfp_mode().map(|_| ())
    }

    /// The parsed [`ZfpMode`].
    ///
    /// # Errors
    /// As [`NdZfpConfig::validate`].
    pub fn zfp_mode(&self) -> Result<ZfpMode> {
        if let Some(dims) = self.dims
            && !(1..=4).contains(&dims)
        {
            return Err(Error::InvalidArgument {
                message: format!("zfp: dims must be 1..=4, got {dims}"),
            });
        }
        let mode = match self.mode.as_str() {
            "reversible" => {
                self.forbid(self.rate.is_some(), "rate")?;
                self.forbid(self.tolerance.is_some(), "tolerance")?;
                self.forbid(self.precision.is_some(), "precision")?;
                ZfpMode::Reversible
            }
            "fixed_rate" => {
                self.forbid(self.tolerance.is_some(), "tolerance")?;
                self.forbid(self.precision.is_some(), "precision")?;
                ZfpMode::FixedRate(self.rate.ok_or_else(|| Error::InvalidArgument {
                    message: "zfp: fixed_rate mode needs a \"rate\"".into(),
                })?)
            }
            "fixed_accuracy" => {
                self.forbid(self.rate.is_some(), "rate")?;
                self.forbid(self.precision.is_some(), "precision")?;
                ZfpMode::FixedAccuracy(self.tolerance.ok_or_else(|| Error::InvalidArgument {
                    message: "zfp: fixed_accuracy mode needs a \"tolerance\"".into(),
                })?)
            }
            "fixed_precision" => {
                self.forbid(self.rate.is_some(), "rate")?;
                self.forbid(self.tolerance.is_some(), "tolerance")?;
                ZfpMode::FixedPrecision(self.precision.ok_or_else(|| Error::InvalidArgument {
                    message: "zfp: fixed_precision mode needs a \"precision\"".into(),
                })?)
            }
            other => {
                return Err(Error::Unsupported {
                    message: format!("zfp: unknown mode {other:?}"),
                });
            }
        };
        mode.validate()?;
        Ok(mode)
    }

    fn forbid(&self, present: bool, what: &str) -> Result<()> {
        if present {
            return Err(Error::InvalidArgument {
                message: format!("zfp: {:?} mode does not take {what:?}", self.mode),
            });
        }
        Ok(())
    }

    /// The ZFP field shape a chunk of `chunk_shape` compresses as — what
    /// [`crate::BrickIndex::fixed_rate`] expects. Without legacy `dims`,
    /// the chunk shape itself (1–4 dimensions); with it, the old
    /// squeeze-and-pad mapping.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for empty/zero-extent shapes, an
    /// out-of-range `dims`, or a geometry the mapping cannot express.
    pub fn effective_shape(&self, chunk_shape: &[usize]) -> Result<Vec<usize>> {
        if let Some(dims) = self.dims
            && !(1..=4).contains(&dims)
        {
            return Err(invalid(format!("zfp: dims must be 1..=4, got {dims}")));
        }
        effective_shape(chunk_shape, self.dims)
    }
}

fn invalid(message: String) -> Error {
    Error::InvalidArgument { message }
}

/// The field shape a chunk compresses as.
///
/// `dims: None` is the registered `zfp` mapping: the chunk shape **is** the
/// field shape, and must have 1–4 dimensions (the codec-series builder
/// collapses singletons with a `reshape` codec upstream). `dims: Some` is
/// the legacy `nd_zfp` mapping: singleton axes squeezed away, then
/// left-padded with size-1 axes up to `dims`.
fn effective_shape(shape: &[usize], dims: Option<u8>) -> Result<Vec<usize>> {
    if shape.is_empty() {
        return Err(invalid("zfp: chunk shape has no dimensions".into()));
    }
    if shape.contains(&0) {
        return Err(invalid(format!(
            "zfp: chunk shape {shape:?} has a zero extent"
        )));
    }
    let Some(dims) = dims else {
        if shape.len() > 4 {
            return Err(invalid(format!(
                "zfp: chunk shape {shape:?} has {} dimensions but ZFP fields are 1-4 \
                 dimensional; collapse singleton dimensions with a reshape codec upstream",
                shape.len()
            )));
        }
        return Ok(shape.to_vec());
    };
    let dims = usize::from(dims);
    let squeezed: Vec<usize> = shape.iter().copied().filter(|&d| d > 1).collect();
    if squeezed.len() > dims {
        return Err(invalid(format!(
            "zfp: chunk shape {shape:?} has {} non-singleton dimensions but the \
             configuration declares dims={dims}; reduce chunking or raise dims",
            squeezed.len()
        )));
    }
    let mut effective = vec![1usize; dims - squeezed.len()];
    effective.extend(squeezed);
    Ok(effective)
}

/// Chunk length check shared by encode and decode.
fn checked_elements(
    shape: &[usize],
    effective: &[usize],
    len: usize,
    dtype: ZfpDtype,
) -> Result<usize> {
    let elements = effective
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| invalid(format!("zfp: chunk shape {shape:?} overflows usize")))?;
    let expected = elements
        .checked_mul(dtype.size_bytes())
        .ok_or_else(|| invalid(format!("zfp: chunk shape {shape:?} overflows usize")))?;
    if len != expected {
        return Err(invalid(format!(
            "zfp: chunk shape {shape:?} ({dtype:?}) needs {expected} bytes, got {len}"
        )));
    }
    Ok(elements)
}

fn typed_vec<T: crate::ZfpElement>(bytes: &[u8]) -> Vec<T> {
    let mut out = vec![T::default(); bytes.len() / size_of::<T>()];
    bytemuck::cast_slice_mut::<T, u8>(&mut out).copy_from_slice(bytes);
    out
}

/// Promote narrow integer samples into `i32` exactly as the C library's
/// `zfp_promote_*` helpers do (shift into the high bits; bias unsigned).
fn promoted_i32(chunk: &[u8], dtype: ZfpDtype) -> Vec<i32> {
    match dtype {
        ZfpDtype::U8 => chunk.iter().map(|&v| (i32::from(v) - 0x80) << 23).collect(),
        ZfpDtype::I8 => chunk
            .iter()
            .map(|&v| i32::from(v.cast_signed()) << 23)
            .collect(),
        // `as_chunks::<2>().0` is the same complete-chunk prefix `chunks_exact(2)`
        // walked — the ragged tail is dropped either way — but it yields
        // `&[u8; 2]`, so `from_le_bytes` takes the array directly and the
        // per-element bounds checks go away.
        ZfpDtype::U16 => chunk
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| (i32::from(u16::from_le_bytes(b)) - 0x8000) << 15)
            .collect(),
        ZfpDtype::I16 => chunk
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| i32::from(i16::from_le_bytes(b)) << 15)
            .collect(),
        _ => unreachable!("promotion is only for narrow integer dtypes"),
    }
}

/// Demote `i32` samples back to the narrow integer dtype (shift down with
/// clamping), the inverse of [`promoted_i32`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to the dtype range
fn demoted_bytes(values: &[i32], dtype: ZfpDtype) -> Vec<u8> {
    match dtype {
        ZfpDtype::U8 => values
            .iter()
            .map(|&v| ((v >> 23) + 0x80).clamp(0x00, 0xff) as u8)
            .collect(),
        ZfpDtype::I8 => values
            .iter()
            .map(|&v| (v >> 23).clamp(-0x80, 0x7f) as i8 as u8)
            .collect(),
        ZfpDtype::U16 => values
            .iter()
            .flat_map(|&v| (((v >> 15) + 0x8000).clamp(0x0000, 0xffff) as u16).to_le_bytes())
            .collect(),
        ZfpDtype::I16 => values
            .iter()
            .flat_map(|&v| ((v >> 15).clamp(-0x8000, 0x7fff) as i16).to_le_bytes())
            .collect(),
        _ => unreachable!("demotion is only for narrow integer dtypes"),
    }
}

/// Encodes a chunk (little-endian elements, C order) into a ZFP stream.
///
/// # Errors
/// [`Error::InvalidArgument`]/[`Error::Unsupported`] for configuration or
/// geometry violations.
pub fn encode_chunk(
    chunk: &[u8],
    shape: &[usize],
    dtype: ZfpDtype,
    config: &NdZfpConfig,
) -> Result<Vec<u8>> {
    let mode = config.zfp_mode()?;
    let effective = effective_shape(shape, config.dims)?;
    checked_elements(shape, &effective, chunk.len(), dtype)?;
    match dtype {
        ZfpDtype::F32 => compress(&typed_vec::<f32>(chunk), &effective, mode),
        ZfpDtype::F64 => compress(&typed_vec::<f64>(chunk), &effective, mode),
        ZfpDtype::I32 => compress(&typed_vec::<i32>(chunk), &effective, mode),
        ZfpDtype::I64 => compress(&typed_vec::<i64>(chunk), &effective, mode),
        ZfpDtype::U8 | ZfpDtype::I8 | ZfpDtype::U16 | ZfpDtype::I16 => {
            compress(&promoted_i32(chunk, dtype), &effective, mode)
        }
    }
}

/// Decodes an `nd_zfp` chunk back to little-endian elements in C order.
///
/// # Errors
/// [`Error::InvalidArgument`]/[`Error::Unsupported`] for configuration or
/// geometry violations; [`Error::Codestream`] for a malformed stream.
pub fn decode_chunk(
    bytes: &[u8],
    shape: &[usize],
    dtype: ZfpDtype,
    config: &NdZfpConfig,
) -> Result<Vec<u8>> {
    fn native<T: crate::ZfpElement + bytemuck::Pod>(
        bytes: &[u8],
        effective: &[usize],
        mode: ZfpMode,
        elements: usize,
    ) -> Result<Vec<u8>> {
        let mut out = vec![T::default(); elements];
        decompress(bytes, effective, mode, &mut out)?;
        Ok(bytemuck::cast_slice::<T, u8>(&out).to_vec())
    }
    let mode = config.zfp_mode()?;
    let effective = effective_shape(shape, config.dims)?;
    // Decode allocates the output itself; validate the geometry only.
    let elements = effective
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| invalid(format!("zfp: chunk shape {shape:?} overflows usize")))?;
    match dtype {
        ZfpDtype::F32 => native::<f32>(bytes, &effective, mode, elements),
        ZfpDtype::F64 => native::<f64>(bytes, &effective, mode, elements),
        ZfpDtype::I32 => native::<i32>(bytes, &effective, mode, elements),
        ZfpDtype::I64 => native::<i64>(bytes, &effective, mode, elements),
        ZfpDtype::U8 | ZfpDtype::I8 | ZfpDtype::U16 | ZfpDtype::I16 => {
            let mut out = vec![0i32; elements];
            decompress(bytes, &effective, mode, &mut out)?;
            Ok(demoted_bytes(&out, dtype))
        }
    }
}

/// The typed reader behind [`NdZfpBrickDecoder`], keyed by the dtype's
/// coded scalar (narrow integers ride the promoted `i32` reader).
enum BrickReaderKind {
    I32(crate::BrickReader<i32>),
    I64(crate::BrickReader<i64>),
    F32(crate::BrickReader<f32>),
    F64(crate::BrickReader<f64>),
}

/// A prepared fixed-rate `nd_zfp` chunk for repeated brick decodes: the
/// stream buffering and header validation are paid **once** at
/// construction, then every [`NdZfpBrickDecoder::decode_brick`] call is a
/// seek plus one block decode. Prefer this over [`decode_chunk_brick`]
/// whenever more than one brick of the same chunk is read (the codec's
/// partial decoder does).
pub struct NdZfpBrickDecoder {
    reader: BrickReaderKind,
    dtype: ZfpDtype,
}

impl NdZfpBrickDecoder {
    /// Prepare a fixed-rate chunk for brick decoding.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] unless the configuration is
    /// `fixed_rate`, or for geometry violations; [`Error::Codestream`]
    /// for a malformed stream.
    pub fn new(
        bytes: &[u8],
        shape: &[usize],
        dtype: ZfpDtype,
        config: &NdZfpConfig,
    ) -> Result<Self> {
        let ZfpMode::FixedRate(rate) = config.zfp_mode()? else {
            return Err(invalid(format!(
                "zfp: brick decode needs fixed_rate mode, configuration says {:?}",
                config.mode
            )));
        };
        let effective = effective_shape(shape, config.dims)?;
        let reader = match dtype.scalar_kind() {
            crate::ZfpScalarKind::I32 => {
                BrickReaderKind::I32(crate::BrickReader::fixed_rate(bytes, &effective, rate)?)
            }
            crate::ZfpScalarKind::I64 => {
                BrickReaderKind::I64(crate::BrickReader::fixed_rate(bytes, &effective, rate)?)
            }
            crate::ZfpScalarKind::F32 => {
                BrickReaderKind::F32(crate::BrickReader::fixed_rate(bytes, &effective, rate)?)
            }
            crate::ZfpScalarKind::F64 => {
                BrickReaderKind::F64(crate::BrickReader::fixed_rate(bytes, &effective, rate)?)
            }
        };
        Ok(Self { reader, dtype })
    }

    /// Decode the brick at per-axis brick coordinates over the chunk's
    /// **effective** shape ([`NdZfpConfig::effective_shape`]); the
    /// returned bytes are the brick's little-endian samples in C order
    /// plus the brick's (possibly clipped) shape.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for out-of-grid coordinates;
    /// [`Error::Codestream`] when the brick fails to decode.
    pub fn decode_brick(&mut self, brick: &[usize]) -> Result<(Vec<u8>, Vec<usize>)> {
        match &mut self.reader {
            BrickReaderKind::I32(reader) => {
                let (values, brick_shape) = reader.brick(brick)?;
                let bytes = if self.dtype == ZfpDtype::I32 {
                    bytemuck::cast_slice::<i32, u8>(&values).to_vec()
                } else {
                    demoted_bytes(&values, self.dtype)
                };
                Ok((bytes, brick_shape))
            }
            BrickReaderKind::I64(reader) => {
                let (values, brick_shape) = reader.brick(brick)?;
                Ok((
                    bytemuck::cast_slice::<i64, u8>(&values).to_vec(),
                    brick_shape,
                ))
            }
            BrickReaderKind::F32(reader) => {
                let (values, brick_shape) = reader.brick(brick)?;
                Ok((
                    bytemuck::cast_slice::<f32, u8>(&values).to_vec(),
                    brick_shape,
                ))
            }
            BrickReaderKind::F64(reader) => {
                let (values, brick_shape) = reader.brick(brick)?;
                Ok((
                    bytemuck::cast_slice::<f64, u8>(&values).to_vec(),
                    brick_shape,
                ))
            }
        }
    }
}

/// Decode a single fixed-rate brick of an `nd_zfp` chunk at its computed
/// offset. `brick` holds per-axis brick coordinates over the chunk's
/// **effective** shape ([`NdZfpConfig::effective_shape`]); the returned
/// bytes are the brick's little-endian samples in C order plus the brick's
/// (possibly clipped) shape.
///
/// One-shot convenience over [`NdZfpBrickDecoder`]; when reading several
/// bricks of the same chunk, build the decoder once instead.
///
/// # Errors
/// [`Error::InvalidArgument`] unless the configuration is `fixed_rate`, or
/// for geometry violations; [`Error::Codestream`] for a malformed stream.
pub fn decode_chunk_brick(
    bytes: &[u8],
    shape: &[usize],
    dtype: ZfpDtype,
    config: &NdZfpConfig,
    brick: &[usize],
) -> Result<(Vec<u8>, Vec<usize>)> {
    NdZfpBrickDecoder::new(bytes, shape, dtype, config)?.decode_brick(brick)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )] // fixture generators cast freely

    use super::*;

    fn fixed_rate(rate: f64, dims: u8) -> NdZfpConfig {
        NdZfpConfig {
            mode: "fixed_rate".into(),
            rate: Some(rate),
            dims: Some(dims),
            ..NdZfpConfig::default()
        }
    }

    fn reversible(dims: u8) -> NdZfpConfig {
        NdZfpConfig {
            dims: Some(dims),
            ..NdZfpConfig::default()
        }
    }

    fn ramp_bytes(n: usize, dtype: ZfpDtype) -> Vec<u8> {
        (0..n)
            .flat_map(|i| {
                let v = (i % 251) as i64 - 97;
                match dtype {
                    ZfpDtype::U8 => vec![(v.rem_euclid(256)) as u8],
                    ZfpDtype::I8 => vec![((v % 128) as i8) as u8],
                    ZfpDtype::U16 => ((v.rem_euclid(65536)) as u16).to_le_bytes().to_vec(),
                    ZfpDtype::I16 => ((v % 32768) as i16).to_le_bytes().to_vec(),
                    ZfpDtype::I32 => ((v * 1001) as i32).to_le_bytes().to_vec(),
                    ZfpDtype::I64 => (v * 100_000_007).to_le_bytes().to_vec(),
                    ZfpDtype::F32 => ((v as f32) / 3.0).to_le_bytes().to_vec(),
                    ZfpDtype::F64 => ((v as f64) / 3.0).to_le_bytes().to_vec(),
                }
            })
            .collect()
    }

    #[test]
    fn every_dtype_roundtrips_reversibly() {
        let shape = [6, 7, 8];
        let n = 6 * 7 * 8;
        for dtype in [
            ZfpDtype::U8,
            ZfpDtype::I8,
            ZfpDtype::U16,
            ZfpDtype::I16,
            ZfpDtype::I32,
            ZfpDtype::I64,
            ZfpDtype::F32,
            ZfpDtype::F64,
        ] {
            let chunk = ramp_bytes(n, dtype);
            let config = reversible(3);
            let encoded = encode_chunk(&chunk, &shape, dtype, &config).expect("encode");
            let decoded = decode_chunk(&encoded, &shape, dtype, &config).expect("decode");
            assert_eq!(chunk, decoded, "{dtype:?}");
        }
    }

    #[test]
    fn singleton_dimensions_are_squeezed() {
        // The builder's tczyx case: [8, 1, 4, 8, 8] holds 4 non-singleton
        // dims; the transposed singleton axis must not change the stream.
        let chunk = ramp_bytes(8 * 4 * 8 * 8, ZfpDtype::F32);
        let config = reversible(4);
        let with_singleton =
            encode_chunk(&chunk, &[8, 1, 4, 8, 8], ZfpDtype::F32, &config).expect("encode");
        let squeezed = encode_chunk(&chunk, &[8, 4, 8, 8], ZfpDtype::F32, &config).expect("encode");
        assert_eq!(with_singleton, squeezed);
        let decoded = decode_chunk(&with_singleton, &[8, 1, 4, 8, 8], ZfpDtype::F32, &config)
            .expect("decode");
        assert_eq!(chunk, decoded);
    }

    #[test]
    fn fewer_non_singleton_dims_than_declared_still_encode() {
        // A [64] chunk under dims=2 pads to [1, 64].
        let chunk = ramp_bytes(64, ZfpDtype::F32);
        let config = fixed_rate(8.0, 2);
        let encoded = encode_chunk(&chunk, &[64], ZfpDtype::F32, &config).expect("encode");
        let decoded = decode_chunk(&encoded, &[64], ZfpDtype::F32, &config).expect("decode");
        assert_eq!(chunk.len(), decoded.len());
    }

    #[test]
    fn too_many_non_singleton_dims_are_refused() {
        let chunk = ramp_bytes(2 * 3 * 4 * 5 * 6, ZfpDtype::F32);
        let config = reversible(4);
        assert!(encode_chunk(&chunk, &[2, 3, 4, 5, 6], ZfpDtype::F32, &config).is_err());
    }

    #[test]
    fn wrong_byte_count_is_refused() {
        let config = reversible(2);
        let chunk = ramp_bytes(16, ZfpDtype::F32);
        assert!(encode_chunk(&chunk[..60], &[4, 4], ZfpDtype::F32, &config).is_err());
        assert!(encode_chunk(&chunk, &[4, 5], ZfpDtype::F32, &config).is_err());
        assert!(encode_chunk(&chunk, &[4, 0], ZfpDtype::F32, &config).is_err());
    }

    #[test]
    fn configuration_violations_are_refused() {
        let base = NdZfpConfig::default();
        assert!(base.validate().is_ok());
        for bad in [
            NdZfpConfig {
                mode: "zstd".into(),
                ..base.clone()
            },
            NdZfpConfig {
                mode: "fixed_rate".into(),
                ..base.clone()
            },
            NdZfpConfig {
                rate: Some(8.0),
                ..base.clone()
            },
            NdZfpConfig {
                mode: "fixed_rate".into(),
                rate: Some(8.0),
                tolerance: Some(0.1),
                ..base.clone()
            },
            NdZfpConfig {
                mode: "fixed_accuracy".into(),
                ..base.clone()
            },
            NdZfpConfig {
                mode: "fixed_precision".into(),
                precision: Some(65),
                ..base.clone()
            },
            NdZfpConfig {
                dims: Some(0),
                ..base.clone()
            },
            NdZfpConfig {
                dims: Some(5),
                ..base.clone()
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn dtype_names_parse() {
        assert_eq!(ZfpDtype::from_zarr_name("float32"), Some(ZfpDtype::F32));
        assert_eq!(ZfpDtype::from_zarr_name("<f8"), Some(ZfpDtype::F64));
        assert_eq!(ZfpDtype::from_zarr_name("int16"), Some(ZfpDtype::I16));
        assert_eq!(ZfpDtype::from_zarr_name("|u1"), Some(ZfpDtype::U8));
        assert_eq!(ZfpDtype::from_zarr_name("uint32"), None);
        assert_eq!(ZfpDtype::from_zarr_name("float16"), None);
    }

    #[cfg(feature = "serde")]
    mod serde_tests {
        use super::*;

        #[test]
        fn configuration_serializes_in_builder_field_order() {
            let config = fixed_rate(8.0, 3);
            let json = serde_json::to_string(&config).expect("serialize");
            assert_eq!(json, r#"{"mode":"fixed_rate","rate":8.0,"dims":3}"#);
            let back: NdZfpConfig = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(config, back);
        }

        #[test]
        fn defaults_fill_missing_fields_and_unknown_fields_are_refused() {
            let config: NdZfpConfig = serde_json::from_str("{}").expect("deserialize");
            assert_eq!(config, NdZfpConfig::default());
            assert!(serde_json::from_str::<NdZfpConfig>(r#"{"level": 5}"#).is_err());
        }
    }
}
