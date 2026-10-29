//! SIMD lane for the reversible 5/3 DWT.
//!
//! The scalar [`super::forward_53`] is the conformance oracle; this lane is
//! **bit-identical** to it (differential tests enforce) but restructures the
//! vertical pass to operate on whole rows — contiguous memory the lifting
//! kernels stream through with NEON on aarch64, AVX2 on x86-64 (runtime
//! detected), or an autovectorizable portable loop elsewhere. Each level runs
//! vertical and horizontal lifting as one pass down the rows, so every row is
//! read once while it is in cache instead of once per lifting step. The
//! horizontal step splits each row into its bands and lifts them in place,
//! short bands through the same kernels.
//!
//! Mirrors `OpenJPH`'s per-ISA `ojph_transform_*` translation units; lane
//! selection happens once per transform call, never in inner loops.
//!
//! `unsafe` lives only in the two `core::arch` lanes below, each of which
//! carries its own narrowly scoped `allow(unsafe_code)`. Everything else in
//! this file — the row splitter, the vertical passes, the de/interleave, both
//! public entry points — is covered by the workspace `deny`.

extern crate alloc;

use alloc::vec;

use ndic_core::Result;

use super::{check_geometry, level_dims};

/// A row-lifting kernel: combines two neighbor rows into a destination row.
type RowKernel = fn(dst: &mut [i32], a: &[i32], b: &[i32]);

/// The four 5/3 row kernels of one lane.
#[derive(Clone, Copy)]
struct Kernels {
    /// `dst -= (a + b) >> 1` — forward predict.
    predict_sub: RowKernel,
    /// `dst += (a + b + 2) >> 2` — forward update.
    update_add: RowKernel,
    /// `dst -= (a + b + 2) >> 2` — inverse update.
    update_sub: RowKernel,
    /// `dst += (a + b) >> 1` — inverse predict.
    predict_add: RowKernel,
}

/// Portable kernels; the chunked loops autovectorize on most targets.
#[cfg_attr(target_arch = "aarch64", allow(dead_code))] // NEON is baseline there
mod portable {
    pub fn predict_sub(dst: &mut [i32], a: &[i32], b: &[i32]) {
        for ((d, &x), &y) in dst.iter_mut().zip(a).zip(b) {
            *d -= (x + y) >> 1;
        }
    }
    pub fn update_add(dst: &mut [i32], a: &[i32], b: &[i32]) {
        for ((d, &x), &y) in dst.iter_mut().zip(a).zip(b) {
            *d += (x + y + 2) >> 2;
        }
    }
    pub fn update_sub(dst: &mut [i32], a: &[i32], b: &[i32]) {
        for ((d, &x), &y) in dst.iter_mut().zip(a).zip(b) {
            *d -= (x + y + 2) >> 2;
        }
    }
    pub fn predict_add(dst: &mut [i32], a: &[i32], b: &[i32]) {
        for ((d, &x), &y) in dst.iter_mut().zip(a).zip(b) {
            *d += (x + y) >> 1;
        }
    }
    pub const KERNELS: super::Kernels = super::Kernels {
        predict_sub,
        update_add,
        update_sub,
        predict_add,
    };
}

/// NEON kernels (aarch64 baseline — always available).
///
/// `unsafe` is unavoidable here: `vld1q_s32` / `vst1q_s32` take raw pointers,
/// and `core::simd` — the only safe alternative — is still unstable in 1.98.
/// The scope is this module and nothing else.
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
mod neon {
    use core::arch::aarch64::{
        int32x4_t, vaddq_s32, vdupq_n_s32, vld1q_s32, vshrq_n_s32, vst1q_s32, vsubq_s32,
    };

    #[inline]
    unsafe fn rows(
        dst: &mut [i32],
        a: &[i32],
        b: &[i32],
        step: impl Fn(int32x4_t, int32x4_t, int32x4_t) -> int32x4_t,
        tail: impl Fn(i32, i32, i32) -> i32,
    ) {
        let len = dst.len();
        let mut i = 0;
        // SAFETY: NEON is part of the aarch64 baseline ABI; all loads and
        // stores stay within `..len - 3` of equally sized slices.
        unsafe {
            while i + 4 <= len {
                let vd = vld1q_s32(dst.as_ptr().add(i));
                let va = vld1q_s32(a.as_ptr().add(i));
                let vb = vld1q_s32(b.as_ptr().add(i));
                vst1q_s32(dst.as_mut_ptr().add(i), step(vd, va, vb));
                i += 4;
            }
        }
        while i < len {
            dst[i] = tail(dst[i], a[i], b[i]);
            i += 1;
        }
    }

    pub fn predict_sub(dst: &mut [i32], a: &[i32], b: &[i32]) {
        // SAFETY: see `rows`.
        unsafe {
            rows(
                dst,
                a,
                b,
                |d, x, y| vsubq_s32(d, vshrq_n_s32::<1>(vaddq_s32(x, y))),
                |d, x, y| d - ((x + y) >> 1),
            );
        }
    }
    pub fn update_add(dst: &mut [i32], a: &[i32], b: &[i32]) {
        // SAFETY: see `rows`.
        unsafe {
            rows(
                dst,
                a,
                b,
                |d, x, y| {
                    vaddq_s32(
                        d,
                        vshrq_n_s32::<2>(vaddq_s32(vaddq_s32(x, y), vdupq_n_s32(2))),
                    )
                },
                |d, x, y| d + ((x + y + 2) >> 2),
            );
        }
    }
    pub fn update_sub(dst: &mut [i32], a: &[i32], b: &[i32]) {
        // SAFETY: see `rows`.
        unsafe {
            rows(
                dst,
                a,
                b,
                |d, x, y| {
                    vsubq_s32(
                        d,
                        vshrq_n_s32::<2>(vaddq_s32(vaddq_s32(x, y), vdupq_n_s32(2))),
                    )
                },
                |d, x, y| d - ((x + y + 2) >> 2),
            );
        }
    }
    pub fn predict_add(dst: &mut [i32], a: &[i32], b: &[i32]) {
        // SAFETY: see `rows`.
        unsafe {
            rows(
                dst,
                a,
                b,
                |d, x, y| vaddq_s32(d, vshrq_n_s32::<1>(vaddq_s32(x, y))),
                |d, x, y| d + ((x + y) >> 1),
            );
        }
    }
    pub const KERNELS: super::Kernels = super::Kernels {
        predict_sub,
        update_add,
        update_sub,
        predict_add,
    };
}

/// AVX2 kernels (x86-64, runtime detected).
///
/// `unsafe` is unavoidable here for the same reason as [`neon`], plus the
/// `#[target_feature(enable = "avx2")]` contract that callers must satisfy by
/// runtime detection. The scope is this module and nothing else.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
mod avx2 {
    use core::arch::x86_64::{
        __m256i, _mm256_add_epi32, _mm256_loadu_si256, _mm256_set1_epi32, _mm256_srai_epi32,
        _mm256_storeu_si256, _mm256_sub_epi32,
    };

    #[inline]
    #[target_feature(enable = "avx2")]
    unsafe fn rows(
        dst: &mut [i32],
        a: &[i32],
        b: &[i32],
        step: impl Fn(__m256i, __m256i, __m256i) -> __m256i,
        tail: impl Fn(i32, i32, i32) -> i32,
    ) {
        let len = dst.len();
        let mut i = 0;
        // SAFETY: caller guarantees AVX2; unaligned intrinsics; all loads
        // and stores stay within `..len - 7` of equally sized slices.
        unsafe {
            while i + 8 <= len {
                let vd = _mm256_loadu_si256(dst.as_ptr().add(i).cast());
                let va = _mm256_loadu_si256(a.as_ptr().add(i).cast());
                let vb = _mm256_loadu_si256(b.as_ptr().add(i).cast());
                _mm256_storeu_si256(dst.as_mut_ptr().add(i).cast(), step(vd, va, vb));
                i += 8;
            }
        }
        while i < len {
            dst[i] = tail(dst[i], a[i], b[i]);
            i += 1;
        }
    }

    macro_rules! kernel {
        ($name:ident, $step:expr, $tail:expr) => {
            pub fn $name(dst: &mut [i32], a: &[i32], b: &[i32]) {
                // SAFETY: selected only after is_x86_feature_detected!("avx2").
                unsafe { rows(dst, a, b, $step, $tail) }
            }
        };
    }
    kernel!(
        predict_sub,
        |d, x, y| _mm256_sub_epi32(d, _mm256_srai_epi32::<1>(_mm256_add_epi32(x, y))),
        |d, x, y| d - ((x + y) >> 1)
    );
    kernel!(
        update_add,
        |d, x, y| _mm256_add_epi32(
            d,
            _mm256_srai_epi32::<2>(_mm256_add_epi32(
                _mm256_add_epi32(x, y),
                _mm256_set1_epi32(2)
            ))
        ),
        |d, x, y| d + ((x + y + 2) >> 2)
    );
    kernel!(
        update_sub,
        |d, x, y| _mm256_sub_epi32(
            d,
            _mm256_srai_epi32::<2>(_mm256_add_epi32(
                _mm256_add_epi32(x, y),
                _mm256_set1_epi32(2)
            ))
        ),
        |d, x, y| d - ((x + y + 2) >> 2)
    );
    kernel!(
        predict_add,
        |d, x, y| _mm256_add_epi32(d, _mm256_srai_epi32::<1>(_mm256_add_epi32(x, y))),
        |d, x, y| d + ((x + y) >> 1)
    );
    pub const KERNELS: super::Kernels = super::Kernels {
        predict_sub,
        update_add,
        update_sub,
        predict_add,
    };
}

/// Selects the best lane for this machine (resolved per call, cached by the
/// feature-detection macro; never queried in inner loops).
fn kernels() -> Kernels {
    #[cfg(target_arch = "aarch64")]
    {
        neon::KERNELS
    }
    #[cfg(all(target_arch = "x86_64", feature = "std"))]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            avx2::KERNELS
        } else {
            portable::KERNELS
        }
    }
    #[cfg(not(any(target_arch = "aarch64", all(target_arch = "x86_64", feature = "std"))))]
    {
        portable::KERNELS
    }
}

/// The active lane's name, for diagnostics and benchmarks.
#[must_use]
pub fn lane_name() -> &'static str {
    #[cfg(target_arch = "aarch64")]
    {
        "neon"
    }
    #[cfg(all(target_arch = "x86_64", feature = "std"))]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            "avx2"
        } else {
            "portable"
        }
    }
    #[cfg(not(any(target_arch = "aarch64", all(target_arch = "x86_64", feature = "std"))))]
    {
        "portable"
    }
}

/// Bands at least this long lift through plain indexed loops, which LLVM vectorizes 16 samples at
/// a time while reusing each loaded vector for both neighbors. A shorter band would leave most of
/// such a loop to its scalar remainder, so it lifts through the lane's 4-wide row kernel instead.
const LONG_BAND: usize = 128;

/// One lifting step along a band, `dst[i] = step(dst[i], src[i], src[i + 1])`, where `kernel`
/// computes the same step on short bands.
#[inline]
fn lift_band(dst: &mut [i32], src: &[i32], kernel: RowKernel, step: impl Fn(i32, i32, i32) -> i32) {
    let len = dst.len();
    let src = &src[..=len];
    if len >= LONG_BAND {
        for i in 0..len {
            dst[i] = step(dst[i], src[i], src[i + 1]);
        }
    } else {
        kernel(dst, &src[..len], &src[1..]);
    }
}

/// Horizontal 5/3 analysis of `src` into `dst` as `[low | high]`, bit-identical to
/// [`super::ana_53`]. The samples split into their bands once, and both lifting steps then run on
/// the contiguous bands, so no load is strided.
fn analyze_row(kern: &Kernels, src: &[i32], dst: &mut [i32]) {
    let n = src.len();
    if n == 1 {
        dst[0] = src[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    let (low, high) = dst[..n].split_at_mut(nl);
    let (pairs, odd) = src.as_chunks::<2>();
    for ((l, h), &[even, odd]) in low.iter_mut().zip(high.iter_mut()).zip(pairs) {
        *l = even;
        *h = odd;
    }
    if let [last] = odd {
        low[nh] = *last;
    }
    // Predict. Only an even-length signal's last high sample lacks a right neighbor, and it
    // mirrors `low[nh - 1]`.
    lift_band(&mut high[..nl - 1], low, kern.predict_sub, |d, a, b| {
        d - ((a + b) >> 1)
    });
    if nh == nl {
        high[nh - 1] -= (low[nh - 1] + low[nh - 1]) >> 1;
    }
    // Update. The first low sample mirrors `high[0]`, and an odd-length signal's last mirrors
    // `high[nh - 1]`.
    low[0] += (high[0] + high[0] + 2) >> 2;
    lift_band(&mut low[1..nh], high, kern.update_add, |d, a, b| {
        d + ((a + b + 2) >> 2)
    });
    if nl > nh {
        low[nh] += (high[nh - 1] + high[nh - 1] + 2) >> 2;
    }
}

/// Inverse of [`analyze_row`], bit-identical to [`super::syn_53`]. Both steps are undone in place
/// on the `[low | high]` bands of `src`, then the bands interleave into `dst`.
fn synthesize_row(kern: &Kernels, src: &mut [i32], dst: &mut [i32]) {
    let n = dst.len();
    if n == 1 {
        dst[0] = src[0];
        return;
    }
    let nl = n.div_ceil(2);
    let nh = n / 2;
    let (low, high) = src[..n].split_at_mut(nl);
    // Undo update, mirroring as in `analyze_row`.
    low[0] -= (high[0] + high[0] + 2) >> 2;
    lift_band(&mut low[1..nh], high, kern.update_sub, |d, a, b| {
        d - ((a + b + 2) >> 2)
    });
    if nl > nh {
        low[nh] -= (high[nh - 1] + high[nh - 1] + 2) >> 2;
    }
    // Undo predict.
    lift_band(&mut high[..nl - 1], low, kern.predict_add, |d, a, b| {
        d + ((a + b) >> 1)
    });
    if nh == nl {
        high[nh - 1] += (low[nh - 1] + low[nh - 1]) >> 1;
    }
    let (pairs, odd) = dst.as_chunks_mut::<2>();
    for ((pair, &l), &h) in pairs.iter_mut().zip(low.iter()).zip(high.iter()) {
        *pair = [l, h];
    }
    if let [last] = odd {
        *last = low[nh];
    }
}

/// One forward level over the `w x h` region at the plane's top left, vertical then horizontal,
/// in one pass down the rows. Pair `j` predicts detail row `2j + 1` from the original even rows
/// around it, updates even row `2j` from the now-final details around it, and each finished row
/// leaves at once through the horizontal analysis while it is still in cache. Low row `j` lands in
/// place, on a row the pass has already read past. High rows wait in `high`, since their home in
/// the bottom half is still unread input.
fn forward_level(
    plane: &mut [i32],
    width: usize,
    (w, h): (usize, usize),
    kern: &Kernels,
    high: &mut [i32],
    row0: &mut [i32],
) {
    let row = |r: usize| r * width;
    // Horizontal analysis of `src` into `dst` as [low | high].
    let analyze = |src: &[i32], dst: &mut [i32]| analyze_row(kern, src, dst);
    if h == 1 {
        if w > 1 {
            analyze(&plane[..w], &mut row0[..w]);
            plane[..w].copy_from_slice(&row0[..w]);
        }
        return;
    }
    let nl = h.div_ceil(2);
    let nh = h / 2;
    for j in 0..nl {
        // Predict: d(2j+1) -= (row(2j) + row(2j+2 | mirror)) >> 1, before row 2j is updated.
        if 2 * j + 1 < h {
            let down = if 2 * j + 2 < h { 2 * j + 2 } else { 2 * j };
            let (above, dst, below) =
                split_three(plane, row(2 * j), row(2 * j + 1), row(down), w, width);
            (kern.predict_sub)(dst, above, below);
        }
        // Update: s(2j) += (d(2j-1 | mirror) + d(2j+1 | mirror) + 2) >> 2.
        let dl = if j == 0 { 1 } else { 2 * j - 1 };
        let dr = if 2 * j + 1 < h { 2 * j + 1 } else { 2 * nh - 1 };
        let (above, dst, below) = split_three(plane, row(dl), row(2 * j), row(dr), w, width);
        (kern.update_add)(dst, above, below);
        // Detail row 2j - 1 has served its last update.
        if j > 0 {
            analyze(
                &plane[row(2 * j - 1)..row(2 * j - 1) + w],
                &mut high[(j - 1) * w..j * w],
            );
        }
        if j == 0 {
            analyze(&plane[..w], &mut row0[..w]);
            plane[..w].copy_from_slice(&row0[..w]);
        } else {
            let (head, tail) = plane.split_at_mut(row(2 * j));
            analyze(&tail[..w], &mut head[row(j)..row(j) + w]);
        }
    }
    if nh == nl {
        analyze(
            &plane[row(2 * nh - 1)..row(2 * nh - 1) + w],
            &mut high[(nh - 1) * w..nh * w],
        );
    }
    for i in 0..nh {
        plane[row(nl + i)..row(nl + i) + w].copy_from_slice(&high[i * w..(i + 1) * w]);
    }
}

/// Inverse of [`forward_level`], also one pass. High rows go through the horizontal synthesis
/// into `high` first. Pairs then run from the bottom up, so output rows `2j` and `2j + 1` only
/// overwrite low rows already consumed: even row `2j` is synthesized from low row `j` and the
/// update undone from the details around it, then odd row `2j + 1` is predicted back from the
/// finished even rows around it.
fn inverse_level(
    plane: &mut [i32],
    width: usize,
    (w, h): (usize, usize),
    kern: &Kernels,
    high: &mut [i32],
    row0: &mut [i32],
) {
    let row = |r: usize| r * width;
    // Horizontal synthesis of `src`, split as [low | high] and lifted in place, into `dst`.
    let synthesize = |src: &mut [i32], dst: &mut [i32]| synthesize_row(kern, src, dst);
    if h == 1 {
        if w > 1 {
            synthesize(&mut plane[..w], &mut row0[..w]);
            plane[..w].copy_from_slice(&row0[..w]);
        }
        return;
    }
    let nl = h.div_ceil(2);
    let nh = h / 2;
    for i in 0..nh {
        synthesize(
            &mut plane[row(nl + i)..row(nl + i) + w],
            &mut high[i * w..(i + 1) * w],
        );
    }
    for j in (0..nl).rev() {
        if j == 0 {
            synthesize(&mut plane[..w], &mut row0[..w]);
            plane[..w].copy_from_slice(&row0[..w]);
        } else {
            let (head, tail) = plane.split_at_mut(row(2 * j));
            synthesize(&mut head[row(j)..row(j) + w], &mut tail[..w]);
        }
        // Undo update: s(2j) -= (d(2j-1 | mirror) + d(2j+1 | mirror) + 2) >> 2, where detail row
        // 2i + 1 waits in `high` row i.
        let dl = j.saturating_sub(1);
        let dr = if 2 * j + 1 < h { j } else { nh - 1 };
        (kern.update_sub)(
            &mut plane[row(2 * j)..row(2 * j) + w],
            &high[dl * w..(dl + 1) * w],
            &high[dr * w..(dr + 1) * w],
        );
        // Undo predict: d(2j+1) += (row(2j) + row(2j+2 | mirror)) >> 1, once both are final.
        if 2 * j + 1 < h {
            let down = if 2 * j + 2 < h { 2 * j + 2 } else { 2 * j };
            plane[row(2 * j + 1)..row(2 * j + 1) + w].copy_from_slice(&high[j * w..(j + 1) * w]);
            let (above, dst, below) =
                split_three(plane, row(2 * j), row(2 * j + 1), row(down), w, width);
            (kern.predict_add)(dst, above, below);
        }
    }
}

/// Borrows three `w`-long rows at distinct offsets: `(a, dst, b)`.
/// `a` and `b` may alias each other (mirror rows) but never `dst`.
fn split_three(
    plane: &mut [i32],
    a_off: usize,
    d_off: usize,
    b_off: usize,
    w: usize,
    width: usize,
) -> (&[i32], &mut [i32], &[i32]) {
    debug_assert!(a_off != d_off && b_off != d_off);
    debug_assert!(a_off.abs_diff(d_off) >= width && b_off.abs_diff(d_off) >= width);
    let len = plane.len();
    assert!(a_off + w <= len && d_off + w <= len && b_off + w <= len);
    // Cut the mutable row out of the plane first. Every source row starts at
    // least one full stride away from it, so each lands wholly in `before` or
    // wholly in `after` — which is what makes the split expressible safely.
    // `a` and `b` may still be the same row (the mirror cases); they only have
    // to be disjoint from `dst`, and two shared borrows of one piece are fine.
    let (before, rest) = plane.split_at_mut(d_off);
    let (dst, after) = rest.split_at_mut(w);
    let (before, after): (&[i32], &[i32]) = (before, after);
    let a = if a_off < d_off {
        &before[a_off..a_off + w]
    } else {
        &after[a_off - d_off - w..a_off - d_off]
    };
    let b = if b_off < d_off {
        &before[b_off..b_off + w]
    } else {
        &after[b_off - d_off - w..b_off - d_off]
    };
    (a, dst, b)
}

/// SIMD-lane forward 5/3, bit-identical to [`super::forward_53`].
///
/// # Errors
/// [`ndic_core::Error::InvalidArgument`] on geometry mismatch.
pub fn forward_53(plane: &mut [i32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let k = kernels();
    // The largest level's high rows, reused by every level below it.
    let mut high = vec![0i32; (height / 2) * width];
    let mut row0 = vec![0i32; width];
    for l in 0..levels {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            break;
        }
        forward_level(plane, width, (w, h), &k, &mut high, &mut row0);
    }
    Ok(())
}

/// SIMD-lane inverse 5/3, bit-identical to [`super::inverse_53`].
///
/// # Errors
/// [`ndic_core::Error::InvalidArgument`] on geometry mismatch.
pub fn inverse_53(plane: &mut [i32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let k = kernels();
    let mut high = vec![0i32; (height / 2) * width];
    let mut row0 = vec![0i32; width];
    for l in (0..levels).rev() {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            continue;
        }
        inverse_level(plane, width, (w, h), &k, &mut high, &mut row0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg_plane(w: usize, h: usize, seed: u64) -> alloc::vec::Vec<i32> {
        let mut state = seed | 1;
        (0..w * h)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                {
                    ((state >> 33) & 0x1FFFF) as i32 - 0x10000
                }
            })
            .collect()
    }

    #[test]
    fn matches_scalar_bit_exactly() {
        for (w, h) in [
            (1usize, 1usize),
            (2, 2),
            (3, 5),
            (7, 1),
            (1, 9),
            (64, 64),
            (65, 33),
            (256, 100),
            (129, 77),
            (300, 9),
            (517, 6),
        ] {
            for levels in 0..=5u8 {
                let plane = lcg_plane(w, h, (w * 31 + h) as u64);
                let mut scalar = plane.clone();
                super::super::forward_53(&mut scalar, w, h, levels).unwrap();
                let mut lane = plane.clone();
                forward_53(&mut lane, w, h, levels).unwrap();
                assert_eq!(lane, scalar, "forward {w}x{h} L{levels} ({})", lane_name());

                let mut back = lane.clone();
                inverse_53(&mut back, w, h, levels).unwrap();
                assert_eq!(back, plane, "inverse {w}x{h} L{levels}");

                // Cross: SIMD inverse of the scalar forward.
                let mut cross = scalar.clone();
                inverse_53(&mut cross, w, h, levels).unwrap();
                assert_eq!(cross, plane, "cross inverse {w}x{h} L{levels}");
            }
        }
    }
}
