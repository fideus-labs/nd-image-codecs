//! HT block decoder: cleanup, `SigProp` and `MagRef` passes (T.814 §7).
//!
//! Ported from `OpenJPH` `ojph_block_decoder32.cpp` (BSD-2-Clause). The
//! decoder writes **sign-magnitude** samples: bit 31 is the sign, and a
//! sample significant in the cleanup pass holds `mu << p | 1 << (p - 1)`
//! (magnitude plus half the quantization bin), where `p = 30 - missing_msbs`
//! is the cleanup pass's least significant bitplane. Use
//! [`super::sign_magnitude_to_coeff`] to map decoded samples back to `i32`
//! coefficients.
//!
//! Unlike the original — which deliberately over-reads its (padded) buffers —
//! every read here is bounds-checked, and every sample store is guarded so a
//! malformed codestream can never write outside `out[..height x stride]`.

extern crate alloc;

use alloc::vec::Vec;

use ndic_core::{Error, Result};

use super::BlockPasses;
use super::mel::MelDecoder;
use super::streams::{CleanBits, FwdReader, RevReader, unstuff_magsgn, unstuff_vlc};
use super::tables::{DEC_VLC_TBL0, DEC_VLC_TBL1, UVLC_TBL0, UVLC_TBL1};

/// Neighbor-propagation masks for the `SigProp` membership scan, indexed by
/// the row (0..=3) of the sample that just became significant.
const SPP_PROPAGATION: [u32; 4] = [0x33, 0x76, 0xEC, 0xC8];

/// Sanity bound on `Scup` from T.814 (`scup <= 4079`).
const MAX_SCUP: usize = 4079;

fn malformed(offset: usize, message: &str) -> Error {
    Error::Codestream {
        offset,
        message: message.into(),
    }
}

/// Reusable working buffers for decoding code-blocks. Decoding through one
/// [`BlockDecoder`] allocates its buffers once, sized to the largest block,
/// rather than once per block as [`decode_block`] does.
#[derive(Debug, Default)]
pub struct BlockDecoder {
    /// Quad-row `inf`/`u_q` pairs, later re-packed into the `sigma` map.
    scratch: Vec<u16>,
    /// `v_n` of the quad row above, the E-max context.
    v_n_scratch: Vec<u32>,
    /// The unstuffed `MagSgn` segment.
    clean: Vec<u8>,
    /// The unstuffed VLC stream.
    vlc_clean: Vec<u8>,
    /// Significance of the stripe row above, for `SigProp`.
    prev_row_sig: Vec<u16>,
}

impl BlockDecoder {
    /// Creates a decoder whose buffers grow on first use.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes one code-block exactly as [`decode_block`] does, reusing this
    /// decoder's buffers.
    ///
    /// # Errors
    /// As [`decode_block`].
    #[allow(clippy::too_many_arguments)] // the decode_block signature
    pub fn decode(
        &mut self,
        coded: &[u8],
        out: &mut [u32],
        width: usize,
        height: usize,
        stride: usize,
        missing_msbs: u32,
        passes: &BlockPasses,
        stripe_causal: bool,
    ) -> Result<()> {
        decode_with(
            self,
            coded,
            out,
            width,
            height,
            stride,
            missing_msbs,
            passes,
            stripe_causal,
        )
    }
}

/// Decodes one HT code-block into sign-magnitude samples.
///
/// * `coded` — the codeword segment: cleanup bytes (`passes.len_cleanup`)
///   followed by refinement bytes (`passes.len_refinement`).
/// * `out` — destination, row-major with `stride`; fully overwritten for the
///   `width x height` block.
/// * `missing_msbs` — number of missing magnitude bit-planes signaled in the
///   packet header; the cleanup LSB is `p = 30 - missing_msbs`.
/// * `stripe_causal` — the `SPcod` vertically-causal context flag.
///
/// # Errors
/// [`Error::Codestream`] on malformed input; never panics.
#[allow(clippy::too_many_arguments)] // faithful port
pub fn decode_block(
    coded: &[u8],
    out: &mut [u32],
    width: usize,
    height: usize,
    stride: usize,
    missing_msbs: u32,
    passes: &BlockPasses,
    stripe_causal: bool,
) -> Result<()> {
    BlockDecoder::new().decode(
        coded,
        out,
        width,
        height,
        stride,
        missing_msbs,
        passes,
        stripe_causal,
    )
}

/// The body of [`BlockDecoder::decode`], with its buffers.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // faithful port
#[allow(clippy::similar_names, clippy::many_single_char_names)] // T.814 names
fn decode_with(
    bufs: &mut BlockDecoder,
    coded: &[u8],
    out: &mut [u32],
    width: usize,
    height: usize,
    stride: usize,
    missing_msbs: u32,
    passes: &BlockPasses,
    stripe_causal: bool,
) -> Result<()> {
    let mut num_passes = passes.num_passes;
    let lengths1 = passes.len_cleanup;
    let lengths2 = passes.len_refinement;

    // ---- geometry & argument validation -------------------------------
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return Err(Error::InvalidArgument {
            message: "code-block dimensions must be 1..=1024".into(),
        });
    }
    if width * height > 4096 {
        return Err(Error::InvalidArgument {
            message: "code-block area must be <= 4096 samples (T.800 §B.7)".into(),
        });
    }
    if stride < width || out.len() < (height - 1) * stride + width {
        return Err(Error::InvalidArgument {
            message: "output buffer too small for code-block geometry".into(),
        });
    }

    // ---- pass / precision validation (mirrors OpenJPH's guards) -------
    if num_passes > 1 && lengths2 == 0 {
        num_passes = 1; // malformed: refinement passes with no bytes
    }
    if num_passes > 3 {
        return Err(malformed(0, "more than 3 coding passes"));
    }
    if missing_msbs >= 30 {
        return Err(malformed(0, "missing MSBs >= 30: not decodable in 32 bits"));
    }
    if missing_msbs == 29 {
        num_passes = 1; // not enough precision for SigProp/MagRef
    }
    let p = 30 - missing_msbs;

    if lengths1 < 2 || lengths1 > coded.len() || lengths1 + lengths2 > coded.len() {
        return Err(malformed(0, "invalid code-block pass lengths"));
    }

    let lcup = lengths1;
    let scup = (usize::from(coded[lcup - 1]) << 4) + usize::from(coded[lcup - 2] & 0xF);
    if scup < 2 || scup > lcup || scup > MAX_SCUP {
        return Err(malformed(lcup - 2, "invalid Scup interface word"));
    }

    // ---- scratch buffers ----------------------------------------------
    // Two u16 entries per quad: `inf` (e_k<<12 | e_1<<8 | rho<<4 | len) and
    // `u_q`. One extra quad row absorbs the terminator writes of the last
    // row, mirroring the reference layout.
    let sstr = (width + 2 + 7) & !7;
    let quad_rows = height.div_ceil(2);
    // The same buffer is later re-packed in place into the `sigma` column
    // layout used by SigProp/MagRef (one u16 = 4 columns x 4 rows).
    let mstr = (width.div_ceil(4) + 2 + 7) & !7;
    let sig_rows = height.div_ceil(4);
    let scratch_len = ((quad_rows + 1) * sstr + 8).max((sig_rows + 1) * mstr + 8);
    let scratch = &mut bufs.scratch;
    scratch.clear();
    scratch.resize(scratch_len, 0);

    let mmsbp2 = missing_msbs + 2;

    // ---- steps 1 and 2: VLC + MEL, then MagSgn ------------------------
    // Step 1 is a serial chain of table lookups, each waiting on the bits the one before
    // consumed. Step 2 is independent loads and arithmetic. After the initial row, step 1 on a
    // quad row runs in the same loop as step 2 on the row above, so the core overlaps the chains.
    let mut mel = MelDecoder::new(coded, lcup, scup);
    // Unstuffed up front, so the VLC bits sit in registers and refill without branching.
    unstuff_vlc(coded, lcup, scup, &mut bufs.vlc_clean);
    let vlc_clean = &bufs.vlc_clean;
    let mut vlc = CleanBits::default();
    let mut run = mel.get_run();
    {
        // Initial row of quads (kappa = 1).
        let mut c_q: u32 = 0;
        let mut idx = 0usize;
        let mut x = 0usize;
        while x < width {
            // First quad of the pair.
            let vlc_val = vlc.fetch(vlc_clean);
            let mut t0 = u32::from(DEC_VLC_TBL0[(c_q | (vlc_val & 0x7F)) as usize]);
            if c_q == 0 {
                // Zero context: quad significance comes from a MEL event.
                run -= 2;
                t0 = if run == -1 { t0 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            scratch[idx] = to_u16(t0);
            x += 2;

            // Context for the second quad (T.814 eq. 1).
            c_q = ((t0 & 0x10) << 3) | ((t0 & 0xE0) << 2);
            let vlc_val = vlc.advance(t0 & 0x7);

            let mut t1 = u32::from(DEC_VLC_TBL0[(c_q | (vlc_val & 0x7F)) as usize]);
            if c_q == 0 && x < width {
                run -= 2;
                t1 = if run == -1 { t1 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            if x >= width {
                t1 = 0;
            }
            scratch[idx + 2] = to_u16(t1);
            x += 2;

            c_q = ((t1 & 0x10) << 3) | ((t1 & 0xE0) << 2);
            let vlc_val = vlc.advance(t1 & 0x7);

            // u values for the pair (kappa = 1 on the initial row).
            let mut uvlc_mode = ((t0 & 0x8) << 3) | ((t1 & 0x8) << 4);
            if uvlc_mode == 0xC0 {
                // Both u offsets set: a MEL event signals min(u0, u1) > 2.
                run -= 2;
                uvlc_mode += if run == -1 { 0x40 } else { 0 };
                if run < 0 {
                    run = mel.get_run();
                }
            }
            let mut uvlc_entry = u32::from(UVLC_TBL0[(uvlc_mode + (vlc_val & 0x3F)) as usize]);
            let vlc_val = vlc.advance(uvlc_entry & 0x7);
            uvlc_entry >>= 3;
            let len = uvlc_entry & 0xF;
            let tmp = vlc_val & ((1 << len) - 1);
            vlc.advance(len);
            uvlc_entry >>= 4;
            let len0 = uvlc_entry & 0x7;
            uvlc_entry >>= 3;
            scratch[idx + 1] = to_u16(1 + (uvlc_entry & 7) + (tmp & !(0xFF << len0)));
            scratch[idx + 3] = to_u16(1 + (uvlc_entry >> 3) + (tmp >> len0));
            idx += 4;
        }
        scratch[idx] = 0;
        scratch[idx + 1] = 0;
    }

    // Exponent bookkeeping: v_n of the bottom sample of column pairs (2k-1, 2k), used as the
    // E-max context of the quad row below.
    let v_n_scratch = &mut bufs.v_n_scratch;
    v_n_scratch.clear();
    v_n_scratch.resize(width / 2 + 4, 0);
    // Unstuff the whole MagSgn segment up front. Every sample's bit length is known from step 1,
    // so its bits are then one unaligned load at a running offset, and consecutive samples no
    // longer wait on each other through a stateful reader.
    unstuff_magsgn(&coded[..lcup - scup], &mut bufs.clean);
    let mut ms = MagSgn {
        clean: &bufs.clean,
        off: 0,
        p,
        stride,
        width,
        mmsbp2,
    };

    for qy in 1..=quad_rows {
        // Step 2 decodes quad row qy - 1 while step 1 decodes quad row qy, when there is one.
        let y = (qy - 1) * 2;
        let mut r = MsRow {
            sp: 0,
            vp: 0,
            dp: y * stride,
            x: 0,
            prev_v_n: 0,
            bottom: y + 1 < height,
        };
        let (done, rest) = scratch.split_at_mut(qy * sstr);
        let above = &done[(qy - 1) * sstr..];
        let mut idx = 0usize;
        if qy < quad_rows {
            let row = &mut rest[..sstr];
            let mut c_q: u32 = 0;
            let mut x = 0usize;
            while x < width {
                vlc_pair(
                    &mut vlc, vlc_clean, &mut mel, &mut run, above, row, &mut c_q, idx, x, width,
                );
                if qy == 1 {
                    ms_pair::<true>(&mut ms, &mut r, above, out, v_n_scratch)?;
                } else {
                    ms_pair::<false>(&mut ms, &mut r, above, out, v_n_scratch)?;
                }
                idx += 4;
                x += 4;
            }
        } else {
            // The last quad row has no step 1 partner.
            while r.x < width {
                if qy == 1 {
                    ms_pair::<true>(&mut ms, &mut r, above, out, v_n_scratch)?;
                } else {
                    ms_pair::<false>(&mut ms, &mut r, above, out, v_n_scratch)?;
                }
            }
        }
        v_n_scratch[r.vp] = r.prev_v_n;
        if qy < quad_rows {
            // Terminator writes, which can reach into the next row's first entries as in the
            // reference layout, so they go through the whole buffer.
            let base = qy * sstr;
            scratch[base + idx] = 0;
            scratch[base + idx + 1] = 0;
        }
    }

    if num_passes < 2 {
        return Ok(());
    }

    // ---- re-pack quad significance into the column-oriented sigma map --
    // Each u16 covers 4 columns x 4 rows; bit (4*col + row) is sigma.
    {
        let mut y = 0usize;
        while y < height {
            let sp = (y >> 1) * sstr;
            let dpi = (y >> 2) * mstr;
            let mut sx = 0usize;
            let mut dx = 0usize;
            let mut x = 0usize;
            while x < width {
                let s0 = u32::from(scratch[sp + sx]);
                let s2 = u32::from(scratch[sp + sx + 2]);
                let s0b = u32::from(scratch[sp + sstr + sx]);
                let s2b = u32::from(scratch[sp + sstr + sx + 2]);
                let t0 = ((s0 & 0x30) >> 4)
                    | ((s0 & 0xC0) >> 2)
                    | ((s2 & 0x30) << 4)
                    | ((s2 & 0xC0) << 6);
                let t1 =
                    ((s0b & 0x30) >> 2) | (s0b & 0xC0) | ((s2b & 0x30) << 6) | ((s2b & 0xC0) << 8);
                scratch[dpi + dx] = to_u16(t0 | t1);
                sx += 4;
                dx += 1;
                x += 4;
            }
            scratch[dpi + dx] = 0;
            y += 4;
        }
        // One zeroed row below the block.
        let dpi = height.div_ceil(4) * mstr;
        let groups = width.div_ceil(4);
        for e in &mut scratch[dpi..=dpi + groups] {
            *e = 0;
        }
    }

    // ---- Significance Propagation pass --------------------------------
    {
        let groups = width.div_ceil(4);
        let prev_row_sig = &mut bufs.prev_row_sig;
        prev_row_sig.clear();
        prev_row_sig.resize(groups + 2, 0);
        let mut sigprop = FwdReader::<0>::new(&coded[lcup..lcup + lengths2]);

        let mut y = 0usize;
        while y < height {
            let mut pattern: u32 = match height - y {
                1 => 0x1111,
                2 => 0x3333,
                3 => 0x7777,
                _ => 0xFFFF,
            };

            let mut prev: u32 = 0;
            let cur_base = (y >> 2) * mstr;
            let dpp = y * stride;
            let mut gi = 0usize;
            let mut x = 0usize;
            while x < width {
                // Truncate the pattern for a partial rightmost group.
                let overhang = (x + 4).saturating_sub(width);
                pattern >>= overhang * 4;

                let ps = u32::from(prev_row_sig[gi]) | (u32::from(prev_row_sig[gi + 1]) << 16);
                let ns = u32::from(scratch[cur_base + mstr + gi])
                    | (u32::from(scratch[cur_base + mstr + gi + 1]) << 16);
                let cs = u32::from(scratch[cur_base + gi])
                    | (u32::from(scratch[cur_base + gi + 1]) << 16);

                let mut u = (ps & 0x8888_8888) >> 3; // row above the stripe
                if !stripe_causal {
                    u |= (ns & 0x1111_1111) << 3; // row below the stripe
                }

                // Candidate members: neighbors of significant samples.
                let mut mbr = cs;
                mbr |= (cs & 0x7777_7777) << 1;
                mbr |= (cs & 0xEEEE_EEEE) >> 1;
                mbr |= u;
                let t = mbr;
                mbr |= t << 4;
                mbr |= t >> 4;
                mbr |= prev >> 12;
                mbr &= pattern;
                mbr &= !cs;

                let mut new_sig = 0u32;
                if mbr != 0 {
                    let mut cwd = sigprop.fetch();
                    let mut cnt = 0u32;
                    let inv_sig = !cs & pattern;

                    // Visit candidates lowest bit first, which is column by column with rows in
                    // order. A new significance only adds candidates at or after its own bit, so
                    // taking the lowest pending bit each time keeps the reference order.
                    let mut pending = mbr;
                    while pending != 0 {
                        let b = pending.trailing_zeros();
                        pending &= pending - 1;
                        if cwd & 1 != 0 {
                            let t = (SPP_PROPAGATION[(b & 3) as usize] << (b & !3)) & inv_sig;
                            new_sig |= 1 << b;
                            pending |= t & !((2 << b) - 1);
                        }
                        cwd >>= 1;
                        cnt += 1;
                    }

                    // Read the sign bits of newly significant samples, in the same order.
                    let val = 3 << (p - 2);
                    let mut rest = new_sig;
                    while rest != 0 {
                        let b = rest.trailing_zeros() as usize;
                        rest &= rest - 1;
                        out[dpp + x + (b >> 2) + (b & 3) * stride] = (cwd << 31) | val;
                        cwd >>= 1;
                        cnt += 1;
                    }
                    sigprop.advance(cnt);
                }

                new_sig |= cs;
                prev_row_sig[gi] = to_u16(new_sig & 0xFFFF);

                // Carry for the next group: this group's rightmost column.
                let t = new_sig;
                new_sig |= (t & 0x7777) << 1;
                new_sig |= (t & 0xEEEE) >> 1;
                prev = (new_sig | u) & 0xF000;

                gi += 1;
                x += 4;
            }
            y += 4;
        }
    }

    if num_passes < 3 {
        return Ok(());
    }

    // ---- Magnitude Refinement pass ------------------------------------
    {
        let mut magref = RevReader::new_mrp(coded, lcup, lengths2);
        let half = 1u32 << (p - 2);

        let mut y = 0usize;
        while y < height {
            let cur_base = (y >> 2) * mstr;
            let dpp = y * stride;
            let mut gi = 0usize;
            let mut i = 0usize;
            while i < width {
                let cwd_start = magref.fetch();
                let mut cwd = cwd_start;
                let sig = u32::from(scratch[cur_base + gi])
                    | (u32::from(scratch[cur_base + gi + 1]) << 16);
                // Visit significant samples lowest bit first, which is column by column with rows
                // in order.
                let mut rest = sig;
                while rest != 0 {
                    let b = rest.trailing_zeros() as usize;
                    rest &= rest - 1;
                    let (j, k) = (b >> 2, b & 3);
                    // Refine bitplane p-1 of a cleanup-significant sample.
                    if i + j < width && y + k < height {
                        let sym = ((1 - (cwd & 1)) << (p - 1)) | half;
                        out[dpp + i + j + k * stride] ^= sym;
                    }
                    cwd >>= 1;
                }
                magref.advance(sig.count_ones());
                gi += 2;
                i += 8;
            }
            y += 4;
        }
    }

    Ok(())
}

/// Decodes the VLC and MEL symbols of the quad pair at entry `idx`, column `x`, of a non-initial
/// quad row into `row`, with `c_q` the context the pair's left neighbour left and `above` the
/// row above. The row above's entry past a pair whose second quad lies outside the block may
/// sit beyond `above`, and reads as zero, since only that absent quad would use it.
#[inline(always)]
#[allow(clippy::inline_always, clippy::too_many_arguments)] // inlined into the fused step loop
fn vlc_pair(
    vlc: &mut CleanBits,
    clean: &[u8],
    mel: &mut MelDecoder<'_>,
    run: &mut i32,
    above: &[u16],
    row: &mut [u16],
    c_q: &mut u32,
    idx: usize,
    x: usize,
    width: usize,
) {
    // sigma of the n, ne, nf neighbors (T.814 eq. 2).
    *c_q |= (u32::from(above[idx]) & 0xA0) << 2;
    *c_q |= (u32::from(above[idx + 2]) & 0x20) << 4;

    let vlc_val = vlc.fetch(clean);
    let mut t0 = u32::from(DEC_VLC_TBL1[(*c_q | (vlc_val & 0x7F)) as usize]);
    if *c_q == 0 {
        *run -= 2;
        t0 = if *run == -1 { t0 } else { 0 };
        if *run < 0 {
            *run = mel.get_run();
        }
    }
    row[idx] = to_u16(t0);

    // sigma of w, sw / nw / n, ne, nf for the second quad.
    *c_q = ((t0 & 0x40) << 2) | ((t0 & 0x80) << 1);
    *c_q |= u32::from(above[idx]) & 0x80;
    *c_q |= (u32::from(above[idx + 2]) & 0xA0) << 2;
    *c_q |= (u32::from(above.get(idx + 4).copied().unwrap_or(0)) & 0x20) << 4;
    let vlc_val = vlc.advance(t0 & 0x7);

    let mut t1 = u32::from(DEC_VLC_TBL1[(*c_q | (vlc_val & 0x7F)) as usize]);
    if *c_q == 0 && x + 2 < width {
        *run -= 2;
        t1 = if *run == -1 { t1 } else { 0 };
        if *run < 0 {
            *run = mel.get_run();
        }
    }
    if x + 2 >= width {
        t1 = 0;
    }
    row[idx + 2] = to_u16(t1);

    *c_q = ((t1 & 0x40) << 2) | ((t1 & 0x80) << 1);
    *c_q |= u32::from(above[idx + 2]) & 0x80;
    let vlc_val = vlc.advance(t1 & 0x7);

    // u values (kappa is applied later, in the MagSgn step).
    let uvlc_mode = ((t0 & 0x8) << 3) | ((t1 & 0x8) << 4);
    let mut uvlc_entry = u32::from(UVLC_TBL1[(uvlc_mode + (vlc_val & 0x3F)) as usize]);
    let vlc_val = vlc.advance(uvlc_entry & 0x7);
    uvlc_entry >>= 3;
    let len = uvlc_entry & 0xF;
    let tmp = vlc_val & ((1 << len) - 1);
    vlc.advance(len);
    uvlc_entry >>= 4;
    let len0 = uvlc_entry & 0x7;
    uvlc_entry >>= 3;
    row[idx + 1] = to_u16((uvlc_entry & 7) + (tmp & !(0xFF << len0)));
    row[idx + 3] = to_u16((uvlc_entry >> 3) + (tmp >> len0));
}

/// Step 2's decoder state: the unstuffed `MagSgn` bits and the geometry every quad shares.
struct MagSgn<'a> {
    clean: &'a [u8],
    /// Bit offset of the next sample's magnitude and sign.
    off: u32,
    p: u32,
    stride: usize,
    width: usize,
    mmsbp2: u32,
}

/// Step 2's position within one quad row.
struct MsRow {
    /// Entry of the next quad in the row's scratch.
    sp: usize,
    /// Column pair of the next quad in `v_n_scratch`.
    vp: usize,
    /// Output index of the next quad's top-left sample.
    dp: usize,
    /// Column of the next quad.
    x: usize,
    /// `v_n` of the previous quad's bottom-right sample.
    prev_v_n: u32,
    /// Whether the row has a bottom sample row inside the block.
    bottom: bool,
}

/// Decodes one sample; returns (value, `v_n`), both zero when its rho bit is clear. Bits are
/// always consumed, but the caller guards the store.
#[inline(always)]
#[allow(clippy::inline_always)] // four per quad in the fused step loop
fn decode_sample(ms: &mut MagSgn<'_>, inf: u32, bit: u32, u_q: u32) -> (u32, u32) {
    // Significance is data-dependent and mispredicts, so an insignificant sample reads zero bits
    // and is masked to zero instead of branching away.
    let keep = ((inf >> (4 + bit)) & 1).wrapping_neg();
    let ms_val = bits_at(ms.clean, ms.off);
    let m_n = u_q.wrapping_sub((inf >> (12 + bit)) & 1) & keep;
    ms.off += m_n;
    let mut v_n = ms_val & ((1u32 << m_n) - 1);
    v_n |= ((inf >> (8 + bit)) & 1) << m_n;
    v_n |= 1; // half the bin, for mid-point reconstruction
    let v_n = v_n & keep;
    // Wrapping like the reference's u32 arithmetic: a malformed
    // stream may produce out-of-range v_n, never a panic.
    let val = ((ms_val << 31) | v_n.wrapping_add(2).wrapping_shl(ms.p - 1)) & keep;
    (val, v_n)
}

/// Decodes the `MagSgn` bits of the next two quads of a quad row, fewer at the row's end, from
/// the row's step 1 entries in `row`. `INITIAL` is the first quad row, whose kappa is 1.
#[inline(always)]
#[allow(clippy::inline_always)] // inlined into the fused step loop beside step 1
fn ms_pair<const INITIAL: bool>(
    ms: &mut MagSgn<'_>,
    r: &mut MsRow,
    row: &[u16],
    out: &mut [u32],
    v_n_scratch: &mut [u32],
) -> Result<()> {
    for _ in 0..2 {
        if r.x >= ms.width {
            break;
        }
        let inf = u32::from(row[r.sp]);
        let mut u_q = u32::from(row[r.sp + 1]);
        if !INITIAL {
            // kappa (T.814 eq. 5): 1 unless the quad has two or more
            // significant samples and the row above saw larger exponents.
            let mut gamma = inf & 0xF0;
            gamma &= gamma.wrapping_sub(0x10);
            let emax = v_n_scratch[r.vp] | v_n_scratch[r.vp + 1];
            // E_max - 1. The `| 2` floors the operand at 2, so `ilog2` can
            // never see the zero it would panic on.
            let emax = (emax | 2).ilog2();
            u_q += if gamma != 0 { emax } else { 1 };
        }
        if u_q > ms.mmsbp2 {
            return Err(malformed(0, "U_q exceeds the magnitude bound"));
        }

        let (val, _) = decode_sample(ms, inf, 0, u_q);
        out[r.dp] = val;
        let (val, v_n) = decode_sample(ms, inf, 1, u_q);
        if r.bottom {
            out[r.dp + ms.stride] = val;
        }
        v_n_scratch[r.vp] = r.prev_v_n | v_n;
        r.prev_v_n = 0;
        r.dp += 1;
        r.x += 1;
        if r.x >= ms.width {
            r.vp += 1;
            break;
        }

        let (val, _) = decode_sample(ms, inf, 2, u_q);
        out[r.dp] = val;
        let (val, v_n) = decode_sample(ms, inf, 3, u_q);
        if r.bottom {
            out[r.dp + ms.stride] = val;
        }
        r.prev_v_n = v_n;
        r.dp += 1;
        r.x += 1;
        r.sp += 2;
        r.vp += 1;
    }
    Ok(())
}

/// The 32 bits at bit offset `off` of an unstuffed stream, LSB-first. Bits past the end read as
/// ones, as the `MagSgn` reader feeds once its segment is exhausted.
#[inline]
fn bits_at(clean: &[u8], off: u32) -> u32 {
    let b = (off >> 3) as usize;
    // The stream ends in more than eight bytes of ones, so a word that runs off its end holds
    // only ones. Taking that constant keeps a copy call out of the decode loop.
    let word = clean
        .get(b..)
        .and_then(<[u8]>::first_chunk::<8>)
        .map_or(u64::MAX, |w| u64::from_le_bytes(*w));
    #[allow(clippy::cast_possible_truncation)] // the low 32 of at least 57 valid bits
    {
        (word >> (off & 7)) as u32
    }
}

/// Narrows an in-range `u32` scratch word to `u16`.
#[inline]
#[allow(clippy::cast_possible_truncation)]
fn to_u16(v: u32) -> u16 {
    debug_assert!(u16::try_from(v).is_ok());
    v as u16
}
