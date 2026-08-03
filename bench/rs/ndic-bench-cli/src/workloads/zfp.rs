//! `inventory`-registered **nd-zfp family** workloads: the `nd_zfp` chunk
//! codec over a correlated float volume, plus fixed-rate brick economy.
//!
//! The lanes differ by mode: `zfp-rate8` (irreversible) runs fixed-rate at
//! 8 bits/value — the GPU-brick budget — while `zfp-reversible` runs the
//! lossless mode. The fixture mirrors the family's target data: a smooth
//! separable float field with small deterministic noise.
//!
//! `brick_bytes` records random-access economy on the fixed-rate lane:
//! `bytes_out` is the byte span one `4³` brick occupies at its computed
//! offset (what a ranged reader fetches), `bytes_in` the whole chunk — the
//! record's "ratio" is the fetched fraction, and the timed region is the
//! brick's actual decode ([`ndic_zfp::decompress_brick`]).

use std::time::Instant;

use ndic_bench_core::{BenchConfig, BenchEntry, BenchOutput};
use ndic_zfp::{BrickIndex, NdZfpConfig, ZfpDtype, ZfpScalarKind};

const SHAPE: [usize; 3] = [32, 64, 64];
const WARMUP: usize = 3;
const SAMPLES: usize = 20;
const RATE: f64 = 8.0;

inventory::submit! {
    BenchEntry::new("zfp", "chunk_encode_f32_zyx_32x64x64", bench_encode)
}
inventory::submit! {
    BenchEntry::new("zfp", "chunk_decode_f32_zyx_32x64x64", bench_decode)
}
inventory::submit! {
    BenchEntry::new("zfp", "brick_bytes_f32_zyx_32x64x64", bench_brick_bytes)
}

/// The lane's codec configuration, or `None` when the config is not an
/// nd-zfp lane: `zfp-rate8` (irreversible) is fixed-rate at 8 bits/value,
/// `zfp-reversible` the lossless mode.
fn config_for(cfg: &BenchConfig) -> Option<NdZfpConfig> {
    if cfg.family != "nd-zfp" {
        return None;
    }
    Some(if cfg.irreversible {
        NdZfpConfig {
            mode: "fixed_rate".into(),
            rate: Some(RATE),
            dims: 3,
            ..Default::default()
        }
    } else {
        NdZfpConfig {
            mode: "reversible".into(),
            dims: 3,
            ..Default::default()
        }
    })
}

/// A deterministic correlated float volume (little-endian `f32` bytes):
/// smooth and separable with small noise, the field ZFP's transform is
/// built for — mirroring the family's simulation/microscopy targets.
fn correlated_volume_f32_bytes() -> Vec<u8> {
    let [nz, ny, nx] = SHAPE;
    let mut bytes = Vec::with_capacity(nz * ny * nx * 4);
    let mut noise: u32 = 0x9e37_79b9;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let f = (z * (nz - z) * 4) + (y * (ny - y)) + (x * (nx - x));
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let n = noise % 5;
                #[allow(clippy::cast_precision_loss)] // f + 500 + n < 2^13
                let v = (f + 500 + n as usize) as f32 / 3.0;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    bytes
}

fn time_samples(iters: usize, mut f: impl FnMut()) -> Vec<u64> {
    (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
        })
        .skip(WARMUP)
        .collect()
}

fn bench_encode(cfg: &BenchConfig) -> BenchOutput {
    let Some(config) = config_for(cfg) else {
        return BenchOutput::default();
    };
    let input = correlated_volume_f32_bytes();
    let mut chunk = Vec::new();
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        chunk = ndic_zfp::encode_chunk(&input, &SHAPE, ZfpDtype::F32, &config)
            .expect("bench volume encodes");
        std::hint::black_box(&chunk);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(input.len() as u64),
        bytes_out: Some(chunk.len() as u64),
    }
}

fn bench_decode(cfg: &BenchConfig) -> BenchOutput {
    let Some(config) = config_for(cfg) else {
        return BenchOutput::default();
    };
    let input = correlated_volume_f32_bytes();
    let chunk =
        ndic_zfp::encode_chunk(&input, &SHAPE, ZfpDtype::F32, &config).expect("chunk encodes");
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        let decoded =
            ndic_zfp::decode_chunk(&chunk, &SHAPE, ZfpDtype::F32, &config).expect("chunk decodes");
        assert_eq!(decoded.len(), input.len());
        std::hint::black_box(&decoded);
    });
    // Compression-direction bytes (matching the other families' decode
    // records), so the ratio gate treats better compression as an
    // improvement on this record too.
    BenchOutput {
        raw_ns,
        bytes_in: Some(input.len() as u64),
        bytes_out: Some(chunk.len() as u64),
    }
}

fn bench_brick_bytes(cfg: &BenchConfig) -> BenchOutput {
    let Some(config) = config_for(cfg) else {
        return BenchOutput::default();
    };
    if config.mode != "fixed_rate" {
        // Random brick access is the fixed-rate lane's story only.
        return BenchOutput::default();
    }
    let input = correlated_volume_f32_bytes();
    let chunk =
        ndic_zfp::encode_chunk(&input, &SHAPE, ZfpDtype::F32, &config).expect("chunk encodes");
    let index = BrickIndex::fixed_rate(&SHAPE, ZfpScalarKind::F32, RATE).expect("index");
    // A middle brick: fully interior, representative of a renderer fetch.
    let coords = [4usize, 8, 8];
    let k = index.linear(&coords).expect("in grid");
    let (_, span) = index.byte_range(k).expect("in range");
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        let (brick, _) = ndic_zfp::decompress_brick::<f32>(&chunk, &SHAPE, RATE, &coords)
            .expect("brick decodes");
        assert_eq!(brick.len(), 64);
        std::hint::black_box(&brick);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(chunk.len() as u64),
        bytes_out: Some(span),
    }
}
