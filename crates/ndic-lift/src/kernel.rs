//! Row-wise integer lifting kernels: `delta`, `haar`, and `lift53`.
//!
//! Each kernel transforms one contiguous **segment slab** in place: `m` axis
//! samples × `inner` interleaved signals, where `inner` is the product of the
//! chunk extents *after* the transformed axis. Sample `k` of signal `j` lives
//! at `slab[k * inner + j]` — a chunk laid out as `(outer, len, inner)` needs
//! no gather/scatter at all, and every lifting step becomes a streaming
//! elementwise operation over contiguous rows of `inner` samples, which the
//! compiler auto-vectorizes. `inner == 1` is the plain 1D (scalar) case; the
//! per-sample operation order is identical for every `inner`, so the output
//! is bit-identical regardless of layout.
//!
//! The lifting kinds (`haar`, `lift53`) write their output in **subband
//! order** — the approximation band `s` first, then the detail band `d` —
//! and recurse `levels` times on the approximation band. This deinterleaved
//! coefficient ordering is part of the `nd_lift` `0.1` specification
//! (`docs/architecture/nd-transform.md`).
//!
//! Boundary rule: whole-sample symmetric (mirror) extension, matching ITU-T
//! T.800 Annex F. On the interleaved grid `x[k] = x[2(n−1) − k]` for `k > n−1`
//! and `x[−k] = x[k]`, which induces `d[−1] = d[0]` and `d[nd] = d[nd−1]` on
//! the detail grid. Odd lengths need no padding; signals shorter than two
//! samples are left untouched.
//!
//! Overflow: the two directions have different guarantees, so they use
//! different arithmetic.
//!
//! The **forward** kernels run behind the [`crate::forward`] budget check,
//! which has already refused any input whose coefficients or lifting
//! intermediates could leave the [`PlaneSample`] range. They use plain
//! arithmetic, so a bug that let the budget drift out of step with the depth
//! rule trips an overflow panic in the test suite instead of silently
//! producing wrong coefficients.
//!
//! The **inverse** kernels have no such guarantee: their input is whatever a
//! storage read produced, and a corrupt or hostile coefficient plane can hold
//! any in-range value. They use the wrapping operations on [`PlaneSample`],
//! which are the same machine instructions but leave overflow *defined* —
//! decoding untrusted bytes must not depend on the profile's `overflow-checks`
//! setting to avoid a panic. Coefficients [`crate::forward`] actually produced
//! never reach a wrap, so this changes no `0.1` output.
//!
//! Each row primitive below belongs to exactly one direction, which is what
//! keeps the split honest — no operation is shared between the two.

use crate::sample::PlaneSample;

/// The row of axis sample `i`.
#[inline]
fn row<T>(buf: &[T], i: usize, inner: usize) -> &[T] {
    &buf[i * inner..(i + 1) * inner]
}

/// The mutable row of axis sample `i`.
#[inline]
fn row_mut<T>(buf: &mut [T], i: usize, inner: usize) -> &mut [T] {
    &mut buf[i * inner..(i + 1) * inner]
}

// --------------------------------------------------------------------------
// Elementwise row primitives — the auto-vectorized inner loops.
// --------------------------------------------------------------------------

/// `out = a − b`.
#[inline]
fn row_sub<T: PlaneSample>(out: &mut [T], a: &[T], b: &[T]) {
    for ((o, &a), &b) in out.iter_mut().zip(a).zip(b) {
        *o = a - b;
    }
}

/// `out = a + b`.
#[inline]
fn row_add<T: PlaneSample>(out: &mut [T], a: &[T], b: &[T]) {
    for ((o, &a), &b) in out.iter_mut().zip(a).zip(b) {
        *o = a.wrapping_add(b);
    }
}

/// Haar update: `out = x₀ + ⌊d/2⌋`.
#[inline]
fn row_haar_update<T: PlaneSample>(out: &mut [T], x0: &[T], d: &[T]) {
    for ((o, &x0), &d) in out.iter_mut().zip(x0).zip(d) {
        *o = x0 + (d >> 1);
    }
}

/// Haar update undone: `out = s − ⌊d/2⌋`.
#[inline]
fn row_haar_unupdate<T: PlaneSample>(out: &mut [T], s: &[T], d: &[T]) {
    for ((o, &s), &d) in out.iter_mut().zip(s).zip(d) {
        *o = s.wrapping_sub(d >> 1);
    }
}

/// 5/3 predict: `out = odd − ⌊(left + right + 1) / 2⌋`.
#[inline]
fn row_predict<T: PlaneSample>(out: &mut [T], odd: &[T], left: &[T], right: &[T]) {
    for (((o, &x1), &l), &r) in out.iter_mut().zip(odd).zip(left).zip(right) {
        *o = x1 - ((l + r + T::ONE) >> 1);
    }
}

/// 5/3 predict undone: `out = d + ⌊(left + right + 1) / 2⌋`.
#[inline]
fn row_unpredict<T: PlaneSample>(out: &mut [T], d: &[T], left: &[T], right: &[T]) {
    for (((o, &d), &l), &r) in out.iter_mut().zip(d).zip(left).zip(right) {
        *o = d.wrapping_add((l.wrapping_add(r).wrapping_add(T::ONE)) >> 1);
    }
}

/// 5/3 update: `out = even + ⌊(d₋ + d₊ + 2) / 4⌋`.
#[inline]
fn row_update<T: PlaneSample>(out: &mut [T], even: &[T], dl: &[T], dr: &[T]) {
    for (((o, &e), &dl), &dr) in out.iter_mut().zip(even).zip(dl).zip(dr) {
        *o = e + ((dl + dr + T::TWO) >> 2);
    }
}

/// 5/3 update undone: `out = s − ⌊(d₋ + d₊ + 2) / 4⌋`.
#[inline]
fn row_unupdate<T: PlaneSample>(out: &mut [T], s: &[T], dl: &[T], dr: &[T]) {
    for (((o, &s), &dl), &dr) in out.iter_mut().zip(s).zip(dl).zip(dr) {
        *o = s.wrapping_sub((dl.wrapping_add(dr).wrapping_add(T::TWO)) >> 2);
    }
}

// --------------------------------------------------------------------------
// Kernels
// --------------------------------------------------------------------------

/// `r[i] = x[i] − x[i−1]`, `r[0] = x[0]`, in place.
pub(crate) fn delta_forward<T: PlaneSample>(seg: &mut [T], inner: usize) {
    let n = seg.len() / inner;
    for k in (1..n).rev() {
        let (head, tail) = seg.split_at_mut(k * inner);
        row_sub_assign(&mut tail[..inner], &head[(k - 1) * inner..]);
    }
}

/// Inverse of [`delta_forward`]: the running sum.
pub(crate) fn delta_inverse<T: PlaneSample>(seg: &mut [T], inner: usize) {
    let n = seg.len() / inner;
    for k in 1..n {
        let (head, tail) = seg.split_at_mut(k * inner);
        row_add_assign(&mut tail[..inner], &head[(k - 1) * inner..]);
    }
}

/// `out -= a`.
#[inline]
fn row_sub_assign<T: PlaneSample>(out: &mut [T], a: &[T]) {
    for (o, &a) in out.iter_mut().zip(a) {
        *o -= a;
    }
}

/// `out += a`.
#[inline]
fn row_add_assign<T: PlaneSample>(out: &mut [T], a: &[T]) {
    for (o, &a) in out.iter_mut().zip(a) {
        *o = (*o).wrapping_add(a);
    }
}

/// One reversible Haar level: `d = x₁ − x₀`, `s = x₀ + ⌊d/2⌋`, subband order.
fn haar_forward_level<T: PlaneSample>(x: &mut [T], inner: usize, tmp: &mut [T]) {
    let n = x.len() / inner;
    let ns = n.div_ceil(2);
    let (ts, td) = tmp.split_at_mut(ns * inner);
    for i in 0..n / 2 {
        let x0 = row(x, 2 * i, inner);
        let x1 = row(x, 2 * i + 1, inner);
        let d = row_mut(td, i, inner);
        row_sub(d, x1, x0);
        row_haar_update(row_mut(ts, i, inner), x0, row(td, i, inner));
    }
    if n % 2 == 1 {
        row_mut(ts, ns - 1, inner).copy_from_slice(row(x, n - 1, inner));
    }
    x.copy_from_slice(&tmp[..n * inner]);
}

/// Inverse of one Haar level: `x₀ = s − ⌊d/2⌋`, `x₁ = x₀ + d`.
fn haar_inverse_level<T: PlaneSample>(x: &mut [T], inner: usize, tmp: &mut [T]) {
    let n = x.len() / inner;
    let ns = n.div_ceil(2);
    let (s, d) = x.split_at(ns * inner);
    for i in 0..n / 2 {
        let (head, tail) = tmp.split_at_mut((2 * i + 1) * inner);
        let x0 = &mut head[2 * i * inner..];
        row_haar_unupdate(x0, row(s, i, inner), row(d, i, inner));
        row_add(&mut tail[..inner], &head[2 * i * inner..], row(d, i, inner));
    }
    if n % 2 == 1 {
        row_mut(tmp, n - 1, inner).copy_from_slice(row(s, ns - 1, inner));
    }
    x.copy_from_slice(&tmp[..n * inner]);
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
fn lift53_forward_level<T: PlaneSample>(x: &mut [T], inner: usize, tmp: &mut [T]) {
    let n = x.len() / inner;
    // `nd - 1` below needs a non-empty detail band; `band_count` only yields
    // levels of at least two samples, which is the sole caller path.
    debug_assert!(n >= 2, "a 5/3 level needs at least two samples");
    let ns = n.div_ceil(2);
    let nd = n / 2;
    let (ts, td) = tmp.split_at_mut(ns * inner);
    for i in 0..nd {
        row_predict(
            row_mut(td, i, inner),
            row(x, 2 * i + 1, inner),
            row(x, 2 * i, inner),
            row(x, right_even(2 * i + 2, n), inner),
        );
    }
    for i in 0..ns {
        // Symmetric extension on the detail grid: d[−1] = d[0], d[nd] = d[nd−1].
        row_update(
            row_mut(ts, i, inner),
            row(x, 2 * i, inner),
            row(td, i.saturating_sub(1), inner),
            row(td, i.min(nd - 1), inner),
        );
    }
    x.copy_from_slice(&tmp[..n * inner]);
}

/// Inverse of one 5/3 level: undo update (evens), then undo predict (odds).
fn lift53_inverse_level<T: PlaneSample>(x: &mut [T], inner: usize, tmp: &mut [T]) {
    let n = x.len() / inner;
    // Same precondition as the forward level: `nd - 1` must not underflow.
    debug_assert!(n >= 2, "a 5/3 level needs at least two samples");
    let ns = n.div_ceil(2);
    let nd = n / 2;
    let (s, d) = x.split_at(ns * inner);
    for i in 0..ns {
        row_unupdate(
            row_mut(tmp, 2 * i, inner),
            row(s, i, inner),
            row(d, i.saturating_sub(1), inner),
            row(d, i.min(nd - 1), inner),
        );
    }
    for i in 0..nd {
        // The odd row sits between its even neighbours; the mirrored right
        // neighbour (even `n`, last pair) folds back onto the left row.
        let (head, tail) = tmp.split_at_mut((2 * i + 1) * inner);
        let left = &head[2 * i * inner..][..inner];
        let (out, rest) = tail.split_at_mut(inner);
        let right = if 2 * i + 2 < n {
            &rest[..inner]
        } else {
            &head[right_even(2 * i + 2, n) * inner..][..inner]
        };
        row_unpredict(out, row(d, i, inner), left, right);
    }
    x.copy_from_slice(&tmp[..n * inner]);
}

/// How many dyadic levels a `levels`-deep decomposition of `n` samples
/// actually visits: a level needs at least two samples, so the chain stops
/// early once the approximation band is a singleton.
///
/// This is the single definition of the depth rule — the encode-time overflow
/// budget ([`crate::forward`]) propagates exactly this many levels.
pub(crate) fn band_count(n: usize, levels: u8) -> u32 {
    let mut count = 0;
    let mut m = n;
    for _ in 0..levels {
        if m < 2 {
            break;
        }
        count += 1;
        m = m.div_ceil(2);
    }
    count
}

/// The length of band `k` of a decomposition of `n` samples.
///
/// Each level's approximation band is `⌈m/2⌉` of the previous one, and nested
/// ceiling division collapses: `⌈⌈n/2⌉/2⌉ = ⌈n/4⌉`, so band `k` is simply
/// `⌈n / 2ᵏ⌉`. Computing it closed-form keeps the level walk allocation-free
/// — `apply_axis` runs it once per segment.
#[inline]
fn band_len(n: usize, k: u32) -> usize {
    // `band_count` never exceeds `usize::BITS`, so `k < usize::BITS` here.
    debug_assert!(k < usize::BITS);
    n.div_ceil(1usize << k)
}

/// Apply `levels` forward levels of `level_fn`, recursing on the `s` band.
fn multi_level_forward<T: PlaneSample>(
    x: &mut [T],
    inner: usize,
    tmp: &mut [T],
    levels: u8,
    level_fn: fn(&mut [T], usize, &mut [T]),
) {
    let n = x.len() / inner;
    for k in 0..band_count(n, levels) {
        let m = band_len(n, k) * inner;
        level_fn(&mut x[..m], inner, &mut tmp[..m]);
    }
}

/// Undo `levels` levels of `level_fn`, deepest band first.
fn multi_level_inverse<T: PlaneSample>(
    x: &mut [T],
    inner: usize,
    tmp: &mut [T],
    levels: u8,
    level_fn: fn(&mut [T], usize, &mut [T]),
) {
    let n = x.len() / inner;
    for k in (0..band_count(n, levels)).rev() {
        let m = band_len(n, k) * inner;
        level_fn(&mut x[..m], inner, &mut tmp[..m]);
    }
}

/// Multi-level reversible Haar, subband order.
pub(crate) fn haar_forward<T: PlaneSample>(seg: &mut [T], inner: usize, tmp: &mut [T], levels: u8) {
    multi_level_forward(seg, inner, tmp, levels, haar_forward_level);
}

/// Inverse of [`haar_forward`].
pub(crate) fn haar_inverse<T: PlaneSample>(seg: &mut [T], inner: usize, tmp: &mut [T], levels: u8) {
    multi_level_inverse(seg, inner, tmp, levels, haar_inverse_level);
}

/// Multi-level 5/3 lifting, subband order.
pub(crate) fn lift53_forward<T: PlaneSample>(
    seg: &mut [T],
    inner: usize,
    tmp: &mut [T],
    levels: u8,
) {
    multi_level_forward(seg, inner, tmp, levels, lift53_forward_level);
}

/// Inverse of [`lift53_forward`].
pub(crate) fn lift53_inverse<T: PlaneSample>(
    seg: &mut [T],
    inner: usize,
    tmp: &mut [T],
    levels: u8,
) {
    multi_level_inverse(seg, inner, tmp, levels, lift53_inverse_level);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn roundtrip(
        input: &[i32],
        levels: u8,
        fwd: fn(&mut [i32], usize, &mut [i32], u8),
        inv: fn(&mut [i32], usize, &mut [i32], u8),
    ) -> Vec<i32> {
        let mut x = input.to_vec();
        let mut tmp = vec![0; input.len()];
        fwd(&mut x, 1, &mut tmp, levels);
        let coeffs = x.clone();
        inv(&mut x, 1, &mut tmp, levels);
        assert_eq!(x, input, "round-trip must be exact");
        coeffs
    }

    #[test]
    fn delta_ramp() {
        let mut x = vec![3, 4, 5, 6, 7];
        delta_forward(&mut x, 1);
        assert_eq!(x, vec![3, 1, 1, 1, 1]);
        delta_inverse(&mut x, 1);
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
    fn rows_transform_like_independent_signals() {
        // Two interleaved signals (inner = 2) must produce exactly the two
        // independent 1D transforms, whatever the kind.
        let a = [3, 1, 4, 1, 5, 9, 2];
        let b = [-2, 7, -1, 8, -2, 8, -1];
        let interleaved: Vec<i32> = a.iter().zip(&b).flat_map(|(&x, &y)| [x, y]).collect();
        for (fwd, inv) in [
            (
                haar_forward as fn(&mut [i32], usize, &mut [i32], u8),
                haar_inverse as fn(&mut [i32], usize, &mut [i32], u8),
            ),
            (lift53_forward, lift53_inverse),
        ] {
            let mut rows = interleaved.clone();
            let mut tmp = vec![0; rows.len()];
            fwd(&mut rows, 2, &mut tmp, 2);
            let expect_a = {
                let mut x = a.to_vec();
                let mut t = vec![0; x.len()];
                fwd(&mut x, 1, &mut t, 2);
                x
            };
            let expect_b = {
                let mut x = b.to_vec();
                let mut t = vec![0; x.len()];
                fwd(&mut x, 1, &mut t, 2);
                x
            };
            let got_a: Vec<i32> = rows.iter().step_by(2).copied().collect();
            let got_b: Vec<i32> = rows.iter().skip(1).step_by(2).copied().collect();
            assert_eq!(got_a, expect_a);
            assert_eq!(got_b, expect_b);
            inv(&mut rows, 2, &mut tmp, 2);
            assert_eq!(rows, interleaved);
        }
        let mut rows = interleaved.clone();
        delta_forward(&mut rows, 2);
        let mut expect_a = a.to_vec();
        delta_forward(&mut expect_a, 1);
        let got_a: Vec<i32> = rows.iter().step_by(2).copied().collect();
        assert_eq!(got_a, expect_a);
        delta_inverse(&mut rows, 2);
        assert_eq!(rows, interleaved);
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
            delta_forward(&mut x, 1);
            delta_inverse(&mut x, 1);
            assert_eq!(x, input);
        }
    }

    #[test]
    fn closed_form_band_lengths_match_the_halving_chain() {
        // `band_len` replaces an iterated `div_ceil(2)` walk with `⌈n/2ᵏ⌉`;
        // the two must agree for every band the depth rule actually visits.
        for n in 0usize..=257 {
            for levels in [0u8, 1, 2, 3, 8, 255] {
                let mut m = n;
                let mut walked = Vec::new();
                for _ in 0..levels {
                    if m < 2 {
                        break;
                    }
                    walked.push(m);
                    m = m.div_ceil(2);
                }
                let count = band_count(n, levels);
                assert_eq!(
                    count as usize,
                    walked.len(),
                    "depth for n={n} levels={levels}"
                );
                let closed: Vec<usize> = (0..count).map(|k| band_len(n, k)).collect();
                assert_eq!(closed, walked, "bands for n={n} levels={levels}");
            }
        }
    }

    #[test]
    fn levels_beyond_dyadic_depth_saturate() {
        // 255 levels on 6 samples applies only ⌈log₂⌉ levels and stays exact.
        roundtrip(&[1, -2, 3, -4, 5, -6], 255, lift53_forward, lift53_inverse);
    }
}
