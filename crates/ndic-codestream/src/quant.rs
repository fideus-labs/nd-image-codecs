//! Quantization / ranging parameters: the `QCD`/`QCC` payload (T.800 §A.6.4)
//! and the per-subband `K_max` magnitude bound the HT coder needs.
//!
//! The reversible path stores per-subband exponents derived from BIBO gain
//! bounds of the 5/3 kernel, exactly as `OpenJPH`'s `param_qcd` does, so our
//! streams range identically.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

/// `ceil(log2(.))` of the squared/mixed 5/3 BIBO gain bounds, precomputed
/// from `OpenJPH`'s `bibo_gains::gain_5x3_{l,h}` float tables (see
/// `scripts/gen-ht-tables.py`'s sibling derivation):
/// `X_LL[nd] = ceil(log2(L[nd]^2))`.
const X_LL: [u32; 34] = [
    0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2,
];
/// `X_LH[d] = ceil(log2(H[d-1] * L[d]))` — for the HL and LH bands.
const X_LH: [u32; 34] = [
    0, 2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3,
    3, 3,
];
/// `X_HH[d] = ceil(log2(H[d-1]^2))`.
const X_HH: [u32; 34] = [
    0, 2, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4, 4,
];

/// Parsed or computed quantization info for one component.
#[derive(Debug, Clone, PartialEq)]
pub struct Quant {
    /// Number of guard bits `G` (`Sqcd >> 5`).
    pub guard_bits: u8,
    /// Quantization style: 0 reversible, 1 scalar derived, 2 scalar expounded.
    pub style: u8,
    /// Reversible: per-subband exponents `eps_b` (`SPqcd >> 3`).
    /// Irreversible: raw 16-bit `(exp << 11) | mantissa` words.
    pub values: Vec<u16>,
}

impl Quant {
    /// Computes reversible ranging for `num_decomps` levels of 5/3 at
    /// `bit_depth` (T.800 §E.1 via the `OpenJPH` BIBO bounds).
    ///
    /// # Errors
    /// [`Error::InvalidArgument`] if the combination needs more than 38 bits.
    pub fn reversible(num_decomps: u8, bit_depth: u8, rct: bool) -> Result<Self> {
        let b = u32::from(bit_depth) + u32::from(rct);
        let nd = num_decomps as usize;
        if nd >= X_LL.len() {
            return Err(Error::InvalidArgument {
                message: "more than 33 decomposition levels".into(),
            });
        }
        let mut raw = Vec::with_capacity(1 + 3 * nd);
        // With no decomposition the LL band IS the level-shifted samples,
        // whose magnitude reaches 2^(B-1) *inclusive*; one extra ranging bit
        // covers it. (OpenJPH omits this and corrupts full-range minima at
        // zero decompositions; declaring a larger exponent is conformant.)
        raw.push(b + if nd == 0 { 1 } else { X_LL[nd] });
        for d in (1..=nd).rev() {
            raw.push(b + X_LH[d]);
            raw.push(b + X_LH[d]);
            raw.push(b + X_HH[d]);
        }
        let max_raw = raw.iter().copied().max().unwrap_or(b);
        if max_raw > 38 {
            return Err(Error::InvalidArgument {
                message: "bit depth + wavelet gain exceeds the 38-bit J2K bound".into(),
            });
        }
        let guard_bits = max_raw.saturating_sub(31).max(1);
        #[allow(clippy::cast_possible_truncation)]
        let values = raw.iter().map(|&r| (r - guard_bits) as u16).collect();
        #[allow(clippy::cast_possible_truncation)]
        Ok(Self {
            guard_bits: guard_bits as u8,
            style: 0,
            values,
        })
    }

    /// `K_max` for the subband `band` (0 = LL; 1..=3 = HL/LH/HH) of
    /// resolution `res` — the magnitude-bit bound the block coder uses.
    #[must_use]
    pub fn k_max(&self, res: u8, band: u8) -> u8 {
        let idx = if res == 0 {
            0
        } else {
            (usize::from(res) - 1) * 3 + usize::from(band)
        };
        let idx = idx.min(self.values.len().saturating_sub(1));
        let num_bits = if self.style == 0 {
            let eps = self.values[idx];
            if eps == 0 { 0 } else { u32::from(eps) - 1 }
        } else {
            u32::from(self.values[idx] >> 11).saturating_sub(1)
        };
        #[allow(clippy::cast_possible_truncation)]
        {
            (num_bits + u32::from(self.guard_bits)) as u8
        }
    }

    /// The `MAGB` bound signaled in `Ccap15` (max over subbands).
    #[must_use]
    pub fn magb(&self) -> u32 {
        let mut b = 0u32;
        for (idx, &v) in self.values.iter().enumerate() {
            let t = if self.style == 0 {
                (u32::from(v) + u32::from(self.guard_bits)).saturating_sub(1)
            } else {
                let num_decomps = (self.values.len() - 1) / 3;
                let nb = num_decomps - if idx > 0 { (idx - 1) / 3 } else { 0 };
                let nb = u32::try_from(nb).unwrap_or(u32::MAX);
                (u32::from(v >> 11) + u32::from(self.guard_bits)).saturating_sub(nb)
            };
            b = b.max(t);
        }
        b
    }

    /// Serializes the `QCD` payload (after `Lqcd`).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 2 * self.values.len());
        out.push((self.guard_bits << 5) | self.style);
        for &v in &self.values {
            if self.style == 0 {
                #[allow(clippy::cast_possible_truncation)]
                out.push((v << 3) as u8);
            } else {
                out.extend_from_slice(&v.to_be_bytes());
            }
        }
        out
    }

    /// Parses a `QCD`/`QCC` payload (the bytes after the length field and
    /// any component index).
    ///
    /// # Errors
    /// [`Error::Codestream`] on malformed payloads.
    pub fn parse(payload: &[u8], offset: usize) -> Result<Self> {
        let Some((&sqcd, rest)) = payload.split_first() else {
            return Err(Error::Codestream {
                offset,
                message: "empty QCD payload".into(),
            });
        };
        let style = sqcd & 0x1F;
        let guard_bits = sqcd >> 5;
        let values = match style {
            0 => rest.iter().map(|&b| u16::from(b >> 3)).collect(),
            2 => {
                if rest.len() % 2 != 0 {
                    return Err(Error::Codestream {
                        offset,
                        message: "odd-length expounded QCD payload".into(),
                    });
                }
                rest.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_be_bytes(c))
                    .collect()
            }
            _ => {
                return Err(Error::Codestream {
                    offset,
                    message: "unsupported quantization style (derived)".into(),
                });
            }
        };
        let quant = Self {
            guard_bits,
            style,
            values,
        };
        if quant.values.is_empty() {
            return Err(Error::Codestream {
                offset,
                message: "QCD with no subband step sizes".into(),
            });
        }
        // Every accessor derives shift amounts from K_max; bound it to the
        // 31-bit magnitude datapath here so no consumer can underflow.
        let max_band = quant.values.len().div_ceil(3);
        for r in 0..=u8::try_from(max_band).unwrap_or(u8::MAX) {
            for b in u8::from(r > 0)..=if r > 0 { 3 } else { 0 } {
                if quant.k_max(r, b) > 31 {
                    return Err(Error::Codestream {
                        offset,
                        message: "QCD K_max exceeds the 31-bit magnitude datapath".into(),
                    });
                }
            }
        }
        Ok(quant)
    }

    /// Irreversible step size for (`res`, `band`), including the band gain
    /// (T.800 eq. E-3), for the 9/7 path.
    #[must_use]
    pub fn irrev_delta(&self, res: u8, band: u8) -> f32 {
        let gains = [1.0f32, 2.0, 2.0, 4.0];
        let idx = if res == 0 {
            0
        } else {
            (usize::from(res) - 1) * 3 + usize::from(band)
        };
        let idx = idx.min(self.values.len().saturating_sub(1));
        let v = self.values[idx];
        let eps = v >> 11;
        let mantissa = f32::from((v & 0x7FF) | 0x800) * gains[usize::from(band.min(3))]
            / f32::from(1u16 << 11);
        // 2^eps as an exact f32 via its bit pattern.
        mantissa / f32::from_bits((127 + u32::from(eps)) << 23)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_decomp_ranging_covers_the_full_signed_range() {
        // ojph's verified dump has Sqcd 0x20 / SPqcd 0x38 (eps 7) here, but
        // that cannot represent the level-shifted minimum -128; we declare
        // eps 8 (K_max 8) instead.
        let q = Quant::reversible(0, 8, false).unwrap();
        assert_eq!(q.guard_bits, 1);
        assert_eq!(q.values, alloc::vec![8]);
        assert_eq!(q.to_bytes(), alloc::vec![0x20, 0x40]);
        assert_eq!(q.k_max(0, 0), 8);
        assert_eq!(q.magb(), 8);
    }

    #[test]
    fn ranging_grows_with_levels() {
        let q = Quant::reversible(5, 16, false).unwrap();
        assert_eq!(q.values.len(), 16);
        // First triple after LL is the coarsest level (d = 5): HH X = 3.
        assert_eq!(q.values[3] + u16::from(q.guard_bits), 16 + 3);
        // Last triple is the finest (d = 1): HH uses H[0]^2 = 4 -> X = 2.
        assert_eq!(q.values[15] + u16::from(q.guard_bits), 16 + 2);
        let parsed = Quant::parse(&q.to_bytes(), 0).unwrap();
        assert_eq!(parsed, q);
    }

    #[test]
    fn rejects_over_38_bits() {
        assert!(Quant::reversible(5, 36, false).is_err());
    }
}
