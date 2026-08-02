//! 1D integer lifting kernels: `delta`, `haar`, and `lift53`.
//!
//! Each kernel transforms one contiguous signal (an axis line, or one bounded
//! group within it) in place. The lifting kinds (`haar`, `lift53`) write their
//! output in **subband order** — the approximation band `s` first, then the
//! detail band `d` — and recurse `levels` times on the approximation band.
//! This deinterleaved coefficient ordering is part of the `nd_lift` `0.1`
//! specification (`docs/architecture/nd-transform.md`).
//!
//! Boundary rule: whole-sample symmetric (mirror) extension, matching ITU-T
//! T.800 Annex F. On the interleaved grid `x[k] = x[2(n−1) − k]` for `k > n−1`
//! and `x[−k] = x[k]`, which induces `d[−1] = d[0]` and `d[nd] = d[nd−1]` on
//! the detail grid. Odd lengths need no padding; signals shorter than two
//! samples are left untouched.
//!
//! Overflow: callers guarantee (via the [`crate::forward`] budget check) that
//! every expression below fits the [`PlaneSample`] type, so the kernels use
//! plain arithmetic.

use crate::sample::PlaneSample;

/// Enough level slots for any band: a band of length ≥ 2 halves at most
/// `usize::BITS − 1` times.
const MAX_LEVELS: usize = usize::BITS as usize;

/// `r[i] = x[i] − x[i−1]`, `r[0] = x[0]`.
pub(crate) fn delta_forward<T: PlaneSample>(x: &mut [T]) {
    for i in (1..x.len()).rev() {
        let prev = x[i - 1];
        x[i] -= prev;
    }
}

/// Inverse of [`delta_forward`]: the running sum.
pub(crate) fn delta_inverse<T: PlaneSample>(x: &mut [T]) {
    for i in 1..x.len() {
        let prev = x[i - 1];
        x[i] += prev;
    }
}

/// One reversible Haar level: `d = x₁ − x₀`, `s = x₀ + ⌊d/2⌋`, subband order.
fn haar_forward_level<T: PlaneSample>(x: &mut [T], tmp: &mut [T]) {
    let n = x.len();
    let ns = n.div_ceil(2);
    for i in 0..n / 2 {
        let x0 = x[2 * i];
        let d = x[2 * i + 1] - x0;
        tmp[i] = x0 + (d >> 1);
        tmp[ns + i] = d;
    }
    if n % 2 == 1 {
        tmp[ns - 1] = x[n - 1];
    }
    x.copy_from_slice(&tmp[..n]);
}

/// Inverse of one Haar level: `x₀ = s − ⌊d/2⌋`, `x₁ = x₀ + d`.
fn haar_inverse_level<T: PlaneSample>(x: &mut [T], tmp: &mut [T]) {
    let n = x.len();
    let ns = n.div_ceil(2);
    for i in 0..n / 2 {
        let d = x[ns + i];
        let x0 = x[i] - (d >> 1);
        tmp[2 * i] = x0;
        tmp[2 * i + 1] = x0 + d;
    }
    if n % 2 == 1 {
        tmp[n - 1] = x[ns - 1];
    }
    x.copy_from_slice(&tmp[..n]);
}

/// The mirrored right even neighbour `x[2i+2]` on a grid of length `n`.
#[inline]
fn right_even(k: usize, n: usize) -> usize {
    if k < n { k } else { 2 * n - 2 - k }
}

/// One 5/3 level: predict then update with the `0.1` rounding rule,
/// subband order.
///
/// ```text
/// d[i] = x[2i+1] − ⌊(x[2i] + x[2i+2] + 1) / 2⌋
/// s[i] = x[2i]   + ⌊(d[i−1] + d[i] + 2) / 4⌋
/// ```
fn lift53_forward_level<T: PlaneSample>(x: &mut [T], tmp: &mut [T]) {
    let n = x.len();
    let ns = n.div_ceil(2);
    let nd = n / 2;
    for i in 0..nd {
        let left = x[2 * i];
        let right = x[right_even(2 * i + 2, n)];
        tmp[ns + i] = x[2 * i + 1] - ((left + right + T::ONE) >> 1);
    }
    for i in 0..ns {
        // Symmetric extension on the detail grid: d[−1] = d[0], d[nd] = d[nd−1].
        let dl = tmp[ns + i.saturating_sub(1)];
        let dr = tmp[ns + i.min(nd - 1)];
        tmp[i] = x[2 * i] + ((dl + dr + T::TWO) >> 2);
    }
    x.copy_from_slice(&tmp[..n]);
}

/// Inverse of one 5/3 level: undo update (evens), then undo predict (odds).
fn lift53_inverse_level<T: PlaneSample>(x: &mut [T], tmp: &mut [T]) {
    let n = x.len();
    let ns = n.div_ceil(2);
    let nd = n / 2;
    let (s, d) = x.split_at(ns);
    for i in 0..ns {
        let dl = d[i.saturating_sub(1)];
        let dr = d[i.min(nd - 1)];
        tmp[2 * i] = s[i] - ((dl + dr + T::TWO) >> 2);
    }
    for i in 0..nd {
        let left = tmp[2 * i];
        let right = tmp[right_even(2 * i + 2, n)];
        tmp[2 * i + 1] = d[i] + ((left + right + T::ONE) >> 1);
    }
    x.copy_from_slice(&tmp[..n]);
}

/// The dyadic band lengths a `levels`-deep decomposition of `n` samples
/// actually visits (a level needs at least two samples).
fn band_lengths(n: usize, levels: u8) -> ([usize; MAX_LEVELS], usize) {
    let mut bands = [0usize; MAX_LEVELS];
    let mut count = 0;
    let mut m = n;
    for _ in 0..levels {
        if m < 2 {
            break;
        }
        bands[count] = m;
        count += 1;
        m = m.div_ceil(2);
    }
    (bands, count)
}

/// Apply `levels` forward levels of `level_fn`, recursing on the `s` band.
fn multi_level_forward<T: PlaneSample>(
    x: &mut [T],
    tmp: &mut [T],
    levels: u8,
    level_fn: fn(&mut [T], &mut [T]),
) {
    let (bands, count) = band_lengths(x.len(), levels);
    for &m in &bands[..count] {
        level_fn(&mut x[..m], &mut tmp[..m]);
    }
}

/// Undo `levels` levels of `level_fn`, deepest band first.
fn multi_level_inverse<T: PlaneSample>(
    x: &mut [T],
    tmp: &mut [T],
    levels: u8,
    level_fn: fn(&mut [T], &mut [T]),
) {
    let (bands, count) = band_lengths(x.len(), levels);
    for &m in bands[..count].iter().rev() {
        level_fn(&mut x[..m], &mut tmp[..m]);
    }
}

/// Multi-level reversible Haar, subband order.
pub(crate) fn haar_forward<T: PlaneSample>(x: &mut [T], tmp: &mut [T], levels: u8) {
    multi_level_forward(x, tmp, levels, haar_forward_level);
}

/// Inverse of [`haar_forward`].
pub(crate) fn haar_inverse<T: PlaneSample>(x: &mut [T], tmp: &mut [T], levels: u8) {
    multi_level_inverse(x, tmp, levels, haar_inverse_level);
}

/// Multi-level 5/3 lifting, subband order.
pub(crate) fn lift53_forward<T: PlaneSample>(x: &mut [T], tmp: &mut [T], levels: u8) {
    multi_level_forward(x, tmp, levels, lift53_forward_level);
}

/// Inverse of [`lift53_forward`].
pub(crate) fn lift53_inverse<T: PlaneSample>(x: &mut [T], tmp: &mut [T], levels: u8) {
    multi_level_inverse(x, tmp, levels, lift53_inverse_level);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn roundtrip(
        input: &[i32],
        levels: u8,
        fwd: fn(&mut [i32], &mut [i32], u8),
        inv: fn(&mut [i32], &mut [i32], u8),
    ) -> Vec<i32> {
        let mut x = input.to_vec();
        let mut tmp = vec![0; input.len()];
        fwd(&mut x, &mut tmp, levels);
        let coeffs = x.clone();
        inv(&mut x, &mut tmp, levels);
        assert_eq!(x, input, "round-trip must be exact");
        coeffs
    }

    #[test]
    fn delta_ramp() {
        let mut x = vec![3, 4, 5, 6, 7];
        delta_forward(&mut x);
        assert_eq!(x, vec![3, 1, 1, 1, 1]);
        delta_inverse(&mut x);
        assert_eq!(x, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn haar_dc_is_sparse() {
        // A constant signal: approximation carries the value, details vanish.
        let coeffs = roundtrip(&[9, 9, 9, 9, 9, 9], 1, haar_forward, haar_inverse);
        assert_eq!(coeffs, vec![9, 9, 9, 0, 0, 0]);
    }

    #[test]
    fn haar_pair_math() {
        // d = 7 − 2 = 5, s = 2 + ⌊5/2⌋ = 4; then the negative pair:
        // d = −7 − (−2) = −5, s = −2 + ⌊−5/2⌋ = −5.
        let coeffs = roundtrip(&[2, 7, -2, -7], 1, haar_forward, haar_inverse);
        assert_eq!(coeffs, vec![4, -5, 5, -5]);
    }

    #[test]
    fn lift53_dc_is_sparse() {
        // Two levels: DC survives only in the deepest approximation band.
        let coeffs = roundtrip(&[5, 5, 5, 5, 5, 5, 5], 2, lift53_forward, lift53_inverse);
        assert_eq!(coeffs, vec![5, 5, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn lift53_ramp_details_vanish_interior() {
        // Interior 5/3 details of a ramp are exactly zero; only the mirrored
        // right edge (n even) leaves d = 1.
        let input: Vec<i32> = (0..8).collect();
        let coeffs = roundtrip(&input, 1, lift53_forward, lift53_inverse);
        assert_eq!(coeffs, vec![0, 2, 4, 6, 0, 0, 0, 1]);
    }

    #[test]
    fn lift53_impulse_analytic() {
        // x = [0, 4, 0, 0]: d0 = 4 − ⌊(0+0+1)/2⌋ = 4, d1 = 0,
        // s0 = 0 + ⌊(4+4+2)/4⌋ = 2, s1 = 0 + ⌊(4+0+2)/4⌋ = 1.
        let coeffs = roundtrip(&[0, 4, 0, 0], 1, lift53_forward, lift53_inverse);
        assert_eq!(coeffs, vec![2, 1, 4, 0]);
    }

    #[test]
    fn odd_singleton_and_empty_lengths() {
        for len in [0usize, 1, 2, 3, 5, 7, 9] {
            let input: Vec<i32> = (0..len)
                .map(|i| i32::try_from(i).unwrap() * 3 - 4)
                .collect();
            roundtrip(&input, 3, haar_forward, haar_inverse);
            roundtrip(&input, 3, lift53_forward, lift53_inverse);
            let mut x = input.clone();
            delta_forward(&mut x);
            delta_inverse(&mut x);
            assert_eq!(x, input);
        }
    }

    #[test]
    fn levels_beyond_dyadic_depth_saturate() {
        // 255 levels on 6 samples applies only ⌈log₂⌉ levels and stays exact.
        roundtrip(&[1, -2, 3, -4, 5, -6], 255, lift53_forward, lift53_inverse);
    }
}
