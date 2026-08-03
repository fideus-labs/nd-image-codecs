//! SIMD lane for the reversible 5/3 DWT.
//!
//! The scalar [`super::forward_53`] is the conformance oracle; this lane is
//! **bit-identical** to it (differential tests enforce) but restructures the
//! vertical pass to operate on whole rows — contiguous memory the lifting
//! kernels stream through with NEON on aarch64, AVX2 on x86-64 (runtime
//! detected), or an autovectorizable portable loop elsewhere. The horizontal
//! pass reuses the scalar per-row routine, which is already cache-friendly.
//!
//! Mirrors `OpenJPH`'s per-ISA `ojph_transform_*` translation units; lane
//! selection happens once per transform call, never in inner loops.
#![allow(unsafe_code)] // core::arch intrinsics; each block carries a SAFETY note

extern crate alloc;

use alloc::vec;

use ndic_core::Result;

use super::{Scratch, ana_53, check_geometry, level_dims, syn_53};

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
#[cfg(target_arch = "aarch64")]
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
#[cfg(target_arch = "x86_64")]
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

/// Vertical forward lifting over rows 0..h of the region, in place, then
/// de-interleave even/odd rows into the top/bottom halves.
fn vertical_forward(plane: &mut [i32], width: usize, w: usize, h: usize, kern: &Kernels) {
    let nl = h.div_ceil(2);
    let nh = h / 2;
    let row = |r: usize| r * width;

    // Predict all odd rows: d(2i+1) -= (row(2i) + row(2i+2 | mirror)) >> 1.
    for i in 0..nh {
        let up = 2 * i;
        let down = if 2 * i + 2 < h { 2 * i + 2 } else { 2 * i };
        let (above, dst, below) = split_three(plane, row(up), row(2 * i + 1), row(down), w, width);
        (kern.predict_sub)(dst, above, below);
    }
    // Update all even rows: s(2i) += (d(2i-1 | mirror) + d(2i+1 | mirror) + 2) >> 2.
    for i in 0..nl {
        let dl = if i == 0 { 1 } else { 2 * i - 1 };
        let dr = if 2 * i + 1 < h { 2 * i + 1 } else { 2 * nh - 1 };
        let (above, dst, below) = split_three(plane, row(dl), row(2 * i), row(dr), w, width);
        (kern.update_add)(dst, above, below);
    }
    deinterleave_rows(plane, width, w, h);
}

/// Inverse of [`vertical_forward`].
fn vertical_inverse(plane: &mut [i32], width: usize, w: usize, h: usize, kern: &Kernels) {
    interleave_rows(plane, width, w, h);
    let nl = h.div_ceil(2);
    let nh = h / 2;
    let row = |r: usize| r * width;
    for i in 0..nl {
        let dl = if i == 0 { 1 } else { 2 * i - 1 };
        let dr = if 2 * i + 1 < h { 2 * i + 1 } else { 2 * nh - 1 };
        let (above, dst, below) = split_three(plane, row(dl), row(2 * i), row(dr), w, width);
        (kern.update_sub)(dst, above, below);
    }
    for i in 0..nh {
        let up = 2 * i;
        let down = if 2 * i + 2 < h { 2 * i + 2 } else { 2 * i };
        let (above, dst, below) = split_three(plane, row(up), row(2 * i + 1), row(down), w, width);
        (kern.predict_add)(dst, above, below);
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
    let ptr = plane.as_mut_ptr();
    let len = plane.len();
    assert!(a_off + w <= len && d_off + w <= len && b_off + w <= len);
    // SAFETY: the three row ranges are in-bounds (asserted above) and the
    // mutable row is disjoint from both source rows (distinct row offsets,
    // debug-asserted to differ by at least one full stride).
    unsafe {
        (
            core::slice::from_raw_parts(ptr.add(a_off), w),
            core::slice::from_raw_parts_mut(ptr.add(d_off), w),
            core::slice::from_raw_parts(ptr.add(b_off), w),
        )
    }
}

/// Moves even rows to the top half and odd rows to the bottom half.
fn deinterleave_rows(plane: &mut [i32], width: usize, w: usize, h: usize) {
    let nl = h.div_ceil(2);
    let nh = h / 2;
    let mut odd = vec![0i32; nh * w];
    for i in 0..nh {
        odd[i * w..(i + 1) * w]
            .copy_from_slice(&plane[(2 * i + 1) * width..(2 * i + 1) * width + w]);
    }
    for i in 1..nl {
        plane.copy_within(2 * i * width..2 * i * width + w, i * width);
    }
    for i in 0..nh {
        plane[(nl + i) * width..(nl + i) * width + w].copy_from_slice(&odd[i * w..(i + 1) * w]);
    }
}

/// Inverse of [`deinterleave_rows`].
fn interleave_rows(plane: &mut [i32], width: usize, w: usize, h: usize) {
    let nl = h.div_ceil(2);
    let nh = h / 2;
    let mut odd = vec![0i32; nh * w];
    for i in 0..nh {
        odd[i * w..(i + 1) * w].copy_from_slice(&plane[(nl + i) * width..(nl + i) * width + w]);
    }
    for i in (1..nl).rev() {
        plane.copy_within(i * width..i * width + w, 2 * i * width);
    }
    for i in 0..nh {
        plane[(2 * i + 1) * width..(2 * i + 1) * width + w]
            .copy_from_slice(&odd[i * w..(i + 1) * w]);
    }
}

/// SIMD-lane forward 5/3, bit-identical to [`super::forward_53`].
///
/// # Errors
/// [`ndic_core::Error::InvalidArgument`] on geometry mismatch.
pub fn forward_53(plane: &mut [i32], width: usize, height: usize, levels: u8) -> Result<()> {
    check_geometry(plane.len(), width, height)?;
    let k = kernels();
    let mut scratch = Scratch::new(width, height);
    for l in 0..levels {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            break;
        }
        if h > 1 {
            vertical_forward(plane, width, w, h, &k);
        }
        if w > 1 {
            for y in 0..h {
                scratch.gather_row(plane, width, y, w);
                let (sig, low, high) = scratch.split(w);
                ana_53(sig, low, high);
                scratch.scatter_row_split(plane, width, y, w);
            }
        }
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
    let mut scratch = Scratch::new(width, height);
    for l in (0..levels).rev() {
        let (w, h) = level_dims(width, height, l);
        if w == 1 && h == 1 {
            continue;
        }
        if w > 1 {
            for y in 0..h {
                scratch.gather_row_split(plane, width, y, w);
                let (sig, low, high) = scratch.split(w);
                syn_53(low, high, sig);
                scratch.scatter_row(plane, width, y, w);
            }
        }
        if h > 1 {
            vertical_inverse(plane, width, w, h, &k);
        }
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
