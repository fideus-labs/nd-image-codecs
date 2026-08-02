//! The HT (FBCOT) block coder: cleanup, `SigProp` and `MagRef` passes.
//!
//! The coder operates on **sign-magnitude** `u32` samples: bit 31 carries the
//! sign, bits 30..0 the magnitude shifted so that the cleanup pass's least
//! significant bitplane sits at bit `p = 30 - missing_msbs`. This mirrors the
//! `OpenJPH` datapath, keeping the port bit-compatible; the helpers
//! [`coeff_to_sign_magnitude`] and [`sign_magnitude_to_coeff`] convert to and
//! from two's-complement wavelet coefficients.
//!
//! See `docs/architecture/ht-block-coder.md` and T.814 §6-7.

mod decode;
mod encode;
pub mod mel;
pub mod streams;
pub mod tables;
#[allow(clippy::doc_markdown)] // generated file
mod tables_data;

pub use decode::decode_block;
pub use encode::encode_block;

/// Coding-pass structure of one code-block's codeword segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockPasses {
    /// Number of coding passes: 1 (cleanup), 2 (+`SigProp`), 3 (+`MagRef`).
    pub num_passes: u32,
    /// Byte length of the cleanup pass (`Lcup`).
    pub len_cleanup: usize,
    /// Combined byte length of the refinement passes (`SigProp` + `MagRef`).
    pub len_refinement: usize,
}

impl BlockPasses {
    /// A cleanup-only segment, as our encoder produces.
    #[must_use]
    pub const fn cleanup_only(len: usize) -> Self {
        Self {
            num_passes: 1,
            len_cleanup: len,
            len_refinement: 0,
        }
    }
}

/// Converts a two's-complement coefficient to the coder's sign-magnitude
/// form with the magnitude shifted up by `shift = 31 - K_max` (T.800 §E.1
/// via the `OpenJPH` datapath).
#[inline]
#[must_use]
pub fn coeff_to_sign_magnitude(v: i32, shift: u32) -> u32 {
    let sign = if v < 0 { 0x8000_0000 } else { 0 };
    sign | (v.unsigned_abs() << shift)
}

/// Inverse of [`coeff_to_sign_magnitude`]: drops the decoder's bin-center
/// bits below `shift` and re-applies the sign.
#[inline]
#[must_use]
#[allow(clippy::cast_possible_wrap)]
pub fn sign_magnitude_to_coeff(v: u32, shift: u32) -> i32 {
    let mag = ((v & 0x7FFF_FFFF) >> shift) as i32;
    if v & 0x8000_0000 != 0 { -mag } else { mag }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_magnitude_roundtrip() {
        for shift in [0u32, 5, 14] {
            for v in [-1000i32, -1, 0, 1, 7, 65535] {
                let sm = coeff_to_sign_magnitude(v, shift);
                assert_eq!(sign_magnitude_to_coeff(sm, shift), v, "v={v} shift={shift}");
            }
        }
    }
}
