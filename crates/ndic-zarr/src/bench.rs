//! `inventory`-registered `nd_lift` **codec-level** workloads for the
//! `ndic-bench` driver.
//!
//! Where the `lift/*` entries (in `ndic-lift`) time the bare plane
//! transforms, these time the full zarrs codec path — sample widening from
//! the array dtype into the coefficient plane, the transform, and the
//! narrowing back — over a `uint16` volume, the dominant microscopy dtype.
//! Entries participate only in the `nd-lift` config family and opt out of
//! every other lane; `lift_levels == 0` benches the `delta` kind, otherwise
//! `lift53` at that depth.

use std::num::NonZeroU64;

use ndic_bench_core::{BenchConfig, BenchEntry, BenchOutput};
use ndic_lift::{AxisTransform, LiftKind, NdLiftConfig};
use zarrs::array::codec::api::{ArrayBytes, ArrayToArrayCodecTraits, CodecOptions};
use zarrs::array::{DataType, FillValue, data_type};

use crate::lift_codec::NdLiftCodec;

const SHAPE: [usize; 3] = [32, 64, 64];
const WARMUP: usize = 3;
const SAMPLES: usize = 30;

inventory::submit! {
    BenchEntry::new("lift_codec", "encode_u16_zyx_32x64x64", bench_encode)
}
inventory::submit! {
    BenchEntry::new("lift_codec", "decode_u16_zyx_32x64x64", bench_decode)
}

/// The codec the config describes, or `None` when the config is not an
/// `nd-lift` lane.
fn codec_for(cfg: &BenchConfig) -> Option<NdLiftCodec> {
    if cfg.family != "nd-lift" {
        return None;
    }
    let (kind, levels) = if cfg.lift_levels == 0 {
        (LiftKind::Delta, 0)
    } else {
        (LiftKind::Lift53, cfg.lift_levels)
    };
    let config = NdLiftConfig::new(vec![AxisTransform {
        axis: "z".to_string(),
        dimension: 0,
        kind,
        levels,
        group: 0,
    }]);
    Some(NdLiftCodec::new(config).expect("valid bench config"))
}

/// A deterministic smooth-in-z `uint16` volume (native-endian bytes),
/// generated outside the timed region.
fn zstack_u16_bytes() -> Vec<u8> {
    let [nz, ny, nx] = SHAPE;
    let mut bytes = Vec::with_capacity(nz * ny * nx * 2);
    let mut noise: u32 = 0x9e37_79b9;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let f = (z * (nz - z) * 4) + (y * (ny - y)) + (x * (nx - x));
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let n = usize::try_from(noise % 17).expect("small");
                let v = u16::try_from(f + 500 + n).expect("fits u16");
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
    }
    bytes
}

struct Ctx {
    codec: NdLiftCodec,
    shape: Vec<NonZeroU64>,
    dtype: DataType,
    fill: FillValue,
}

fn ctx_for(cfg: &BenchConfig) -> Option<Ctx> {
    Some(Ctx {
        codec: codec_for(cfg)?,
        shape: SHAPE
            .iter()
            .map(|&d| NonZeroU64::new(d as u64).expect("non-zero"))
            .collect(),
        dtype: data_type::uint16(),
        fill: FillValue::new(vec![0u8; 2]),
    })
}

fn elapsed_ns(start: std::time::Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn bench_encode(cfg: &BenchConfig) -> BenchOutput {
    let Some(ctx) = ctx_for(cfg) else {
        return BenchOutput::default();
    };
    let input = zstack_u16_bytes();
    let options = CodecOptions::default();
    let mut raw_ns = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        let bytes = ArrayBytes::from(input.as_slice());
        let start = std::time::Instant::now();
        let encoded = ctx
            .codec
            .encode(bytes, &ctx.shape, &ctx.dtype, &ctx.fill, &options)
            .expect("bench volume encodes");
        let ns = elapsed_ns(start);
        if i >= WARMUP {
            raw_ns.push(ns);
        }
        std::hint::black_box(&encoded);
    }
    raw_ns.into()
}

fn bench_decode(cfg: &BenchConfig) -> BenchOutput {
    let Some(ctx) = ctx_for(cfg) else {
        return BenchOutput::default();
    };
    let input = zstack_u16_bytes();
    let options = CodecOptions::default();
    let coeffs = ctx
        .codec
        .encode(
            ArrayBytes::from(input.as_slice()),
            &ctx.shape,
            &ctx.dtype,
            &ctx.fill,
            &options,
        )
        .expect("bench volume encodes")
        .into_fixed()
        .expect("fixed bytes")
        .into_owned();
    let mut raw_ns = Vec::with_capacity(SAMPLES);
    for i in 0..WARMUP + SAMPLES {
        let bytes = ArrayBytes::from(coeffs.as_slice());
        let start = std::time::Instant::now();
        let decoded = ctx
            .codec
            .decode(bytes, &ctx.shape, &ctx.dtype, &ctx.fill, &options)
            .expect("valid coefficients decode");
        let ns = elapsed_ns(start);
        if i >= WARMUP {
            raw_ns.push(ns);
        }
        std::hint::black_box(&decoded);
    }
    raw_ns.into()
}
