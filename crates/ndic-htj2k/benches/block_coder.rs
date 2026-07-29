//! Criterion micro-benchmarks for the HT block coder.
//!
//! Real workloads land with roadmap Phase 1 (`cleanup_{encode,decode}_64x64`);
//! this harness exists so the `[[bench]]` target builds from day one. The
//! cross-configuration suite lives in `bench/` (see
//! `docs/development/benchmarking.md`) — Criterion here is for local, in-crate
//! iteration on the hot loops.

#![allow(missing_docs)] // criterion_group! expands to undocumented items

use criterion::{Criterion, criterion_group, criterion_main};

fn block_coder_placeholder(c: &mut Criterion) {
    c.bench_function("htj2k/placeholder_noop", |b| {
        b.iter(|| std::hint::black_box(0u32));
    });
}

criterion_group!(benches, block_coder_placeholder);
criterion_main!(benches);
