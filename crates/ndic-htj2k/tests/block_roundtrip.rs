//! Encode-decode round-trip tests for the HT block coder.
//!
//! The encoder emits a cleanup-only segment; decoding it must reproduce
//! every coefficient exactly (the cleanup pass is self-contained and, at
//! `p = 31 - K_max`, lossless).

#![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)] // test RNG math
#![allow(clippy::cast_sign_loss)]

use ndic_htj2k::{
    BlockPasses, coeff_to_sign_magnitude, decode_block, encode_block, sign_magnitude_to_coeff,
};
use proptest::prelude::*;

/// Encodes and decodes one block; asserts exact coefficient recovery.
fn assert_roundtrip(coeffs: &[i32], width: usize, height: usize, k_max: u32) {
    let shift = 31 - k_max;
    let missing_msbs = k_max - 1;
    let buf: Vec<u32> = coeffs
        .iter()
        .map(|&v| coeff_to_sign_magnitude(v, shift))
        .collect();

    // Skip blocks with no significance at bitplane p, like the tile encoder.
    let mv = buf.iter().fold(0u32, |a, &v| a | (v & 0x7FFF_FFFF));
    if mv < (1 << shift) {
        return;
    }

    let coded = encode_block(&buf, width, height, width, missing_msbs).expect("encode");

    let mut out = vec![0u32; coeffs.len()];
    decode_block(
        &coded,
        &mut out,
        width,
        height,
        width,
        missing_msbs,
        &BlockPasses::cleanup_only(coded.len()),
        false,
    )
    .expect("decode");

    for (i, (&want, &got)) in coeffs.iter().zip(out.iter()).enumerate() {
        assert_eq!(
            sign_magnitude_to_coeff(got, shift),
            want,
            "sample {i} ({}, {}) of {width}x{height} K_max={k_max}: raw {got:#010x}",
            i % width,
            i / width,
        );
    }
}

#[test]
fn roundtrips_small_fixed_blocks() {
    // 4x4 with one significant sample in each quad position.
    for pos in 0..16 {
        let mut c = vec![0i32; 16];
        c[pos] = if pos % 2 == 0 { 5 } else { -3 };
        assert_roundtrip(&c, 4, 4, 8);
    }

    // 8x8 ramp with signs.
    let ramp: Vec<i32> = (0..64).map(|i| (i - 32) * 3).collect();
    assert_roundtrip(&ramp, 8, 8, 10);

    // Full-range 16-bit-ish values at K_max = 18.
    let extremes: Vec<i32> = (0..64)
        .map(|i| if i % 3 == 0 { 65535 } else { -65536 + i })
        .collect();
    assert_roundtrip(&extremes, 8, 8, 18);

    // Single row / single column blocks.
    assert_roundtrip(&[1, -2, 3, -4, 5, -6, 7, -8], 8, 1, 6);
    assert_roundtrip(&[1, -2, 3, -4, 5, -6, 7, -8], 1, 8, 6);

    // 1x1.
    assert_roundtrip(&[-7], 1, 1, 5);

    // Odd sizes exercise partial quads on both axes.
    let odd: Vec<i32> = (0..35).map(|i| (i * 7 % 23) - 11).collect();
    assert_roundtrip(&odd, 7, 5, 8);
    assert_roundtrip(&odd[..15], 5, 3, 8);
    assert_roundtrip(&odd[..9], 3, 3, 8);
}

#[test]
fn roundtrips_dense_and_sparse_64x64() {
    // Dense pseudo-random block (LCG so the test is deterministic).
    let mut state = 0x1234_5678u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        ((state >> 33) & 0xFFFF) as i32 - 0x8000
    };
    let dense: Vec<i32> = (0..4096).map(|_| next()).collect();
    assert_roundtrip(&dense, 64, 64, 18);

    // Sparse block: a few isolated significant samples.
    let mut sparse = vec![0i32; 4096];
    for i in [0usize, 63, 64 * 63, 4095, 2048, 1000, 1001, 1064] {
        sparse[i] = if i % 2 == 0 { 12345 } else { -4321 };
    }
    assert_roundtrip(&sparse, 64, 64, 16);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn roundtrips_random_blocks(
        width in 1usize..=64,
        height in 1usize..=64,
        k_max in 2u32..=28,
        seed in any::<u64>(),
        density in 0u32..=100,
    ) {
        let bound = 1i64 << (k_max - 1);
        let mut state = seed | 1;
        let mut next = move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            state >> 32
        };
        let coeffs: Vec<i32> = (0..width * height)
            .map(|_| {
                if next() % 100 >= u64::from(density) {
                    0
                } else {
                    let m = (next() % (2 * bound as u64)) as i64 - bound;
                    m as i32
                }
            })
            .collect();
        assert_roundtrip(&coeffs, width, height, k_max);
    }
}
