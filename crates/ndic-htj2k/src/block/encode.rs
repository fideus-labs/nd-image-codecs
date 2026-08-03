//! HT block encoder: a single self-contained cleanup pass (T.814 §6).
//!
//! Ported from `OpenJPH` `ojph_block_encoder.cpp` (`ojph_encode_codeblock32`,
//! BSD-2-Clause). Like `OpenJPH`, the encoder emits **only the cleanup pass**:
//! for lossless coding one cleanup pass down to bitplane `p` carries every
//! magnitude bit, and `SigProp`/`MagRef` exist decoder-side for interop with
//! encoders that use them for rate control.
//!
//! Input samples are **sign-magnitude**: bit 31 the sign, the magnitude
//! shifted so the coding LSB sits at bit `p = 30 - missing_msbs` (see
//! [`super::coeff_to_sign_magnitude`]).

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use ndic_core::{Error, Result};

use super::mel::MelEncoder;
use super::streams::{MagSgnEncoder, VlcEncoder};
use super::tables::{ENC_UVLC, ENC_VLC_TBL0, ENC_VLC_TBL1};

/// Per-quad working state gathered from the sample buffer.
#[derive(Default, Clone, Copy)]
struct Quad {
    /// Significance pattern (bit n = sample n of the quad).
    rho: u32,
    /// Exponents of `2 mu_p - 1` per sample.
    e_q: [u32; 4],
    /// Largest exponent in the quad.
    e_qmax: u32,
    /// `v_n = 2 (mu_p - 1) + sign` per sample.
    s: [u32; 4],
}

impl Quad {
    /// Loads one 2x2 quad from the sign-magnitude buffer; columns or the
    /// bottom row beyond the block read as insignificant.
    fn load(
        buf: &[u32],
        stride: usize,
        width: usize,
        height: usize,
        x: usize,
        y: usize,
        p: u32,
    ) -> Self {
        let mut quad = Self::default();
        for n in 0..4usize {
            let sx = x + n / 2;
            let sy = y + n % 2;
            if sx >= width || sy >= height {
                continue;
            }
            let raw = buf[sy * stride + sx];
            // (raw + raw) drops the sign bit; >> p aligns the coding LSB.
            let mut val = (raw.wrapping_add(raw) >> p) & !1; // 2 mu_p
            if val != 0 {
                quad.rho |= 1 << n;
                val -= 1; // 2 mu_p - 1
                quad.e_q[n] = 32 - val.leading_zeros();
                quad.e_qmax = quad.e_qmax.max(quad.e_q[n]);
                quad.s[n] = (val - 1) + (raw >> 31); // 2 (mu_p - 1) + sign
            }
        }
        quad
    }

    /// EMB pattern: which samples attain the quad's maximum exponent
    /// (communicated only when a `u` value is coded).
    fn eps(&self, u_q: u32) -> u32 {
        if u_q == 0 {
            return 0;
        }
        let mut eps = 0;
        for n in 0..4 {
            eps |= u32::from(self.rho & (1 << n) != 0 && self.e_q[n] == self.e_qmax) << n;
        }
        eps
    }
}

/// Emits the `MagSgn` bits of one quad given its VLC tuple (`e_k` in the
/// low nibble) and exponent bound `u_q_cap`.
fn emit_magsgn(ms: &mut MagSgnEncoder, q: &Quad, tuple: u32, uq: u32) {
    for n in 0..4u32 {
        if q.rho & (1 << n) != 0 {
            let m = uq - ((tuple >> n) & 1);
            let mask = 1u32.checked_shl(m).map_or(u32::MAX, |b| b - 1);
            ms.encode(q.s[n as usize] & mask, m);
        }
    }
}

/// Encodes one code-block's cleanup pass, returning the codeword segment.
///
/// * `buf` — sign-magnitude samples (see the module docs), row-major with
///   `stride`.
/// * `missing_msbs` — the value that will be signaled in the packet header;
///   the coding LSB is `p = 30 - missing_msbs`. Every magnitude must fit
///   below bit 31 after the implied shift.
///
/// The block must contain at least one significant sample at bitplane `p`
/// (callers skip all-zero blocks, as the packet header can signal
/// non-inclusion for free).
///
/// # Errors
/// [`Error::InvalidArgument`] on bad geometry or `missing_msbs` out of the
/// 32-bit datapath's range.
#[allow(clippy::too_many_lines)] // faithful port of one long reference routine
#[allow(clippy::similar_names, clippy::many_single_char_names)] // T.814 names
pub fn encode_block(
    buf: &[u32],
    width: usize,
    height: usize,
    stride: usize,
    missing_msbs: u32,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 || width > 1024 || height > 1024 || width * height > 4096 {
        return Err(Error::InvalidArgument {
            message: "code-block dimensions must be 1..=1024 with area <= 4096".into(),
        });
    }
    if stride < width || buf.len() < (height - 1) * stride + width {
        return Err(Error::InvalidArgument {
            message: "sample buffer too small for code-block geometry".into(),
        });
    }
    if missing_msbs >= 30 {
        return Err(Error::InvalidArgument {
            message: "missing_msbs must be < 30 for the 32-bit datapath".into(),
        });
    }
    let p = 30 - missing_msbs;

    let mut mel = MelEncoder::new();
    let mut vlc = VlcEncoder::new();
    let mut ms = MagSgnEncoder::new();

    // Line buffers for the E and context values a quad row leaves for the
    // next one. One byte per quad column; sized for 512 quads plus slack.
    let cols = width.div_ceil(2);
    let mut e_val = vec![0u8; cols + 4];
    let mut cx_val = vec![0u8; cols + 4];

    // ---- initial row of quads (kappa = 1) -----------------------------
    let mut c_q0: u32 = 0;
    let mut lep = 0usize; // cursor into e_val
    let mut lcxp = 0usize; // cursor into cx_val
    let mut x = 0usize;
    while x < width {
        let q0 = Quad::load(buf, stride, width, height, x, 0, p);
        let uq0 = q0.e_qmax.max(1); // kappa = 1
        let u_q0 = uq0 - 1;
        let mut u_q1 = 0;
        let eps0 = q0.eps(u_q0);

        e_val[lep] = e_val[lep].max(to_u8(q0.e_q[1]));
        lep += 1;
        e_val[lep] = to_u8(q0.e_q[3]);
        cx_val[lcxp] |= to_u8((q0.rho & 2) >> 1);
        lcxp += 1;
        cx_val[lcxp] = to_u8((q0.rho & 8) >> 3);

        let tuple0 = u32::from(ENC_VLC_TBL0[((c_q0 << 8) + (q0.rho << 4) + eps0) as usize]);
        vlc.encode(tuple0 >> 8, (tuple0 >> 4) & 7);
        if c_q0 == 0 {
            mel.encode(q0.rho != 0);
        }
        emit_magsgn(&mut ms, &q0, tuple0, uq0);

        if x + 2 < width {
            let q1 = Quad::load(buf, stride, width, height, x + 2, 0, p);
            let c_q1 = (q0.rho >> 1) | (q0.rho & 1);
            let uq1 = q1.e_qmax.max(1); // kappa = 1
            u_q1 = uq1 - 1;
            let eps1 = q1.eps(u_q1);

            e_val[lep] = e_val[lep].max(to_u8(q1.e_q[1]));
            lep += 1;
            e_val[lep] = to_u8(q1.e_q[3]);
            cx_val[lcxp] |= to_u8((q1.rho & 2) >> 1);
            lcxp += 1;
            cx_val[lcxp] = to_u8((q1.rho & 8) >> 3);

            let tuple1 = u32::from(ENC_VLC_TBL0[((c_q1 << 8) + (q1.rho << 4) + eps1) as usize]);
            vlc.encode(tuple1 >> 8, (tuple1 >> 4) & 7);
            if c_q1 == 0 {
                mel.encode(q1.rho != 0);
            }
            emit_magsgn(&mut ms, &q1, tuple1, uq1);

            c_q0 = (q1.rho >> 1) | (q1.rho & 1);
        } else {
            c_q0 = 0;
        }

        // UVLC for the pair (T.814 §7.3.7, initial rows).
        if u_q0 > 0 && u_q1 > 0 {
            mel.encode(u_q0.min(u_q1) > 2);
        }
        if u_q0 > 2 && u_q1 > 2 {
            let (c0, c1) = (ENC_UVLC[(u_q0 - 2) as usize], ENC_UVLC[(u_q1 - 2) as usize]);
            vlc.encode(c0.prefix.into(), c0.prefix_len.into());
            vlc.encode(c1.prefix.into(), c1.prefix_len.into());
            vlc.encode(c0.suffix.into(), c0.suffix_len.into());
            vlc.encode(c1.suffix.into(), c1.suffix_len.into());
        } else if u_q0 > 2 && u_q1 > 0 {
            let c0 = ENC_UVLC[u_q0 as usize];
            vlc.encode(c0.prefix.into(), c0.prefix_len.into());
            vlc.encode(u_q1 - 1, 1);
            vlc.encode(c0.suffix.into(), c0.suffix_len.into());
        } else {
            let (c0, c1) = (ENC_UVLC[u_q0 as usize], ENC_UVLC[u_q1 as usize]);
            vlc.encode(c0.prefix.into(), c0.prefix_len.into());
            vlc.encode(c1.prefix.into(), c1.prefix_len.into());
            vlc.encode(c0.suffix.into(), c0.suffix_len.into());
            vlc.encode(c1.suffix.into(), c1.suffix_len.into());
        }

        x += 4;
    }
    e_val[lep + 1] = 0;

    // ---- non-initial rows of quads ------------------------------------
    let mut y = 2usize;
    while y < height {
        lep = 0;
        let mut max_e = i32::from(e_val[0].max(e_val[1])) - 1;
        e_val[0] = 0;
        lcxp = 0;
        c_q0 = u32::from(cx_val[0]) + (u32::from(cx_val[1]) << 2);
        cx_val[0] = 0;

        let mut x = 0usize;
        while x < width {
            let q0 = Quad::load(buf, stride, width, height, x, y, p);
            let kappa = if q0.rho & (q0.rho.wrapping_sub(1)) != 0 {
                to_u32_clamped(max_e.max(1))
            } else {
                1
            };
            let uq0 = q0.e_qmax.max(kappa);
            let u_q0 = uq0 - kappa;
            let mut u_q1 = 0;
            let eps0 = q0.eps(u_q0);

            e_val[lep] = e_val[lep].max(to_u8(q0.e_q[1]));
            lep += 1;
            max_e = i32::from(e_val[lep].max(e_val[lep + 1])) - 1;
            e_val[lep] = to_u8(q0.e_q[3]);
            cx_val[lcxp] |= to_u8((q0.rho & 2) >> 1);
            lcxp += 1;
            let mut c_q1 = u32::from(cx_val[lcxp]) + (u32::from(cx_val[lcxp + 1]) << 2);
            cx_val[lcxp] = to_u8((q0.rho & 8) >> 3);

            let tuple0 = u32::from(ENC_VLC_TBL1[((c_q0 << 8) + (q0.rho << 4) + eps0) as usize]);
            vlc.encode(tuple0 >> 8, (tuple0 >> 4) & 7);
            if c_q0 == 0 {
                mel.encode(q0.rho != 0);
            }
            emit_magsgn(&mut ms, &q0, tuple0, uq0);

            if x + 2 < width {
                let q1 = Quad::load(buf, stride, width, height, x + 2, y, p);
                let kappa = if q1.rho & (q1.rho.wrapping_sub(1)) != 0 {
                    to_u32_clamped(max_e.max(1))
                } else {
                    1
                };
                c_q1 |= ((q0.rho & 4) >> 1) | ((q0.rho & 8) >> 2);
                let uq1 = q1.e_qmax.max(kappa);
                u_q1 = uq1 - kappa;
                let eps1 = q1.eps(u_q1);

                e_val[lep] = e_val[lep].max(to_u8(q1.e_q[1]));
                lep += 1;
                max_e = i32::from(e_val[lep].max(e_val[lep + 1])) - 1;
                e_val[lep] = to_u8(q1.e_q[3]);
                cx_val[lcxp] |= to_u8((q1.rho & 2) >> 1);
                lcxp += 1;
                c_q0 = u32::from(cx_val[lcxp]) + (u32::from(cx_val[lcxp + 1]) << 2);
                cx_val[lcxp] = to_u8((q1.rho & 8) >> 3);

                let tuple1 = u32::from(ENC_VLC_TBL1[((c_q1 << 8) + (q1.rho << 4) + eps1) as usize]);
                vlc.encode(tuple1 >> 8, (tuple1 >> 4) & 7);
                if c_q1 == 0 {
                    mel.encode(q1.rho != 0);
                }
                emit_magsgn(&mut ms, &q1, tuple1, uq1);

                c_q0 |= ((q1.rho & 4) >> 1) | ((q1.rho & 8) >> 2);
            } else {
                c_q0 = c_q1;
            }

            // UVLC for the pair (no MEL event on non-initial rows).
            let (c0, c1) = (ENC_UVLC[u_q0 as usize], ENC_UVLC[u_q1 as usize]);
            vlc.encode(c0.prefix.into(), c0.prefix_len.into());
            vlc.encode(c1.prefix.into(), c1.prefix_len.into());
            vlc.encode(c0.suffix.into(), c0.suffix_len.into());
            vlc.encode(c1.suffix.into(), c1.suffix_len.into());

            x += 4;
        }
        y += 2;
    }

    // ---- terminate & assemble -----------------------------------------
    let (mel_bytes, vlc_bytes) = vlc.terminate_with_mel(mel);
    let ms_bytes = ms.terminate();

    let scup = mel_bytes.len() + vlc_bytes.len();
    if !(2..=4079).contains(&scup) {
        return Err(Error::InvalidArgument {
            message: "cleanup segment suffix out of range (Scup)".into(),
        });
    }
    let mut out = ms_bytes;
    out.reserve(scup);
    out.extend_from_slice(&mel_bytes);
    out.extend_from_slice(&vlc_bytes);
    let n = out.len();
    #[allow(clippy::cast_possible_truncation)] // scup <= 4079
    {
        out[n - 1] = (scup >> 4) as u8;
        out[n - 2] = (out[n - 2] & 0xF0) | (scup & 0xF) as u8;
    }
    Ok(out)
}

#[inline]
#[allow(clippy::cast_possible_truncation)]
fn to_u8(v: u32) -> u8 {
    debug_assert!(v <= 255);
    v as u8
}

#[inline]
#[allow(clippy::cast_sign_loss)]
fn to_u32_clamped(v: i32) -> u32 {
    debug_assert!(v >= 0);
    v.max(0) as u32
}
