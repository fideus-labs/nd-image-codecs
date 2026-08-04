//! `ndic-zfp` — ZFP block compression (`nd_zfp`).
//!
//! The [ZFP](https://zfp.llnl.gov) fixed-block compressed-array format for
//! **1D–4D** data, registered as the Zarr v3 **array-to-bytes** codec
//! `nd_zfp`. Target use: GPU volume rendering, random brick access, and
//! predictable memory (fixed-rate mode gives every `4^d` block a fixed bit
//! budget, so brick addresses are computable without an index lookup).
//!
//! The block transform and coder are delegated to the pure-Rust
//! [`zfp-rs`](https://crates.io/crates/zfp-rs) crate rather than a port
//! maintained here; `zfp-rs` produces bit-for-bit identical streams to the
//! LLNL C reference implementation on little-endian targets and verifies
//! that against the upstream test suite's checksums and `zfp-sys` in its
//! own CI.
//!
//! ## Stream format
//!
//! Chunks are standard ZFP streams: the full ZFP header (32-bit magic,
//! 52-bit field metadata, 12- or 64-bit compression mode) followed by the
//! compressed blocks, padded to a 64-bit word boundary. This is the same
//! byte layout `zfp -h`, `zfpy`, and `imagecodecs`' numcodecs ZFP produce,
//! so `nd_zfp` chunks cross-decode with those implementations where modes
//! overlap. Streams are little-endian (the ZFP 64-bit-word default).
//!
//! ## Modes
//!
//! - [`ZfpMode::FixedRate`] — fixed bits per block; random access, bounded
//!   memory (primary mode for GPU bricks).
//! - [`ZfpMode::FixedAccuracy`] — absolute error bound, variable size.
//! - [`ZfpMode::FixedPrecision`] — relative-precision control.
//! - [`ZfpMode::Reversible`] — bit-for-bit lossless.
//!
//! Native sample types: `f32`, `f64`, `i32`, `i64` (see [`ZfpElement`]).
//! The chunk-level API ([`encode_chunk`]/[`decode_chunk`]) additionally
//! promotes `u8`/`i8`/`u16`/`i16` into `i32` per the C library's guidance.
//!
//! ## Brick access
//!
//! In fixed-rate mode block *k* occupies exactly
//! `header_bits + k * bits_per_block` … `+ bits_per_block` bits of the
//! stream: [`BrickIndex`] computes those offsets and
//! [`decompress_brick`] decodes a single `4^d` brick without touching the
//! rest of the payload.

use ndic_core::{Error, Result};
use zfp_rs::{
    ZfpBitStream, ZfpConfig, ZfpDimensionality, ZfpField, ZfpFieldMut, ZfpHeaderMask,
    ZfpScalarType, ZfpStreamAlignment,
};

mod chunk;

pub use chunk::{
    NdZfpBrickDecoder, NdZfpConfig, ZfpDtype, decode_chunk, decode_chunk_brick, encode_chunk,
};

/// The Zarr v3 array-to-bytes codec identifier: the name registered in
/// zarr-extensions, whose streams this codec reads and writes.
pub const CODEC_NAME: &str = "zfp";

/// The deprecated pre-registration codec name, kept as a read alias so
/// stores written before the `zfp` adoption keep decoding. Configurations
/// under this name may carry the legacy `dims` member.
pub const LEGACY_CODEC_NAME: &str = "nd_zfp";

/// Bits in the fixed part of a full ZFP header (32-bit magic + 52-bit field
/// metadata); the mode word follows.
const HEADER_FIXED_BITS: u64 = 32 + 52;

/// Largest mode value that fits the short 12-bit mode word
/// (`ZFP_MODE_SHORT_MAX` in the reference implementation); larger values
/// use the 64-bit long form.
const MODE_SHORT_MAX: u64 = (1u64 << 12) - 2;

/// ZFP compression mode, mirroring the reference implementation's modes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZfpMode {
    /// Fixed bits per value (`rate` × `4^d` bits per block).
    FixedRate(f64),
    /// Absolute error tolerance.
    FixedAccuracy(f64),
    /// Number of uncompressed bit planes encoded per block.
    FixedPrecision(u32),
    /// Bit-for-bit lossless.
    Reversible,
}

impl ZfpMode {
    /// Validates the mode's parameter.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for a non-positive/non-finite rate, a
    /// negative/non-finite tolerance, or a precision outside `1..=64`.
    pub fn validate(self) -> Result<()> {
        match self {
            Self::FixedRate(rate) if !(rate.is_finite() && rate > 0.0) => Err(invalid(format!(
                "nd_zfp: fixed-rate mode needs a positive finite rate, got {rate}"
            ))),
            Self::FixedAccuracy(tol) if !(tol.is_finite() && tol >= 0.0) => Err(invalid(format!(
                "nd_zfp: fixed-accuracy mode needs a non-negative finite tolerance, got {tol}"
            ))),
            Self::FixedPrecision(p) if !(1..=64).contains(&p) => Err(invalid(format!(
                "nd_zfp: fixed-precision mode needs 1..=64 bit planes, got {p}"
            ))),
            _ => Ok(()),
        }
    }

    /// The `zfp-rs` expert-parameter configuration for this mode.
    fn config(self, scalar: ZfpScalarType, dims: ZfpDimensionality) -> ZfpConfig {
        match self {
            Self::FixedRate(rate) => {
                ZfpConfig::fixed_rate(rate, scalar, dims, ZfpStreamAlignment::None)
            }
            Self::FixedAccuracy(tol) => ZfpConfig::fixed_accuracy(tol),
            Self::FixedPrecision(p) => ZfpConfig::fixed_precision(p),
            Self::Reversible => ZfpConfig::reversible(),
        }
    }
}

/// The four scalar types ZFP codes natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZfpScalarKind {
    /// 32-bit signed integer (also the promotion target for narrower ints).
    I32,
    /// 64-bit signed integer.
    I64,
    /// IEEE 754 single precision.
    F32,
    /// IEEE 754 double precision.
    F64,
}

impl ZfpScalarKind {
    fn to_zfp(self) -> ZfpScalarType {
        match self {
            Self::I32 => ZfpScalarType::Int32,
            Self::I64 => ZfpScalarType::Int64,
            Self::F32 => ZfpScalarType::Float,
            Self::F64 => ZfpScalarType::Double,
        }
    }
}

/// Sample types [`compress`]/[`decompress`] accept: `i32`, `i64`, `f32`,
/// `f64` (the types ZFP codes natively).
pub trait ZfpElement: zfp_rs::ZfpScalar {
    /// The scalar-kind tag for this type.
    fn kind() -> ZfpScalarKind;
}

impl ZfpElement for i32 {
    fn kind() -> ZfpScalarKind {
        ZfpScalarKind::I32
    }
}
impl ZfpElement for i64 {
    fn kind() -> ZfpScalarKind {
        ZfpScalarKind::I64
    }
}
impl ZfpElement for f32 {
    fn kind() -> ZfpScalarKind {
        ZfpScalarKind::F32
    }
}
impl ZfpElement for f64 {
    fn kind() -> ZfpScalarKind {
        ZfpScalarKind::F64
    }
}

fn invalid(message: String) -> Error {
    Error::InvalidArgument { message }
}

fn malformed(message: String) -> Error {
    Error::Codestream { offset: 0, message }
}

/// Shape checks shared by every entry point: rank 1–4, no zero extents.
fn validate_shape(shape: &[usize]) -> Result<ZfpDimensionality> {
    if shape.is_empty() || shape.len() > 4 {
        return Err(invalid(format!(
            "nd_zfp: shape must have 1..=4 dimensions, got {}",
            shape.len()
        )));
    }
    if shape.contains(&0) {
        return Err(invalid(format!(
            "nd_zfp: shape {shape:?} has a zero extent"
        )));
    }
    #[allow(clippy::cast_possible_truncation)] // rank checked to 1..=4 above
    Ok(ZfpDimensionality::try_from(shape.len() as u32).expect("rank checked"))
}

/// [`validate_shape`] plus the element-count check against a buffer.
fn validate_shape_len(shape: &[usize], len: usize, what: &str) -> Result<ZfpDimensionality> {
    let dims = validate_shape(shape)?;
    let elements = shape
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| invalid(format!("nd_zfp: shape {shape:?} overflows usize")))?;
    if elements != len {
        return Err(invalid(format!(
            "nd_zfp: shape {shape:?} has {elements} elements but the {what} holds {len}"
        )));
    }
    Ok(dims)
}

/// Row-major shape → ZFP `[nx, ny, nz, nw]` (x fastest), zero-padded.
fn rev_dims(shape: &[usize]) -> [usize; 4] {
    let mut dims = [0usize; 4];
    for (slot, &extent) in dims.iter_mut().zip(shape.iter().rev()) {
        *slot = extent;
    }
    dims
}

fn field_of<'a, T: ZfpElement>(data: &'a [T], shape: &[usize]) -> ZfpField<'a> {
    let d = rev_dims(shape);
    match shape.len() {
        1 => ZfpField::new(data, [d[0]]),
        2 => ZfpField::new(data, [d[0], d[1]]),
        3 => ZfpField::new(data, [d[0], d[1], d[2]]),
        _ => ZfpField::new(data, [d[0], d[1], d[2], d[3]]),
    }
}

fn field_mut_of<'a, T: ZfpElement>(data: &'a mut [T], shape: &[usize]) -> ZfpFieldMut<'a> {
    let d = rev_dims(shape);
    match shape.len() {
        1 => ZfpFieldMut::new(data, [d[0]]),
        2 => ZfpFieldMut::new(data, [d[0], d[1]]),
        3 => ZfpFieldMut::new(data, [d[0], d[1], d[2]]),
        _ => ZfpFieldMut::new(data, [d[0], d[1], d[2], d[3]]),
    }
}

/// Compress a 1D–4D row-major array into a self-describing ZFP stream
/// (full header + payload, word-padded).
///
/// # Errors
/// [`Error::InvalidArgument`] for shape/mode violations or dimensions
/// exceeding the ZFP header metadata bounds.
pub fn compress<T: ZfpElement>(data: &[T], shape: &[usize], mode: ZfpMode) -> Result<Vec<u8>> {
    let dims = validate_shape_len(shape, data.len(), "input")?;
    mode.validate()?;
    let scalar = T::kind().to_zfp();
    let config = mode.config(scalar, dims);
    let capacity = config.maximum_size(scalar, shape);
    if capacity == 0 {
        return Err(invalid(format!(
            "nd_zfp: shape {shape:?} exceeds the ZFP stream size bounds"
        )));
    }
    let field = field_of(data, shape);
    let mut bs = ZfpBitStream::new(capacity);
    if bs.write_header(&config, &field, ZfpHeaderMask::FULL) == 0 {
        return Err(invalid(format!(
            "nd_zfp: shape {shape:?} exceeds the ZFP header metadata bounds"
        )));
    }
    bs.compress(&config, &field)
        .map_err(|e| invalid(format!("nd_zfp compress: {e}")))?;
    Ok(bs.into_vec())
}

/// Decompress a stream produced with the same `shape` and `mode` into a
/// caller-owned buffer.
///
/// # Errors
/// [`Error::InvalidArgument`] for shape/mode violations;
/// [`Error::Codestream`] when the stream's header does not declare exactly
/// this shape, scalar type, and mode, or (fixed-rate) when the stream
/// length is not the computed size.
pub fn decompress<T: ZfpElement>(
    bytes: &[u8],
    shape: &[usize],
    mode: ZfpMode,
    out: &mut [T],
) -> Result<()> {
    let dims = validate_shape_len(shape, out.len(), "output")?;
    mode.validate()?;
    let scalar = T::kind().to_zfp();
    let config = mode.config(scalar, dims);
    let mut bs = padded_stream(bytes, &config, scalar, shape)?;
    let header = read_checked_header(&mut bs, &config, scalar, shape)?;
    if matches!(mode, ZfpMode::FixedRate(_)) {
        let expected = fixed_rate_stream_len(header.bits_read as u64, &config, shape);
        if bytes.len() != expected {
            return Err(malformed(format!(
                "nd_zfp: fixed-rate stream is {} bytes, computed size is {expected}",
                bytes.len()
            )));
        }
    }
    let mut field = field_mut_of(out, shape);
    bs.decompress(&config, &mut field)
        .map_err(|e| malformed(format!("nd_zfp decompress: {e}")))?;
    Ok(())
}

/// Wrap `bytes` in a bitstream padded with zero words up to the
/// configuration's maximum stream size, so a truncated stream can never
/// read out of bounds (it fails the header checks or decodes bounded
/// garbage instead of panicking).
fn padded_stream(
    bytes: &[u8],
    config: &ZfpConfig,
    scalar: ZfpScalarType,
    shape: &[usize],
) -> Result<ZfpBitStream> {
    let max = config.maximum_size(scalar, shape);
    if max == 0 {
        return Err(invalid(format!(
            "nd_zfp: shape {shape:?} exceeds the ZFP stream size bounds"
        )));
    }
    if bytes.len() > max {
        return Err(malformed(format!(
            "nd_zfp: stream is {} bytes but this configuration compresses to at most {max}",
            bytes.len()
        )));
    }
    let nwords = max.div_ceil(8);
    let mut words = Vec::with_capacity(nwords);
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        words.push(u64::from_le_bytes(word));
    }
    words.resize(nwords, 0);
    Ok(ZfpBitStream::from_buffer(words))
}

/// Read the full header and require it to declare exactly the expected
/// scalar type, dimensions, and compression parameters.
fn read_checked_header(
    bs: &mut ZfpBitStream,
    config: &ZfpConfig,
    scalar: ZfpScalarType,
    shape: &[usize],
) -> Result<zfp_rs::ZfpHeader> {
    let header = bs
        .read_header(ZfpHeaderMask::FULL)
        .map_err(|e| malformed(format!("nd_zfp: {e}")))?;
    let meta = header
        .metadata
        .ok_or_else(|| malformed("nd_zfp: stream header carries no field metadata".into()))?;
    if meta.scalar_type != scalar {
        return Err(malformed(format!(
            "nd_zfp: stream holds {} samples, expected {scalar}",
            meta.scalar_type
        )));
    }
    let expected = rev_dims(shape);
    if meta.dims != expected {
        return Err(malformed(format!(
            "nd_zfp: stream declares dimensions {:?}, expected {expected:?} (shape {shape:?})",
            meta.dims
        )));
    }
    let stream_config = header
        .config
        .ok_or_else(|| malformed("nd_zfp: stream header carries no compression mode".into()))?;
    if stream_config != *config {
        return Err(malformed(
            "nd_zfp: stream compression parameters do not match the configuration".into(),
        ));
    }
    Ok(header)
}

/// Exact byte length of a fixed-rate stream: header + one fixed budget per
/// block, padded to a 64-bit word.
fn fixed_rate_stream_len(header_bits: u64, config: &ZfpConfig, shape: &[usize]) -> usize {
    let blocks: u64 = shape.iter().map(|&n| n.div_ceil(4) as u64).product();
    let total_bits = header_bits + blocks * u64::from(config.max_bits());
    usize::try_from(total_bits.next_multiple_of(64) / 8).expect("stream size fits usize")
}

/// Computed brick addressing for **fixed-rate** streams: block *k* spans
/// exactly `header_bits + k * bits_per_brick` … `+ bits_per_brick` bits.
#[derive(Debug, Clone, PartialEq)]
pub struct BrickIndex {
    header_bits: u64,
    bits_per_brick: u64,
    /// Per-axis block counts, row-major (same axis order as the shape).
    grid: Vec<usize>,
    stream_len: usize,
}

impl BrickIndex {
    /// Build the index for a fixed-rate stream over `shape` (row-major)
    /// holding `scalar` samples at `rate` bits per value.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for shape/rate violations.
    pub fn fixed_rate(shape: &[usize], scalar: ZfpScalarKind, rate: f64) -> Result<Self> {
        let dims = validate_shape(shape)?;
        let mode = ZfpMode::FixedRate(rate);
        mode.validate()?;
        let config = mode.config(scalar.to_zfp(), dims);
        let mode_word_bits = if config.mode_bits() <= MODE_SHORT_MAX {
            12
        } else {
            64
        };
        let header_bits = HEADER_FIXED_BITS + mode_word_bits;
        let grid: Vec<usize> = shape.iter().map(|&n| n.div_ceil(4)).collect();
        Ok(Self {
            header_bits,
            bits_per_brick: u64::from(config.max_bits()),
            stream_len: fixed_rate_stream_len(header_bits, &config, shape),
            grid,
        })
    }

    /// Number of bricks in the stream.
    pub fn num_bricks(&self) -> usize {
        self.grid.iter().product()
    }

    /// Per-axis brick counts, row-major (same axis order as the shape).
    pub fn grid(&self) -> &[usize] {
        &self.grid
    }

    /// Size of the stream header in bits.
    pub fn header_bits(&self) -> u64 {
        self.header_bits
    }

    /// Fixed bit budget of every brick.
    pub fn bits_per_brick(&self) -> u64 {
        self.bits_per_brick
    }

    /// Exact byte length of the whole stream.
    pub fn stream_len(&self) -> usize {
        self.stream_len
    }

    /// Linear brick number for per-axis brick coordinates (row-major, the
    /// stream's block order: last axis fastest).
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when `coords` is out of the brick grid.
    pub fn linear(&self, coords: &[usize]) -> Result<usize> {
        if coords.len() != self.grid.len() {
            return Err(invalid(format!(
                "nd_zfp: brick coordinates {coords:?} do not match the {}-axis grid",
                self.grid.len()
            )));
        }
        let mut linear = 0usize;
        for (&c, &g) in coords.iter().zip(&self.grid) {
            if c >= g {
                return Err(invalid(format!(
                    "nd_zfp: brick coordinates {coords:?} exceed the grid {:?}",
                    self.grid
                )));
            }
            linear = linear * g + c;
        }
        Ok(linear)
    }

    /// `(offset, length)` of brick `k` in bits from the stream start.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when `k` is out of range.
    pub fn bit_range(&self, k: usize) -> Result<(u64, u64)> {
        if k >= self.num_bricks() {
            return Err(invalid(format!(
                "nd_zfp: brick {k} out of range ({} bricks)",
                self.num_bricks()
            )));
        }
        Ok((
            self.header_bits + k as u64 * self.bits_per_brick,
            self.bits_per_brick,
        ))
    }

    /// `(offset, length)` of the byte span enclosing brick `k` — the range
    /// a byte-granular reader (HTTP Range, Zarr partial read) fetches.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when `k` is out of range.
    pub fn byte_range(&self, k: usize) -> Result<(u64, u64)> {
        let (bit_offset, bit_len) = self.bit_range(k)?;
        let start = bit_offset / 8;
        let end = (bit_offset + bit_len).div_ceil(8);
        Ok((start, end - start))
    }
}

/// A prepared fixed-rate stream for repeated brick decodes: the stream
/// buffering, size check, and header validation are paid **once**, then
/// every [`BrickReader::brick`] call is a seek plus one block decode —
/// the O(1) access the format promises. Prefer this over
/// [`decompress_brick`] whenever more than one brick of the same chunk is
/// read.
pub struct BrickReader<T: ZfpElement> {
    stream: ZfpBitStream,
    index: BrickIndex,
    config: ZfpConfig,
    shape: Vec<usize>,
    _samples: core::marker::PhantomData<T>,
}

impl<T: ZfpElement> BrickReader<T> {
    /// Prepare a fixed-rate stream over `shape` (row-major) for brick
    /// decoding: validates the exact stream length and the full header.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for shape/rate violations;
    /// [`Error::Codestream`] when the stream fails the header or size
    /// checks of [`decompress`].
    pub fn fixed_rate(bytes: &[u8], shape: &[usize], rate: f64) -> Result<Self> {
        let dims = validate_shape(shape)?;
        let index = BrickIndex::fixed_rate(shape, T::kind(), rate)?;
        if bytes.len() != index.stream_len() {
            return Err(malformed(format!(
                "nd_zfp: fixed-rate stream is {} bytes, computed size is {}",
                bytes.len(),
                index.stream_len()
            )));
        }
        let scalar = T::kind().to_zfp();
        let config = ZfpMode::FixedRate(rate).config(scalar, dims);
        let mut stream = padded_stream(bytes, &config, scalar, shape)?;
        read_checked_header(&mut stream, &config, scalar, shape)?;
        Ok(Self {
            stream,
            index,
            config,
            shape: shape.to_vec(),
            _samples: core::marker::PhantomData,
        })
    }

    /// The stream's computed brick addressing.
    pub fn index(&self) -> &BrickIndex {
        &self.index
    }

    /// Decode the `4^d` brick at per-axis brick coordinates `brick`,
    /// without decoding any other brick. Returns the brick's samples
    /// (row-major) and its shape — edge bricks are clipped to the array
    /// bounds.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] for out-of-grid coordinates;
    /// [`Error::Codestream`] when the brick fails to decode.
    pub fn brick(&mut self, brick: &[usize]) -> Result<(Vec<T>, Vec<usize>)> {
        let k = self.index.linear(brick)?;
        let (bit_offset, _) = self.index.bit_range(k)?;
        self.stream.seek_read(bit_offset);
        let brick_shape: Vec<usize> = self
            .shape
            .iter()
            .zip(brick)
            .map(|(&n, &b)| (n - b * 4).min(4))
            .collect();
        let mut out = vec![T::default(); brick_shape.iter().product()];
        let mut field = field_mut_of(&mut out, &brick_shape);
        self.stream
            .decompress(&self.config, &mut field)
            .map_err(|e| malformed(format!("nd_zfp decompress brick {k}: {e}")))?;
        Ok((out, brick_shape))
    }
}

/// Decode a single `4^d` brick of a fixed-rate stream at its computed
/// offset, without decoding any other brick. Returns the brick's samples
/// (row-major) and its shape — edge bricks are clipped to the array
/// bounds.
///
/// One-shot convenience over [`BrickReader`]; when reading several bricks
/// of the same chunk, build the reader once instead.
///
/// # Errors
/// [`Error::InvalidArgument`] for shape/rate/coordinate violations;
/// [`Error::Codestream`] when the stream fails the header or size checks
/// of [`decompress`].
pub fn decompress_brick<T: ZfpElement>(
    bytes: &[u8],
    shape: &[usize],
    rate: f64,
    brick: &[usize],
) -> Result<(Vec<T>, Vec<usize>)> {
    BrickReader::<T>::fixed_rate(bytes, shape, rate)?.brick(brick)
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

    /// A smooth, mildly noisy field the transform is designed for.
    fn smooth<T: From<i16>>(shape: &[usize]) -> Vec<T> {
        let n: usize = shape.iter().product();
        let mut noise: u32 = 0x9e37_79b9;
        (0..n)
            .map(|i| {
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let idx = i as i64;
                let value = (idx % 97) * 3 - 140 + i64::from(noise % 5);
                T::from(i16::try_from(value).expect("in range"))
            })
            .collect()
    }

    fn roundtrip_reversible<T: ZfpElement + From<i16> + PartialEq + std::fmt::Debug>(
        shape: &[usize],
    ) {
        let data: Vec<T> = smooth(shape);
        let bytes = compress(&data, shape, ZfpMode::Reversible).expect("compress");
        let mut out = vec![T::default(); data.len()];
        decompress(&bytes, shape, ZfpMode::Reversible, &mut out).expect("decompress");
        assert_eq!(data, out, "shape {shape:?}");
    }

    #[test]
    fn reversible_roundtrips_every_type_and_rank() {
        for shape in [
            vec![5],
            vec![4, 4],
            vec![5, 7],
            vec![3, 5, 7],
            vec![4, 4, 4],
            vec![3, 4, 5, 2],
        ] {
            roundtrip_reversible::<f32>(&shape);
            roundtrip_reversible::<f64>(&shape);
            roundtrip_reversible::<i32>(&shape);
            roundtrip_reversible::<i64>(&shape);
        }
    }

    #[test]
    fn fixed_accuracy_bounds_the_error() {
        let shape = [8, 9];
        let data: Vec<f64> = smooth(&shape);
        for tol in [1.0, 0.25, 0.001] {
            let bytes = compress(&data, &shape, ZfpMode::FixedAccuracy(tol)).expect("compress");
            let mut out = vec![0.0f64; data.len()];
            decompress(&bytes, &shape, ZfpMode::FixedAccuracy(tol), &mut out).expect("decompress");
            let worst = data
                .iter()
                .zip(&out)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f64, f64::max);
            assert!(worst <= tol, "tolerance {tol} exceeded: {worst}");
        }
    }

    #[test]
    fn fixed_precision_roundtrips() {
        let shape = [6, 6, 6];
        let data: Vec<f32> = smooth(&shape);
        let bytes = compress(&data, &shape, ZfpMode::FixedPrecision(24)).expect("compress");
        let mut out = vec![0.0f32; data.len()];
        decompress(&bytes, &shape, ZfpMode::FixedPrecision(24), &mut out).expect("decompress");
    }

    #[test]
    fn fixed_rate_stream_has_the_computed_length() {
        for shape in [vec![16, 16], vec![9, 10, 11], vec![4, 4, 4, 4]] {
            let data: Vec<f32> = smooth(&shape);
            let bytes = compress(&data, &shape, ZfpMode::FixedRate(8.0)).expect("compress");
            let index = BrickIndex::fixed_rate(&shape, ZfpScalarKind::F32, 8.0).expect("index");
            assert_eq!(bytes.len(), index.stream_len(), "shape {shape:?}");
        }
    }

    #[test]
    fn every_brick_decodes_to_the_full_decode_slice() {
        let shape = [9, 10, 11];
        let rate = 8.0;
        let data: Vec<f32> = smooth(&shape);
        let bytes = compress(&data, &shape, ZfpMode::FixedRate(rate)).expect("compress");
        let mut full = vec![0.0f32; data.len()];
        decompress(&bytes, &shape, ZfpMode::FixedRate(rate), &mut full).expect("decompress");
        let index = BrickIndex::fixed_rate(&shape, ZfpScalarKind::F32, rate).expect("index");
        assert_eq!(index.grid(), &[3, 3, 3]);
        for bz in 0..3 {
            for by in 0..3 {
                for bx in 0..3 {
                    let coords = [bz, by, bx];
                    let (brick, brick_shape) =
                        decompress_brick::<f32>(&bytes, &shape, rate, &coords).expect("brick");
                    let mut expected = Vec::new();
                    for z in 0..brick_shape[0] {
                        for y in 0..brick_shape[1] {
                            for x in 0..brick_shape[2] {
                                let at =
                                    ((bz * 4 + z) * shape[1] + by * 4 + y) * shape[2] + bx * 4 + x;
                                expected.push(full[at]);
                            }
                        }
                    }
                    assert_eq!(brick, expected, "brick {coords:?}");
                }
            }
        }
    }

    #[test]
    fn brick_reader_matches_one_shot_brick_decodes() {
        // The prepared reader (buffer + header paid once) must decode every
        // brick identically to the one-shot path, in any visit order.
        let shape = [9, 10, 7];
        let rate = 8.0;
        let data: Vec<f32> = smooth(&shape);
        let bytes = compress(&data, &shape, ZfpMode::FixedRate(rate)).expect("compress");
        let mut reader = BrickReader::<f32>::fixed_rate(&bytes, &shape, rate).expect("reader");
        let grid = reader.index().grid().to_vec();
        let mut coords_list = Vec::new();
        for bz in 0..grid[0] {
            for by in 0..grid[1] {
                for bx in 0..grid[2] {
                    coords_list.push([bz, by, bx]);
                }
            }
        }
        coords_list.reverse(); // out-of-stream-order seeks must work too
        for coords in coords_list {
            let (from_reader, shape_r) = reader.brick(&coords).expect("reader brick");
            let (one_shot, shape_o) =
                decompress_brick::<f32>(&bytes, &shape, rate, &coords).expect("one-shot brick");
            assert_eq!(shape_r, shape_o, "brick {coords:?}");
            assert_eq!(from_reader, one_shot, "brick {coords:?}");
        }
    }

    #[test]
    fn brick_byte_ranges_tile_the_payload() {
        let shape = [16, 16];
        let index = BrickIndex::fixed_rate(&shape, ZfpScalarKind::F64, 8.0).expect("index");
        assert_eq!(index.num_bricks(), 16);
        let (first_off, _) = index.bit_range(0).expect("range");
        assert_eq!(first_off, index.header_bits());
        let (last_off, last_len) = index.bit_range(15).expect("range");
        assert_eq!(last_off, index.header_bits() + 15 * index.bits_per_brick());
        assert!((last_off + last_len).div_ceil(8) <= index.stream_len() as u64);
        let (byte_off, byte_len) = index.byte_range(3).expect("range");
        assert!(byte_len >= index.bits_per_brick() / 8);
        assert!(byte_off < index.stream_len() as u64);
    }

    #[test]
    fn malformed_streams_error_cleanly() {
        let shape = [8, 8];
        let data: Vec<f32> = smooth(&shape);
        let good = compress(&data, &shape, ZfpMode::FixedRate(8.0)).expect("compress");
        let mut out = vec![0.0f32; data.len()];
        // Empty, garbage, truncated, oversized: errors, never panics.
        for bad in [
            Vec::new(),
            vec![0xAAu8; good.len()],
            good[..good.len() / 2].to_vec(),
            [good.clone(), vec![0u8; 64]].concat(),
        ] {
            assert!(
                decompress(&bad, &shape, ZfpMode::FixedRate(8.0), &mut out).is_err(),
                "{} bytes accepted",
                bad.len()
            );
        }
        // Wrong expected dtype, shape, or mode: refused via the header.
        let mut out64 = vec![0.0f64; data.len()];
        assert!(decompress(&good, &shape, ZfpMode::FixedRate(8.0), &mut out64).is_err());
        let mut out_small = vec![0.0f32; 32];
        assert!(decompress(&good, &[4, 8], ZfpMode::FixedRate(8.0), &mut out_small).is_err());
        assert!(decompress(&good, &shape, ZfpMode::Reversible, &mut out).is_err());
    }

    #[test]
    fn shape_and_mode_violations_are_refused() {
        let data = vec![0.0f32; 16];
        assert!(compress(&data, &[], ZfpMode::Reversible).is_err());
        assert!(compress(&data, &[2, 2, 2, 2, 1], ZfpMode::Reversible).is_err());
        assert!(compress(&data, &[4, 0], ZfpMode::Reversible).is_err());
        assert!(compress(&data, &[4, 5], ZfpMode::Reversible).is_err());
        assert!(compress(&data, &[4, 4], ZfpMode::FixedRate(0.0)).is_err());
        assert!(compress(&data, &[4, 4], ZfpMode::FixedRate(f64::NAN)).is_err());
        assert!(compress(&data, &[4, 4], ZfpMode::FixedAccuracy(-1.0)).is_err());
        assert!(compress(&data, &[4, 4], ZfpMode::FixedPrecision(0)).is_err());
        assert!(compress(&data, &[4, 4], ZfpMode::FixedPrecision(65)).is_err());
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn shapes() -> impl Strategy<Value = Vec<usize>> {
            prop::collection::vec(1usize..9, 1..=4)
        }

        proptest! {
            #[test]
            fn reversible_f32_roundtrips(shape in shapes(), seed in any::<u64>()) {
                let n: usize = shape.iter().product();
                let mut state = seed | 1;
                let data: Vec<f32> = (0..n).map(|_| {
                    state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    ((state >> 40) as i32 - (1 << 23)) as f32 / 256.0
                }).collect();
                let bytes = compress(&data, &shape, ZfpMode::Reversible).unwrap();
                let mut out = vec![0.0f32; n];
                decompress(&bytes, &shape, ZfpMode::Reversible, &mut out).unwrap();
                prop_assert_eq!(data, out);
            }

            #[test]
            fn reversible_i64_roundtrips(shape in shapes(), seed in any::<u64>()) {
                let n: usize = shape.iter().product();
                let mut state = seed | 1;
                let data: Vec<i64> = (0..n).map(|_| {
                    state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    // Keep within ZFP's reversible i64 range (~2^62).
                    (state >> 3) as i64 - (1 << 60)
                }).collect();
                let bytes = compress(&data, &shape, ZfpMode::Reversible).unwrap();
                let mut out = vec![0i64; n];
                decompress(&bytes, &shape, ZfpMode::Reversible, &mut out).unwrap();
                prop_assert_eq!(data, out);
            }

            #[test]
            fn fixed_rate_bricks_match_full_decode(
                shape in prop::collection::vec(1usize..9, 2..=3),
                seed in any::<u64>(),
            ) {
                let n: usize = shape.iter().product();
                let mut state = seed | 1;
                let data: Vec<f64> = (0..n).map(|_| {
                    state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    f64::from((state >> 40) as i32) / 1024.0
                }).collect();
                let bytes = compress(&data, &shape, ZfpMode::FixedRate(10.0)).unwrap();
                let mut full = vec![0.0f64; n];
                decompress(&bytes, &shape, ZfpMode::FixedRate(10.0), &mut full).unwrap();
                let index = BrickIndex::fixed_rate(&shape, ZfpScalarKind::F64, 10.0).unwrap();
                // Decode the last brick (always exercises clipping when the
                // shape is not a multiple of 4) and compare.
                let coords: Vec<usize> = index.grid().iter().map(|&g| g - 1).collect();
                let (brick, brick_shape) =
                    decompress_brick::<f64>(&bytes, &shape, 10.0, &coords).unwrap();
                let mut expected = Vec::new();
                let starts: Vec<usize> = coords.iter().map(|&c| c * 4).collect();
                if shape.len() == 2 {
                    for y in 0..brick_shape[0] {
                        for x in 0..brick_shape[1] {
                            expected.push(full[(starts[0] + y) * shape[1] + starts[1] + x]);
                        }
                    }
                } else {
                    for z in 0..brick_shape[0] {
                        for y in 0..brick_shape[1] {
                            for x in 0..brick_shape[2] {
                                expected.push(
                                    full[((starts[0] + z) * shape[1] + starts[1] + y) * shape[2]
                                        + starts[2]
                                        + x],
                                );
                            }
                        }
                    }
                }
                prop_assert_eq!(brick, expected);
            }
        }
    }
}
