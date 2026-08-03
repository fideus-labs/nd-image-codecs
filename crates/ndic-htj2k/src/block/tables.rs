//! Lookup-table access for the HT block coder.
//!
//! The static tables live in the generated [`super::tables_data`] module; this
//! module adds the encoder-side UVLC table (T.814 Table 3, algorithmic) and a
//! test that re-derives every generated table from the raw T.814 rows,
//! guarding against generator drift.

pub use super::tables_data::{
    DEC_VLC_TBL0, DEC_VLC_TBL1, ENC_VLC_TBL0, ENC_VLC_TBL1, UVLC_BIAS, UVLC_TBL0, UVLC_TBL1,
};

/// One encoder-side UVLC code: prefix/suffix codewords and lengths for a
/// `u` value (T.814 Table 3; `u_ext` is a 64-bit-path feature we omit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UvlcCode {
    /// Prefix codeword (LSB-first).
    pub prefix: u8,
    /// Prefix length in bits.
    pub prefix_len: u8,
    /// Suffix codeword.
    pub suffix: u8,
    /// Suffix length in bits.
    pub suffix_len: u8,
}

const fn uvlc(prefix: u8, prefix_len: u8, suffix: u8, suffix_len: u8) -> UvlcCode {
    UvlcCode {
        prefix,
        prefix_len,
        suffix,
        suffix_len,
    }
}

/// Encoder UVLC codes for `u` in `0..=32` (the 32-bit datapath bound).
///
/// `u = 0` emits nothing; `1 -> "1"`, `2 -> "01"`, `3..=4 -> "001" + 1 bit`,
/// `5..=32 -> "000" + 5 bits`. Codewords are LSB-first as the VLC stream is.
pub static ENC_UVLC: [UvlcCode; 33] = {
    let mut t = [uvlc(0, 0, 0, 0); 33];
    t[1] = uvlc(1, 1, 0, 0);
    t[2] = uvlc(2, 2, 0, 0);
    t[3] = uvlc(4, 3, 0, 1);
    t[4] = uvlc(4, 3, 1, 1);
    let mut i = 5;
    while i < 33 {
        #[allow(clippy::cast_possible_truncation)]
        {
            t[i] = uvlc(0, 3, (i - 5) as u8, 5);
        }
        i += 1;
    }
    t
};

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)] // index/bit-field math on small values
mod tests {
    use super::super::tables_data::{RAW_TBL0, RAW_TBL1, VlcSrcEntry};
    use super::*;

    /// Port of `ojph_block_common.cpp::vlc_init_tables` (decoder direction).
    fn derive_dec(src: &[VlcSrcEntry]) -> Vec<u16> {
        let mut tbl = vec![0u16; 1024];
        for (i, slot) in tbl.iter_mut().enumerate() {
            let cwd = (i & 0x7F) as u8;
            let c_q = (i >> 7) as u8;
            for e in src {
                if e.c_q == c_q && e.cwd == (cwd & ((1u16 << e.cwd_len) - 1) as u8) {
                    *slot = (u16::from(e.rho) << 4)
                        | (u16::from(e.u_off) << 3)
                        | (u16::from(e.e_k) << 12)
                        | (u16::from(e.e_1) << 8)
                        | u16::from(e.cwd_len);
                }
            }
        }
        tbl
    }

    /// Port of `ojph_block_encoder.cpp::vlc_init_tables` (encoder direction).
    fn derive_enc(src: &[VlcSrcEntry]) -> Vec<u16> {
        let mut tbl = vec![0u16; 2048];
        for (i, slot) in tbl.iter_mut().enumerate() {
            let c_q = (i >> 8) as u8;
            let rho = ((i >> 4) & 0xF) as u8;
            let emb = (i & 0xF) as u8;
            if (emb & rho) != emb || (rho == 0 && c_q == 0) {
                continue;
            }
            let mut best: Option<&VlcSrcEntry> = None;
            if emb != 0 {
                let mut best_ones = -1i32;
                for e in src {
                    if e.c_q == c_q
                        && e.rho == rho
                        && e.u_off == 1
                        && (emb & e.e_k) == e.e_1
                        && i32::from(e.e_k.count_ones() as u8) >= best_ones
                    {
                        best = Some(e);
                        best_ones = i32::from(e.e_k.count_ones() as u8);
                    }
                }
            } else {
                best = src
                    .iter()
                    .find(|e| e.c_q == c_q && e.rho == rho && e.u_off == 0);
            }
            let b = best.expect("table entry must exist");
            *slot = (u16::from(b.cwd) << 8) | (u16::from(b.cwd_len) << 4) | u16::from(b.e_k);
        }
        tbl
    }

    #[test]
    fn derivations_match_static_tables() {
        assert_eq!(derive_dec(&RAW_TBL0), DEC_VLC_TBL0.as_slice());
        assert_eq!(derive_dec(&RAW_TBL1), DEC_VLC_TBL1.as_slice());
        assert_eq!(derive_enc(&RAW_TBL0), ENC_VLC_TBL0.as_slice());
        assert_eq!(derive_enc(&RAW_TBL1), ENC_VLC_TBL1.as_slice());
    }

    #[test]
    fn uvlc_decoder_tables_are_consistent_with_encoder_codes() {
        // For every u0 in 1..=32 (single-quad mode 1: only u_off0 set), the
        // decoder table must reproduce u0 from the encoder's bits.
        for u0 in 1u16..=32 {
            let code = ENC_UVLC[u0 as usize];
            let bits = u32::from(code.prefix) | (u32::from(code.suffix) << code.prefix_len);
            let entry = UVLC_TBL1[(64 + (bits & 0x3F)) as usize];
            let total_prefix = entry & 0x7;
            let u0_suffix_len = (entry >> 7) & 0x7;
            let u0_pfx = (entry >> 10) & 0x7;
            assert_eq!(total_prefix, u16::from(code.prefix_len));
            assert_eq!(u0_suffix_len, u16::from(code.suffix_len));
            let suffix = (bits >> total_prefix) & ((1 << u0_suffix_len) - 1);
            assert_eq!(u0_pfx + suffix as u16, u0, "u0 = {u0}");
        }
    }

    #[test]
    fn dec_vlc_entries_have_sane_lengths() {
        for t in DEC_VLC_TBL0.iter().chain(DEC_VLC_TBL1.iter()) {
            if *t != 0 {
                assert!(t & 0x7 >= 1, "valid codewords are 1..=7 bits: {t:#06x}");
            }
        }
    }
}
