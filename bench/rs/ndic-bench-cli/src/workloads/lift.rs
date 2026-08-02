//! `inventory`-registered `ndic-lift` transform workloads.
//!
//! One entry per direction over a deterministic, z-correlated synthetic
//! volume. Entries participate only in the `nd-lift` config family
//! (`nd-lift-delta-zstd`, `nd-lift-53-zstd`) and return an empty output —
//! which the driver skips — for every other lane. The transform kind follows
//! the config: `lift_levels == 0` benches `delta`, otherwise `lift53` at that
//! depth. Timing only; compression ratios for the same lanes come from the
//! `bench/py/run_nd_lift.py` pipeline records.

use ndic_bench_core::{BenchConfig, BenchEntry, BenchOutput};
use ndic_lift::{AxisTransform, LiftKind, forward, inverse};

const SHAPE: [usize; 3] = [32, 64, 64];
const WARMUP: usize = 3;
const SAMPLES: usize = 30;

inventory::submit! {
    BenchEntry::new("lift", "forward_zyx_32x64x64", bench_forward)
}
inventory::submit! {
    BenchEntry::new("lift", "inverse_zyx_32x64x64", bench_inverse)
}

/// The z-transform the config describes, or `None` when the config is not an
/// `nd-lift` lane.
fn steps_for(cfg: &BenchConfig) -> Option<[AxisTransform; 1]> {
    if cfg.family != "nd-lift" {
        return None;
    }
    let (kind, levels) = if cfg.lift_levels == 0 {
        (LiftKind::Delta, 0)
    } else {
        (LiftKind::Lift53, cfg.lift_levels)
    };
    Some([AxisTransform {
        axis: "z".to_string(),
        dimension: 0,
        kind,
        levels,
        group: 0,
    }])
}

/// A deterministic smooth-in-z volume with mild noise: the structure the
/// transform is designed to decorrelate, generated outside the timed region.
fn zstack() -> Vec<i32> {
    let [nz, ny, nx] = SHAPE;
    let mut samples = Vec::with_capacity(nz * ny * nx);
    let mut noise: u32 = 0x9e37_79b9;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                // Smooth separable field, slowly varying along every axis.
                let f = (z * (nz - z) * 4) + (y * (ny - y)) + (x * (nx - x));
                // Small deterministic xorshift noise (±8).
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let n = i32::try_from(noise % 17).expect("small") - 8;
                samples.push(i32::try_from(f).expect("fits i32") + 500 + n);
            }
        }
    }
    samples
}

fn elapsed_ns(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn bench_forward(cfg: &BenchConfig) -> BenchOutput {
    let Some(steps) = steps_for(cfg) else {
        return BenchOutput::default();
    };
    let original = zstack();
    let mut raw_ns = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        let mut chunk = original.clone();
        let start = std::time::Instant::now();
        forward(&mut chunk, &SHAPE, &steps).expect("bench volume is in budget");
        let ns = elapsed_ns(start);
        if i >= WARMUP {
            raw_ns.push(ns);
        }
        std::hint::black_box(&chunk);
    }
    raw_ns.into()
}

fn bench_inverse(cfg: &BenchConfig) -> BenchOutput {
    let Some(steps) = steps_for(cfg) else {
        return BenchOutput::default();
    };
    let mut coeffs = zstack();
    forward(&mut coeffs, &SHAPE, &steps).expect("bench volume is in budget");
    let mut raw_ns = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        let mut chunk = coeffs.clone();
        let start = std::time::Instant::now();
        inverse(&mut chunk, &SHAPE, &steps).expect("valid transform");
        let ns = elapsed_ns(start);
        if i >= WARMUP {
            raw_ns.push(ns);
        }
        std::hint::black_box(&chunk);
    }
    raw_ns.into()
}
