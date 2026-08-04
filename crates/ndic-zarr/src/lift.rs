//! The `nd_lift` chunk transform core: element bytes ⇄ coefficient-plane
//! bytes.
//!
//! Feature-free on purpose: the `zarrs` codec (`lift_codec`) and the WASM
//! (TypeScript) binding both call these functions, so every ecosystem
//! produces byte-identical coefficient planes. Elements and coefficients are
//! little-endian, C order — the same convention as [`crate::htj2k`].
//!
//! On encode the chunk is widened into its coefficient plane (`int32` for
//! input data types of at most 32 bits, `int64` for 64-bit input) and
//! decorrelated by [`ndic_lift::forward`]; decode runs the inverse transform
//! and narrows back, refusing coefficients outside the element type's range.

use ndic_core::{Error, Result};
use ndic_lift::{NdLiftConfig, PlaneSample};

/// Element data types the `nd_lift` codec transforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftDtype {
    /// `uint8` → `int32` plane.
    U8,
    /// `int8` → `int32` plane.
    I8,
    /// `uint16` → `int32` plane.
    U16,
    /// `int16` → `int32` plane.
    I16,
    /// `uint32` → `int32` plane (values above `i32::MAX` are refused).
    U32,
    /// `int32` → `int32` plane (identity width).
    I32,
    /// `uint64` → `int64` plane (values above `i64::MAX` are refused).
    U64,
    /// `int64` → `int64` plane (identity width).
    I64,
}

impl LiftDtype {
    /// Parse a Zarr v3 data type name or a NumPy-style descr code.
    pub fn from_zarr_name(name: &str) -> Option<Self> {
        Some(match name {
            "uint8" | "|u1" | "u1" => Self::U8,
            "int8" | "|i1" | "i1" => Self::I8,
            "uint16" | "<u2" | "u2" => Self::U16,
            "int16" | "<i2" | "i2" => Self::I16,
            "uint32" | "<u4" | "u4" => Self::U32,
            "int32" | "<i4" | "i4" => Self::I32,
            "uint64" | "<u8" | "u8" => Self::U64,
            "int64" | "<i8" | "i8" => Self::I64,
            _ => return None,
        })
    }

    /// The element size in bytes.
    pub fn size_bytes(self) -> usize {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::U16 | Self::I16 => 2,
            Self::U32 | Self::I32 => 4,
            Self::U64 | Self::I64 => 8,
        }
    }

    /// The widened coefficient's size in bytes (`int32` or `int64`).
    pub fn plane_size_bytes(self) -> usize {
        match self {
            Self::U64 | Self::I64 => 8,
            _ => 4,
        }
    }
}

/// Conversion between a little-endian element type and its widened
/// coefficient plane.
///
/// The only value checks that exist — `u32`/`u64` widening into the signed
/// plane, and every narrow on decode — run as min/max reductions before a
/// bulk cast, never as a branch per element.
trait PlaneConvert<P>: Sized {
    /// Read little-endian samples from `bytes`, widened into the plane.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when a sample does not fit the plane
    /// (unsigned input at or above the plane's sign bit) — the
    /// overflow-budget refusal.
    fn widen_le(bytes: &[u8]) -> Result<Vec<P>>;

    /// Narrow the plane back to samples, as little-endian bytes.
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] when a coefficient is outside the element
    /// type's range (a corrupt or mismatched chunk).
    fn narrow_le(plane: &[P]) -> Result<Vec<u8>>;
}

fn narrow_error(lo: i128, hi: i128, dtype: &str) -> Error {
    Error::InvalidArgument {
        message: format!(
            "nd_lift decode: coefficient range [{lo}, {hi}] does not narrow back to {dtype} \
             (corrupt or mismatched chunk)"
        ),
    }
}

/// Little-endian byte I/O for the plane types (`to_le_bytes` widths differ,
/// so the generic code goes through this trait).
trait PlaneLe: Copy {
    /// Append the value's little-endian bytes to `out`.
    fn extend_le(self, out: &mut Vec<u8>);
    /// Read one value from a little-endian slice of the type's width.
    fn from_le_slice(bytes: &[u8]) -> Self;
}

macro_rules! plane_le {
    ($p:ty) => {
        impl PlaneLe for $p {
            fn extend_le(self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
            fn from_le_slice(bytes: &[u8]) -> Self {
                Self::from_le_bytes(bytes.try_into().unwrap())
            }
        }
    };
}
plane_le!(i32);
plane_le!(i64);

/// Emit a plane slice as little-endian bytes.
fn plane_bytes_le<P: PlaneLe>(plane: &[P]) -> Vec<u8> {
    let mut out = Vec::with_capacity(size_of_val(plane));
    for &v in plane {
        v.extend_le(&mut out);
    }
    out
}

/// Element types strictly narrower than the plane: widening cannot fail;
/// narrowing range-checks via one min/max scan, then bulk-casts.
macro_rules! plane_convert_narrower {
    ($in:ty => $p:ty) => {
        impl PlaneConvert<$p> for $in {
            fn widen_le(bytes: &[u8]) -> Result<Vec<$p>> {
                Ok(bytes
                    .chunks_exact(size_of::<$in>())
                    .map(|c| <$p>::from(<$in>::from_le_bytes(c.try_into().unwrap())))
                    .collect())
            }
            fn narrow_le(plane: &[$p]) -> Result<Vec<u8>> {
                const LO: $p = <$in>::MIN as $p;
                const HI: $p = <$in>::MAX as $p;
                let lo = plane.iter().copied().min().unwrap_or(LO);
                let hi = plane.iter().copied().max().unwrap_or(HI);
                if lo < LO || hi > HI {
                    return Err(narrow_error(lo.into(), hi.into(), stringify!($in)));
                }
                // Range-checked above, so the truncating cast is exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let out = plane
                    .iter()
                    .flat_map(|&v| (v as $in).to_le_bytes())
                    .collect();
                Ok(out)
            }
        }
    };
}

/// The identity pairs (`i32 → i32`, `i64 → i64`).
macro_rules! plane_convert_identity {
    ($t:ty) => {
        impl PlaneConvert<$t> for $t {
            fn widen_le(bytes: &[u8]) -> Result<Vec<$t>> {
                Ok(bytes
                    .chunks_exact(size_of::<$t>())
                    .map(|c| <$t>::from_le_bytes(c.try_into().unwrap()))
                    .collect())
            }
            fn narrow_le(plane: &[$t]) -> Result<Vec<u8>> {
                Ok(plane_bytes_le(plane))
            }
        }
    };
}

/// Unsigned element types as wide as the plane (`u32 → i32`, `u64 → i64`):
/// both directions range-check via one min/max scan, then bulk-cast.
macro_rules! plane_convert_unsigned_full_width {
    ($in:ty => $p:ty) => {
        impl PlaneConvert<$p> for $in {
            fn widen_le(bytes: &[u8]) -> Result<Vec<$p>> {
                #[allow(clippy::cast_sign_loss)]
                const PLANE_MAX: $in = <$p>::MAX as $in;
                let decode = |c: &[u8]| <$in>::from_le_bytes(c.try_into().unwrap());
                let max = bytes
                    .chunks_exact(size_of::<$in>())
                    .map(decode)
                    .max()
                    .unwrap_or(0);
                if max > PLANE_MAX {
                    return Err(Error::InvalidArgument {
                        message: format!(
                            "nd_lift overflow budget: input value {max} does not fit the widened \
                             {} coefficient plane",
                            stringify!($p),
                        ),
                    });
                }
                // Range-checked above, so the wrapping cast is exact.
                #[allow(clippy::cast_possible_wrap)]
                let out = bytes
                    .chunks_exact(size_of::<$in>())
                    .map(|c| decode(c) as $p)
                    .collect();
                Ok(out)
            }
            fn narrow_le(plane: &[$p]) -> Result<Vec<u8>> {
                let lo = plane.iter().copied().min().unwrap_or(0);
                if lo < 0 {
                    let hi = plane.iter().copied().max().unwrap_or(0);
                    return Err(narrow_error(lo.into(), hi.into(), stringify!($in)));
                }
                // Non-negative per the check above, so the cast is exact.
                #[allow(clippy::cast_sign_loss)]
                let out = plane
                    .iter()
                    .flat_map(|&v| (v as $in).to_le_bytes())
                    .collect();
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

/// Widen `bytes` as `In` elements to the plane type `P`, transform, and emit
/// the plane's little-endian bytes (or the reverse).
fn transform_bytes<In, P>(
    bytes: &[u8],
    shape: &[usize],
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>>
where
    In: PlaneConvert<P>,
    P: PlaneSample + PlaneLe,
{
    config.validate(shape.len())?;
    let n: usize = shape.iter().product();
    if forward {
        if bytes.len() != n * size_of::<In>() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "nd_lift encode: got {} bytes for {n} elements of {} bytes",
                    bytes.len(),
                    size_of::<In>()
                ),
            });
        }
        let mut plane = In::widen_le(bytes)?;
        ndic_lift::forward(&mut plane, shape, &config.transforms)?;
        Ok(plane_bytes_le(&plane))
    } else {
        if bytes.len() != n * size_of::<P>() {
            return Err(Error::InvalidArgument {
                message: format!(
                    "nd_lift decode: got {} bytes for {n} coefficients of {} bytes",
                    bytes.len(),
                    size_of::<P>()
                ),
            });
        }
        let mut plane: Vec<P> = bytes
            .chunks_exact(size_of::<P>())
            .map(|c| P::from_le_slice(c))
            .collect();
        ndic_lift::inverse(&mut plane, shape, &config.transforms)?;
        In::narrow_le(&plane)
    }
}

fn run(
    bytes: &[u8],
    shape: &[usize],
    dtype: LiftDtype,
    config: &NdLiftConfig,
    forward: bool,
) -> Result<Vec<u8>> {
    match dtype {
        LiftDtype::U8 => transform_bytes::<u8, i32>(bytes, shape, config, forward),
        LiftDtype::I8 => transform_bytes::<i8, i32>(bytes, shape, config, forward),
        LiftDtype::U16 => transform_bytes::<u16, i32>(bytes, shape, config, forward),
        LiftDtype::I16 => transform_bytes::<i16, i32>(bytes, shape, config, forward),
        LiftDtype::U32 => transform_bytes::<u32, i32>(bytes, shape, config, forward),
        LiftDtype::I32 => transform_bytes::<i32, i32>(bytes, shape, config, forward),
        LiftDtype::U64 => transform_bytes::<u64, i64>(bytes, shape, config, forward),
        LiftDtype::I64 => transform_bytes::<i64, i64>(bytes, shape, config, forward),
    }
}

/// Encodes a chunk (little-endian `dtype` elements, C order) into its
/// widened, decorrelated coefficient plane (little-endian `int32` or
/// `int64`).
///
/// # Errors
/// [`Error::InvalidArgument`] for a byte/shape mismatch, an invalid
/// configuration, or input exceeding the overflow budget;
/// [`Error::Unsupported`] for an unimplemented configuration version.
pub fn forward_chunk(
    bytes: &[u8],
    shape: &[usize],
    dtype: LiftDtype,
    config: &NdLiftConfig,
) -> Result<Vec<u8>> {
    run(bytes, shape, dtype, config, true)
}

/// Decodes a coefficient plane back to little-endian `dtype` elements in C
/// order, undoing [`forward_chunk`] exactly.
///
/// # Errors
/// [`Error::InvalidArgument`] for a byte/shape mismatch, an invalid
/// configuration, or coefficients that do not narrow back to `dtype`;
/// [`Error::Unsupported`] for an unimplemented configuration version.
pub fn inverse_chunk(
    bytes: &[u8],
    shape: &[usize],
    dtype: LiftDtype,
    config: &NdLiftConfig,
) -> Result<Vec<u8>> {
    run(bytes, shape, dtype, config, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndic_lift::{AxisTransform, LiftKind};

    fn config() -> NdLiftConfig {
        NdLiftConfig::new(vec![AxisTransform {
            axis: "z".into(),
            dimension: 0,
            kind: LiftKind::Lift53,
            levels: 1,
            group: 0,
        }])
    }

    #[test]
    fn round_trips_u16() {
        let shape = [4, 3, 3];
        let values: Vec<u16> = (0..36).map(|i| (i * 7) % 4096).collect();
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let plane = forward_chunk(&bytes, &shape, LiftDtype::U16, &config()).unwrap();
        assert_eq!(plane.len(), 36 * 4, "u16 widens to an int32 plane");
        let back = inverse_chunk(&plane, &shape, LiftDtype::U16, &config()).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn round_trips_u64_in_the_i64_plane() {
        let shape = [4, 2, 2];
        let values: Vec<u64> = (0..16u64).map(|i| i * 3_000_000_000).collect();
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        let plane = forward_chunk(&bytes, &shape, LiftDtype::U64, &config()).unwrap();
        assert_eq!(plane.len(), 16 * 8);
        let back = inverse_chunk(&plane, &shape, LiftDtype::U64, &config()).unwrap();
        assert_eq!(back, bytes);
    }

    #[test]
    fn refuses_unsigned_input_beyond_the_plane() {
        let bytes = u32::MAX.to_le_bytes();
        let err = forward_chunk(&bytes, &[1], LiftDtype::U32, &NdLiftConfig::new(Vec::new()))
            .expect_err("u32::MAX does not fit an int32 plane");
        assert!(err.to_string().contains("overflow budget"), "{err}");
    }

    #[test]
    fn refuses_coefficients_that_do_not_narrow() {
        let plane = 70_000i32.to_le_bytes();
        let err = inverse_chunk(&plane, &[1], LiftDtype::U16, &NdLiftConfig::new(Vec::new()))
            .expect_err("70000 does not narrow back to u16");
        assert!(
            err.to_string().contains("does not narrow back to u16"),
            "{err}"
        );
    }
}
