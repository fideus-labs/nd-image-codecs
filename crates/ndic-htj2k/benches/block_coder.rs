//! Criterion micro-benchmarks for the HT block coder.
//!
//! Local, in-crate iteration on the hot loops; the cross-configuration
//! suite lives in `bench/` (see `docs/development/benchmarking.md`).

#![allow(missing_docs)] // criterion_group! expands to undocumented items

use criterion::{Criterion, criterion_group, criterion_main};
use ndic_htj2k::{BlockPasses, coeff_to_sign_magnitude, decode_block, encode_block};

/// A deterministic dense 64x64 block of 16-bit-ish coefficients.
fn test_block(k_max: u32) -> Vec<u32> {
    let shift = 31 - k_max;
    let mut state = 0x1234_5678u64;
    (0..4096)
        .map(|_| {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let v = ((state >> 33) & 0xFFFF) as i32 - 0x8000;
            coeff_to_sign_magnitude(v, shift)
        })
        .collect()
}

fn block_coder(c: &mut Criterion) {
    let k_max = 18u32;
    let buf = test_block(k_max);
    let coded = encode_block(&buf, 64, 64, 64, k_max - 1).expect("encode");

    c.bench_function("htj2k/cleanup_encode_64x64", |b| {
        b.iter(|| encode_block(std::hint::black_box(&buf), 64, 64, 64, k_max - 1).unwrap());
    });

    let mut out = vec![0u32; 4096];
    c.bench_function("htj2k/cleanup_decode_64x64", |b| {
        b.iter(|| {
            decode_block(
                std::hint::black_box(&coded),
                &mut out,
                64,
                64,
                64,
                k_max - 1,
                &BlockPasses::cleanup_only(coded.len()),
                false,
            )
            .unwrap();
        });
    });
}

criterion_group!(benches, block_coder);
criterion_main!(benches);
