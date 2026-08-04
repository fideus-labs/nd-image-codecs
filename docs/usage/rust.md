---
title: Rust Library
description: 'Using nd-image-codecs from Rust: the codec_series builder, the codec crates, and their zarrs integration.'
---

# Rust Library

:::{note} Status
Every snippet on this page is compiled and run by CI against the current
API (`scripts/ci/check-usage-docs.py`).
:::

```toml
[dependencies]
ndic-core = "0.0"
ndic-codestream = "0.0"
ndic-lift = "0.0"
ndic-zfp = "0.0"
# ndic-zarr has no default features. `zarrs` is the one that registers
# nd_lift/htj2k/zfp into the zarrs plugin registry, so leaving it off gives
# you a crate that cannot open an array using them. The codec_series builder
# below needs no features; see "Feature flags" for the rest.
ndic-zarr = { version = "0.0", features = ["zarrs"] }
```

## Build a codec series

```rust
use ndic_zarr::series::{codec_series, Axis, Family, SeriesSpec};

let axes: Vec<Axis> = "tczyx"
    .chars().enumerate()
    .map(|(i, c)| Axis::new(i, &c.to_string()))
    .collect();

let spec = SeriesSpec::new(axes, vec![8, 1, 32, 256, 256], "uint16", Family::NdLiftHt);
// spec.decorrelate = Decorrelate::Adjust { add: vec![], remove: vec![] };  // overrides
let codecs = codec_series(&spec)?;   // → Vec<serde_json::Value>: the Zarr v3 `codecs` array
assert_eq!(codecs[0]["name"], "transpose");
```

The same builder backs `ndic series` and the Python/TypeScript mirrors; output
is byte-identical across all three (see [Zarr & OME-Zarr](./zarr.md) for the rules).

## Encode a plane / a volume

`ndic-codestream` codes one 2D plane per call: components are
[`CoeffPlane`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-core/src/plane.rs)
views over `i32` samples, and a volume is its planes coded independently. The
`Zarr` `htj2k` codec is the batched form of exactly this loop.

```rust
use ndic_core::{CoeffPlane, EncodeParams, SampleType};

// Defaults are streaming-friendly: RPCL, 64×64 blocks, 5/3, TLM/PLT on.
let params = EncodeParams::default();

let (width, height) = (64, 48);
let samples: Vec<i32> = (0..width * height)
    .map(|i| i32::try_from((i * 7) % 4096).unwrap())
    .collect();
let plane = CoeffPlane { samples: &samples, width, height };
let codestream = ndic_codestream::writer::encode_image(&[plane], SampleType::U16, &params)?;
assert_eq!(&codestream[..2], &[0xff, 0x4f]);   // SOC marker
```

Cross-axis decorrelation is not an `EncodeParams` concern — it belongs to the
`nd_lift` codec upstream ([the cross-axis transform](../architecture/nd-transform.md)):

```rust
use ndic_lift::{AxisTransform, LiftKind};

let steps = [AxisTransform {
    axis: "z".into(), dimension: 0, kind: LiftKind::Lift53, levels: 2, group: 0,
}];
let shape = [8usize, 8, 8];
let mut chunk: Vec<i32> = (0..512).map(|i| (i % 97) as i32).collect();
let original = chunk.clone();
ndic_lift::forward(&mut chunk, &shape, &steps)?;    // then encode the planes
ndic_lift::inverse(&mut chunk, &shape, &steps)?;
assert_eq!(chunk, original);                        // reversible, exactly
```

## ZFP chunks and bricks

The ZFP core ([zfp codec](../architecture/zfp.md)) compresses 1D–4D
arrays into standard ZFP streams; in fixed-rate mode every `4^d` brick sits
at a computed offset:

```rust
use ndic_zfp::{BrickIndex, ZfpMode, ZfpScalarKind};

let volume_shape = [32usize, 64, 64];
let field: Vec<f32> = (0..volume_shape.iter().product::<usize>())
    .map(|i| (i % 512) as f32 / 3.0)
    .collect();
let zfp = ndic_zfp::compress(&field, &volume_shape, ZfpMode::FixedRate(8.0))?;

// Decode one 4³ brick without touching the rest of the payload…
let (brick, brick_shape) =
    ndic_zfp::decompress_brick::<f32>(&zfp, &volume_shape, 8.0, &[2, 10, 7])?;
assert_eq!(brick_shape, [4, 4, 4]);
assert_eq!(brick.len(), 64);

// …or plan the ranged fetch for it (HTTP Range, Zarr partial read).
let bricks = BrickIndex::fixed_rate(&volume_shape, ZfpScalarKind::F32, 8.0)?;
let (offset, len) = bricks.byte_range(bricks.linear(&[2, 10, 7])?)?;
assert!(offset + len <= zfp.len() as u64);
```

## Decode — full and partial

```rust
use ndic_codestream::range::RangeIndex;
use ndic_codestream::reader::Codestream;

// Full decode. `comps` holds one i32 plane per component.
let decoded = Codestream::parse(&codestream)?.decode()?;
assert_eq!((decoded.width, decoded.height), (64, 48));
assert_eq!(decoded.comps[0], samples);

// Partial: plan a thumbnail's byte ranges without reading the whole file,
// then decode just those bytes at the resolution the plan covers.
let index = RangeIndex::from_codestream(&Codestream::parse(&codestream)?)?;
let plan = index.thumbnail(32)?;                    // ranges + decoded_size
assert!(plan.total_bytes < codestream.len() as u64);
let prefix: Vec<u8> = plan.ranges.iter()
    .flat_map(|r| codestream[r.start as usize..=r.end as usize].to_vec())
    .collect();
let thumb = Codestream::parse_prefix(&prefix)?.decode_to_resolution(plan.max_res)?;
assert_eq!(vec![thumb.height as u64, thumb.width as u64], plan.decoded_size);
```

## Error handling

Everything returns `ndic_core::Result<T>`; match on the variants when you need to
distinguish caller bugs from bad data:

```rust
use ndic_core::Error;

match Codestream::parse(b"not a codestream").and_then(|cs| cs.decode()) {
    Ok(image) => println!("decoded {}×{}", image.width, image.height),
    Err(Error::Codestream { offset, message }) => {
        println!("bad stream @{offset}: {message}");
    }
    Err(other) => return Err(other.into()),
}
```

## Feature flags

| Crate | Feature | Effect |
| --- | --- | --- |
| all | `std` (default) | Disable for `no_std` (WASM core builds) |
| `ndic-htj2k`, `ndic-lift` | `parallel` (default) | `rayon` across code-blocks; off for wasm32 |
| `ndic-lift`, `ndic-zfp` | `serde` | Codec configuration types (`NdLiftConfig`, `NdZfpConfig`) |
| `ndic-zarr` | `zarrs` | The Zarr v3 codec registration ([Zarr & OME-Zarr](./zarr.md)) |
| `ndic-zarr` | `wasm` | The `wasm-bindgen` chunk cores for the TypeScript package |

## Going deeper

- Parameter semantics: [`EncodeParams` docs](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-core/src/params.rs)
  and [codestream architecture](../architecture/codestream.md)
- Partial decode & range plans: [Byte-Range Access](../architecture/range-access.md)
- Cross-axis transform semantics: [nd_lift Transform](../architecture/nd-transform.md)
