//! `inventory`-registered **nd-lift-ht family** workloads: the composed
//! `nd_lift → htj2k` chunk path plus the byte-range thumbnail economy.
//!
//! These are what tell the family lanes apart: `simd-53-ht` runs the plane
//! codec over an undecorrelated chunk (`lift_levels == 0`), `simd-53-lift-z2`
//! lifts z two levels first — the measured ratio gap between the two *is*
//! the z-decorrelation gain the roadmap asks the baselines to record. The
//! fixture mirrors `bench/py/synthetic.correlated_zstack`: smooth in z with
//! small shot-like noise, the case decorrelation exists for.
//!
//! `thumbnail_bytes` records byte-range plan economy: `bytes_out` is the
//! bytes a 16-px thumbnail plan fetches, `bytes_in` the whole chunk — the
//! record's "ratio" is the fetched fraction, gated like any other ratio.

use std::num::NonZeroU64;
use std::time::Instant;

use ndic_bench_core::{BenchConfig, BenchEntry, BenchOutput};
use ndic_codestream::range::RangeIndex;
use ndic_lift::{AxisTransform, LiftKind, NdLiftConfig};
use ndic_zarr::htj2k_codec::Htj2kCodec;
use ndic_zarr::lift_codec::NdLiftCodec;
use zarrs::array::codec::api::{
    ArrayBytes, ArrayToArrayCodecTraits, ArrayToBytesCodecTraits, CodecOptions,
};
use zarrs::array::{DataType, FillValue, data_type};

const SHAPE: [usize; 3] = [32, 64, 64];
const WARMUP: usize = 3;
const SAMPLES: usize = 20;

inventory::submit! {
    BenchEntry::new("lift_ht", "chunk_encode_u16_zyx_32x64x64", bench_encode)
}
inventory::submit! {
    BenchEntry::new("lift_ht", "chunk_decode_u16_zyx_32x64x64", bench_decode)
}
inventory::submit! {
    BenchEntry::new("lift_ht", "thumbnail_bytes_zyx_32x64x64", bench_thumbnail_bytes)
}

/// The composed pipeline for one family lane, or `None` when the config is
/// not a reversible nd-lift-ht lane (the 9/7 lane opts out until the lossy
/// path lands; the scalar lane exists for the DWT comparison only).
struct Pipeline {
    lift: Option<NdLiftCodec>,
    htj2k: Htj2kCodec,
    shape: Vec<NonZeroU64>,
    dtype: DataType,
    fill: FillValue,
}

fn pipeline_for(cfg: &BenchConfig) -> Option<Pipeline> {
    if cfg.family != "nd-lift-ht" || cfg.irreversible || !cfg.simd {
        return None;
    }
    let lift = (cfg.lift_levels > 0).then(|| {
        let config = NdLiftConfig::new(vec![AxisTransform {
            axis: "z".to_string(),
            dimension: 0,
            kind: LiftKind::Lift53,
            levels: cfg.lift_levels,
            group: 0,
        }]);
        NdLiftCodec::new(config).expect("valid bench config")
    });
    let htj2k = Htj2kCodec::new(ndic_zarr::htj2k::Htj2kConfig::default()).expect("default config");
    Some(Pipeline {
        lift,
        htj2k,
        shape: SHAPE
            .iter()
            .map(|&d| NonZeroU64::new(d as u64).expect("non-zero"))
            .collect(),
        dtype: data_type::uint16(),
        fill: FillValue::new(vec![0u8; 2]),
    })
}

/// A deterministic correlated z-stack (native-endian `uint16` bytes):
/// smooth in z with small noise, mirroring `synthetic.correlated_zstack`.
fn correlated_zstack_u16_bytes() -> Vec<u8> {
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
                let n = usize::try_from(noise % 5).expect("small");
                let v = u16::try_from(f + 500 + n).expect("fits u16");
                bytes.extend_from_slice(&v.to_ne_bytes());
            }
        }
    }
    bytes
}

impl Pipeline {
    /// The htj2k-facing (post-lift) data type and fill value.
    fn plane_repr(&self) -> (DataType, FillValue) {
        match &self.lift {
            Some(lift) => (
                lift.encoded_data_type(&self.dtype).expect("dtype"),
                lift.encoded_fill_value(&self.dtype, &self.fill)
                    .expect("fill"),
            ),
            None => (self.dtype.clone(), self.fill.clone()),
        }
    }

    fn encode(&self, input: &[u8], options: &CodecOptions) -> Vec<u8> {
        let (dtype, fill) = self.plane_repr();
        let mut bytes = ArrayBytes::from(input);
        if let Some(lift) = &self.lift {
            bytes = lift
                .encode(bytes, &self.shape, &self.dtype, &self.fill, options)
                .expect("lift encodes");
        }
        self.htj2k
            .encode(bytes, &self.shape, &dtype, &fill, options)
            .expect("chunk encodes")
            .into_owned()
    }

    fn decode(&self, chunk: &[u8], options: &CodecOptions) -> Vec<u8> {
        let (dtype, fill) = self.plane_repr();
        let mut bytes = self
            .htj2k
            .decode(
                zarrs::array::codec::api::ArrayBytesRaw::from(chunk),
                &self.shape,
                &dtype,
                &fill,
                options,
            )
            .expect("chunk decodes");
        if let Some(lift) = &self.lift {
            bytes = lift
                .decode(bytes, &self.shape, &self.dtype, &self.fill, options)
                .expect("lift decodes");
        }
        bytes.into_fixed().expect("fixed").into_owned()
    }
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
    let Some(pipeline) = pipeline_for(cfg) else {
        return BenchOutput::default();
    };
    let input = correlated_zstack_u16_bytes();
    let options = CodecOptions::default();
    let mut chunk = Vec::new();
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        chunk = pipeline.encode(&input, &options);
        std::hint::black_box(&chunk);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(input.len() as u64),
        bytes_out: Some(chunk.len() as u64),
    }
}

fn bench_decode(cfg: &BenchConfig) -> BenchOutput {
    let Some(pipeline) = pipeline_for(cfg) else {
        return BenchOutput::default();
    };
    let input = correlated_zstack_u16_bytes();
    let options = CodecOptions::default();
    let chunk = pipeline.encode(&input, &options);
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        let decoded = pipeline.decode(&chunk, &options);
        assert_eq!(decoded.len(), input.len());
        std::hint::black_box(&decoded);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(chunk.len() as u64),
        bytes_out: Some(input.len() as u64),
    }
}

fn bench_thumbnail_bytes(cfg: &BenchConfig) -> BenchOutput {
    let Some(pipeline) = pipeline_for(cfg) else {
        return BenchOutput::default();
    };
    let input = correlated_zstack_u16_bytes();
    let options = CodecOptions::default();
    let chunk = pipeline.encode(&input, &options);
    let mut fetched = 0u64;
    let raw_ns = time_samples(WARMUP + SAMPLES, || {
        let index = RangeIndex::from_chunk_prefix(&chunk).expect("indexed chunk");
        let plan = index.thumbnail(16).expect("plan");
        assert!(plan.ranges.len() <= 3, "thumbnail plan economy");
        fetched = plan.total_bytes;
        std::hint::black_box(&plan);
    });
    BenchOutput {
        raw_ns,
        bytes_in: Some(chunk.len() as u64),
        bytes_out: Some(fetched),
    }
}
