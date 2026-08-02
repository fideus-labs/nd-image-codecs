//! Property tests: `inverse ∘ forward` is the identity for every transform
//! kind, level depth, grouping, and dtype value range, on both plane widths.

use ndic_lift::{AxisTransform, LiftKind, forward, inverse};
use proptest::collection::vec as pvec;
use proptest::prelude::*;

/// Input value ranges that transform in the `i32` plane.
///
/// The ≤16-bit dtypes appear at their full width — the plane is wide enough
/// for any transform this test generates. `u32`/`i32` input also lands in the
/// `i32` plane but cannot be exercised at full width: the overflow budget
/// refuses those encodes outright (`chunk::tests::
/// budget_refuses_full_range_i32_planes`), so 32-bit dtypes contribute a
/// bounded sub-range with the same headroom [`i64_cases`] leaves — 15 bits
/// for three steps × three 5/3 levels and their intermediates.
const I32_PLANE_RANGES: &[(i32, i32)] = &[
    (0, u8::MAX as i32),
    (i8::MIN as i32, i8::MAX as i32),
    (0, u16::MAX as i32),
    (i16::MIN as i32, i16::MAX as i32),
    (-(1 << 16), 1 << 16),
];

fn kinds() -> impl Strategy<Value = LiftKind> {
    prop_oneof![
        Just(LiftKind::Delta),
        Just(LiftKind::Haar),
        Just(LiftKind::Lift53),
    ]
}

/// Random steps over `ndim` dimensions (repeats allowed — steps compose).
fn steps(ndim: usize) -> impl Strategy<Value = Vec<AxisTransform>> {
    pvec(
        (0..ndim, kinds(), 1u8..=3, prop_oneof![Just(0u32), 2u32..=6]).prop_map(
            |(dimension, kind, levels, group)| AxisTransform {
                axis: format!("d{dimension}"),
                dimension,
                kind,
                levels,
                group,
            },
        ),
        1..=3,
    )
}

/// A shape of 1–4 dimensions (each 1–9), a chunk of values from one dtype
/// range, and a random step list — the full round-trip case.
fn i32_cases() -> impl Strategy<Value = (Vec<usize>, Vec<i32>, Vec<AxisTransform>)> {
    (pvec(1usize..=9, 1..=4), 0..I32_PLANE_RANGES.len()).prop_flat_map(|(shape, range_index)| {
        let (lo, hi) = I32_PLANE_RANGES[range_index];
        let len: usize = shape.iter().product();
        let ndim = shape.len();
        (Just(shape), pvec(lo..=hi, len), steps(ndim))
    })
}

/// 64-bit dtype cases transform in the `i64` plane; values span well past
/// `i32` while staying inside the encode overflow budget.
fn i64_cases() -> impl Strategy<Value = (Vec<usize>, Vec<i64>, Vec<AxisTransform>)> {
    // Leaves headroom for the worst random case here (three steps × three
    // 5/3 levels ≈ nine doublings plus intermediates) inside the i64 budget.
    const BOUND: i64 = 1 << 48;
    pvec(1usize..=9, 1..=4).prop_flat_map(|shape| {
        let len: usize = shape.iter().product();
        let ndim = shape.len();
        (Just(shape), pvec(-BOUND..=BOUND, len), steps(ndim))
    })
}

proptest! {
    #[test]
    fn i32_plane_roundtrips((shape, chunk, steps) in i32_cases()) {
        let mut transformed = chunk.clone();
        forward(&mut transformed, &shape, &steps).unwrap();
        inverse(&mut transformed, &shape, &steps).unwrap();
        prop_assert_eq!(transformed, chunk);
    }

    #[test]
    fn i64_plane_roundtrips((shape, chunk, steps) in i64_cases()) {
        let mut transformed = chunk.clone();
        forward(&mut transformed, &shape, &steps).unwrap();
        inverse(&mut transformed, &shape, &steps).unwrap();
        prop_assert_eq!(transformed, chunk);
    }
}
