//! `inventory`-registered HTJ2K workloads.
//!
//! Phase-3 lanes: the 5/3 DWT (scalar vs SIMD, selected by
//! [`BenchConfig::simd`]) and the full HTJ2K plane codec (encode + decode
//! with ratio tracking). The plane codec always runs its shipped
//! (SIMD-lane) DWT — the `dwt53` entry is the lane comparison.

use std::time::Instant;

use ndic_bench_core::{BenchConfig, BenchEntry, BenchOutput};
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

/// Deterministic 16-bit synthetic microscopy-like plane.
#[allow(clippy::cast_possible_wrap)] // small index/state math
fn synthetic_plane(w: usize, h: usize) -> Vec<i32> {
    let mut state = 0x5DEE_CE66u64;
    (0..w * h)
        .map(|i| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let x = (i % w) as i64;
            let y = (i / w) as i64;
            let smooth = ((x * x) / 23 + (y * y) / 31 + x * y / 41) % 4096;
            let noise = (state >> 44) as i64 % 256;
            i32::try_from((smooth + noise).clamp(0, 65_535)).unwrap_or(0)
        })
        .collect()
}

/// Time `iters` runs of `f`, in nanoseconds each.
fn time_samples(iters: usize, mut f: impl FnMut()) -> Vec<u64> {
    (0..iters)
        .map(|_| {
            let t = Instant::now();
            f();
            u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
        })
        .collect()
}

fn dwt53_fwd(config: &BenchConfig) -> BenchOutput {
    let (w, h) = (2048usize, 2048usize);
    let plane = synthetic_plane(w, h);
    let mut buf = plane.clone();
    let raw_ns = time_samples(12, || {
        buf.copy_from_slice(&plane);
        if config.simd {
            ndic_htj2k::dwt::simd::forward_53(&mut buf, w, h, 5).expect("dwt");
        } else {
            ndic_htj2k::dwt::forward_53(&mut buf, w, h, 5).expect("dwt");
        }
    });
    raw_ns.into()
}

fn ht_encode(config: &BenchConfig) -> BenchOutput {
    if config.irreversible {
        return BenchOutput::default(); // 9/7 encode lands post-Phase 3
    }
    let (w, h) = (1024usize, 1024usize);
    let samples = synthetic_plane(w, h);
    let plane = CoeffPlane::new(&samples, w, h).expect("plane");
    let params = EncodeParams::default();
    let mut out = Vec::new();
    let raw_ns = time_samples(10, || {
        out = ndic_codestream::encode_image(&[plane], SampleType::U16, &params).expect("encode");
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some((w * h * 2) as u64),
        bytes_out: Some(out.len() as u64),
    }
}

fn ht_decode(config: &BenchConfig) -> BenchOutput {
    if config.irreversible {
        return BenchOutput::default();
    }
    let (w, h) = (1024usize, 1024usize);
    let samples = synthetic_plane(w, h);
    let plane = CoeffPlane::new(&samples, w, h).expect("plane");
    let stream = ndic_codestream::encode_image(&[plane], SampleType::U16, &EncodeParams::default())
        .expect("encode");
    let raw_ns = time_samples(10, || {
        let cs = ndic_codestream::Codestream::parse(&stream).expect("parse");
        let dec = cs.decode().expect("decode");
        assert_eq!(dec.comps[0].len(), w * h);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(stream.len() as u64),
        bytes_out: Some((w * h * 2) as u64),
    }
}

inventory::submit! { BenchEntry::new("transform", "dwt53_fwd_2048", dwt53_fwd) }
inventory::submit! { BenchEntry::new("htj2k", "plane_encode_1024", ht_encode) }
inventory::submit! { BenchEntry::new("htj2k", "plane_decode_1024", ht_decode) }
