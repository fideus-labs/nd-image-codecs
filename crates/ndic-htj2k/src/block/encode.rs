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

use alloc::vec::Vec;

use ndic_core::{Error, Result};

use super::mel::MelEncoder;
use super::streams::{MagSgnEncoder, VlcEncoder};
use super::tables::{ENC_UVLC, ENC_VLC_TBL0, ENC_VLC_TBL1, UvlcCode};

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
    /// Loads the 2x2 quad at column `x` from the sign-magnitude rows `rows`
    /// (top, bottom); columns past a row's end read as insignificant, and an
    /// empty bottom row stands in for the one beyond the block.
    #[inline(always)]
    #[allow(clippy::inline_always)] // out of line, the quad returns through memory
    fn load(rows: [&[u32]; 2], x: usize, p: u32) -> Self {
        // A missing sample reads as zero, which codes exactly as an insignificant one. Away from
        // a row's end both columns of each row load with one check, so the four lanes compute
        // without branches.
        let samples = match (rows[0].get(x..x + 2), rows[1].get(x..x + 2)) {
            (Some(&[t0, t1]), Some(&[b0, b1])) => [t0, b0, t1, b1],
            _ => core::array::from_fn(|n| rows[n % 2].get(x + n / 2).copied().unwrap_or(0)),
        };
        let mut quad = Self::default();
        for (n, &raw) in samples.iter().enumerate() {
            // (raw + raw) drops the sign bit; >> p aligns the coding LSB.
            let val = (raw.wrapping_add(raw) >> p) & !1; // 2 mu_p
            // Significance is data-dependent and mispredicts, so an insignificant sample is
            // masked to zero instead of branched around. Its `val - 1` wraps to all ones, whose
            // zero `leading_zeros` the significance bit then clears.
            let sig = u32::from(val != 0);
            let val = val.wrapping_sub(1); // 2 mu_p - 1
            quad.rho |= sig << n;
            quad.e_q[n] = (32 - val.leading_zeros()) * sig;
            quad.e_qmax = quad.e_qmax.max(quad.e_q[n]);
            quad.s[n] = (val.wrapping_sub(1) + (raw >> 31)) & sig.wrapping_neg(); // 2 (mu_p - 1) + sign
        }
        quad
    }

    /// EMB pattern: which samples attain the quad's maximum exponent
    /// (communicated only when a `u` value is coded).
    fn eps(&self, u_q: u32) -> u32 {
        // A coded `u` means `e_qmax` exceeds kappa >= 1, and insignificant samples have a zero
        // exponent, so matching `e_qmax` alone implies significance.
        debug_assert!(u_q == 0 || self.e_qmax >= 2);
        let mut eps = 0;
        for n in 0..4 {
            eps |= u32::from(self.e_q[n] == self.e_qmax) << n;
        }
        // Whether `u` is coded varies quad to quad, so the pattern is masked rather than
        // skipped behind a branch.
        eps & u32::from(u_q != 0).wrapping_neg()
    }
}

/// The two sample rows of the quad row starting at `y`, each `width` long.
/// The bottom row is empty when `y` is the block's last row.
fn quad_rows(buf: &[u32], stride: usize, width: usize, height: usize, y: usize) -> [&[u32]; 2] {
    let top = &buf[y * stride..y * stride + width];
    let bottom = if y + 1 < height {
        &buf[(y + 1) * stride..(y + 1) * stride + width]
    } else {
        &[]
    };
    [top, bottom]
}

/// Emits the `MagSgn` bits of one quad given its VLC tuple (`e_k` in the
/// low nibble) and exponent bound `u_q_cap`.
fn emit_magsgn(ms: &mut MagSgnEncoder, q: &Quad, tuple: u32, uq: u32) {
    // Insignificant samples become zero-length codewords, so all four go out in one run.
    let word = |n: usize| {
        let m = (uq - ((tuple >> n) & 1)) * ((q.rho >> n) & 1);
        let mask = 1u32.checked_shl(m).map_or(u32::MAX, |b| b - 1);
        (u64::from(q.s[n] & mask), m)
    };
    if uq <= 14 {
        // Four codewords of at most 14 bits fit beside the 7 a flush leaves, so the quad
        // flushes once.
        for n in 0..4 {
            let (cwd, len) = word(n);
            ms.merge(cwd, len);
        }
        ms.flush();
    } else if uq <= 28 {
        // Two codewords of at most 28 bits fit, so the quad flushes twice.
        for n in 0..4 {
            let (cwd, len) = word(n);
            ms.merge(cwd, len);
            if n % 2 == 1 {
                ms.flush();
            }
        }
    } else {
        for n in 0..4 {
            let (cwd, len) = word(n);
            ms.merge(cwd, len);
            ms.flush();
        }
    }
}

/// Merges a quad pair's UVLC codewords into the VLC stream and flushes it once for the whole
/// pair. The pair's two VLC codewords of at most 7 bits and UVLC codes of at most 16 fit beside
/// the 7 bits a flush leaves.
fn finish_pair(vlc: &mut VlcEncoder, words: &[(u32, u32)]) {
    for &(cwd, len) in words {
        vlc.merge(u64::from(cwd), len);
    }
    vlc.flush();
}

/// Reusable working buffers for encoding code-blocks. Encoding through one
/// [`BlockEncoder`] allocates its stream and line buffers once rather than
/// once per block as [`encode_block`] does, so each block allocates only its
/// own output.
#[derive(Debug, Default)]
pub struct BlockEncoder {
    /// MEL stream bytes.
    mel: Vec<u8>,
    /// VLC stream bytes.
    vlc: Vec<u8>,
    /// `MagSgn` stream bytes.
    ms: Vec<u8>,
    /// E values a quad row leaves for the next.
    e_val: Vec<u8>,
    /// Context values a quad row leaves for the next.
    cx_val: Vec<u8>,
}

impl BlockEncoder {
    /// Creates an encoder whose buffers grow on first use.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Encodes one code-block exactly as [`encode_block`] does, reusing this
    /// encoder's buffers.
    ///
    /// # Errors
    /// As [`encode_block`].
    pub fn encode(
        &mut self,
        buf: &[u32],
        width: usize,
        height: usize,
        stride: usize,
        missing_msbs: u32,
    ) -> Result<Vec<u8>> {
        encode_with(self, buf, width, height, stride, missing_msbs)
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
pub fn encode_block(
    buf: &[u32],
    width: usize,
    height: usize,
    stride: usize,
    missing_msbs: u32,
) -> Result<Vec<u8>> {
    BlockEncoder::new().encode(buf, width, height, stride, missing_msbs)
}

/// The body of [`BlockEncoder::encode`], with its buffers.
#[allow(clippy::too_many_lines)] // faithful port of one long reference routine
#[allow(clippy::similar_names, clippy::many_single_char_names)] // T.814 names
fn encode_with(
    bufs: &mut BlockEncoder,
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

    let mut mel = MelEncoder::with_buffer(core::mem::take(&mut bufs.mel));
    let mut vlc = VlcEncoder::with_buffer(core::mem::take(&mut bufs.vlc));
    let mut ms = MagSgnEncoder::with_buffer(core::mem::take(&mut bufs.ms));

    // Line buffers for the E and context values a quad row leaves for the
    // next one. One byte per quad column; sized for 512 quads plus slack.
    let cols = width.div_ceil(2);
    let e_val = &mut bufs.e_val;
    e_val.clear();
    e_val.resize(cols + 4, 0);
    let cx_val = &mut bufs.cx_val;
    cx_val.clear();
    cx_val.resize(cols + 4, 0);

    // ---- initial row of quads (kappa = 1) -----------------------------
    let mut c_q0: u32 = 0;
    let mut lep = 0usize; // cursor into e_val
    let mut lcxp = 0usize; // cursor into cx_val
    let rows = quad_rows(buf, stride, width, height, 0);
    let mut x = 0usize;
    while x < width {
        // Both quads load up front, so the second's loads overlap the first's coding. A missing
        // second quad loads as insignificant.
        let q0 = Quad::load(rows, x, p);
        let q1 = Quad::load(rows, x + 2, p);
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
        vlc.merge(u64::from(tuple0 >> 8), (tuple0 >> 4) & 7);
        if c_q0 == 0 {
            mel.encode(q0.rho != 0);
        }
        emit_magsgn(&mut ms, &q0, tuple0, uq0);

        if x + 2 < width {
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
            vlc.merge(u64::from(tuple1 >> 8), (tuple1 >> 4) & 7);
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
            finish_pair(&mut vlc, &uvlc_pair(c0, c1));
        } else if u_q0 > 2 && u_q1 > 0 {
            let c0 = ENC_UVLC[u_q0 as usize];
            finish_pair(
                &mut vlc,
                &[
                    (c0.prefix.into(), c0.prefix_len.into()),
                    (u_q1 - 1, 1),
                    (c0.suffix.into(), c0.suffix_len.into()),
                ],
            );
        } else {
            let (c0, c1) = (ENC_UVLC[u_q0 as usize], ENC_UVLC[u_q1 as usize]);
            finish_pair(&mut vlc, &uvlc_pair(c0, c1));
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

        let rows = quad_rows(buf, stride, width, height, y);
        let mut x = 0usize;
        while x < width {
            // Both quads load up front, so the second's loads overlap the first's coding. A
            // missing second quad loads as insignificant.
            let q0 = Quad::load(rows, x, p);
            let q1 = Quad::load(rows, x + 2, p);
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
            vlc.merge(u64::from(tuple0 >> 8), (tuple0 >> 4) & 7);
            if c_q0 == 0 {
                mel.encode(q0.rho != 0);
            }
            emit_magsgn(&mut ms, &q0, tuple0, uq0);

            if x + 2 < width {
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
                vlc.merge(u64::from(tuple1 >> 8), (tuple1 >> 4) & 7);
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
            finish_pair(&mut vlc, &uvlc_pair(c0, c1));

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
    let mut out = Vec::with_capacity(ms_bytes.len() + scup);
    out.extend_from_slice(&ms_bytes);
    out.extend_from_slice(&mel_bytes);
    out.extend_from_slice(&vlc_bytes);
    let n = out.len();
    #[allow(clippy::cast_possible_truncation)] // scup <= 4079
    {
        out[n - 1] = (scup >> 4) as u8;
        out[n - 2] = (out[n - 2] & 0xF0) | (scup & 0xF) as u8;
    }
    // Hand the stream buffers back for the next block.
    (bufs.mel, bufs.vlc, bufs.ms) = (mel_bytes, vlc_bytes, ms_bytes);
    Ok(out)
}

/// The UVLC codewords of a quad pair in stream order: both prefixes, then both
/// suffixes.
fn uvlc_pair(c0: UvlcCode, c1: UvlcCode) -> [(u32, u32); 4] {
    [
        (c0.prefix.into(), c0.prefix_len.into()),
        (c1.prefix.into(), c1.prefix_len.into()),
        (c0.suffix.into(), c0.suffix_len.into()),
        (c1.suffix.into(), c1.suffix_len.into()),
    ]
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
