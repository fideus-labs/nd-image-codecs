---
title: Rust Library
description: 'Using nd-image-codecs from Rust: the codec_series builder, the codec crates, and their zarrs integration.'
---

# Rust Library

:::{caution} Status: Partial
The `codec_series` builder and the registered Zarr codecs (`nd_lift`,
`htj2k`, `nd_zfp`) work today; the direct codestream API sketches below
land per the [roadmap](../development/roadmap/index.md).
:::

```toml
[dependencies]
ndic-core = "0.0"
ndic-codestream = "0.0"
# ndic-zarr has no default features. `zarrs` is the one that registers
# nd_lift/htj2k/nd_zfp into the zarrs plugin registry, so leaving it off gives
# you a crate that cannot open an array using them. The codec_series builder
# below needs no features; see "Feature flags" for the rest.
ndic-zarr = { version = "0.0", features = ["zarrs"] }
```

## Build a codec series

Works today:

```rust
use ndic_zarr::series::{codec_series, Axis, Family, SeriesSpec};

let axes: Vec<Axis> = "tczyx"
    .chars().enumerate()
    .map(|(i, c)| Axis::new(i, &c.to_string()))
    .collect();

let spec = SeriesSpec::new(axes, vec![8, 1, 32, 256, 256], "uint16", Family::NdLiftHt);
// spec.decorrelate = Decorrelate::Adjust { add: vec![], remove: vec![] };  // overrides
let codecs = codec_series(&spec)?;   // → Vec<serde_json::Value>: the Zarr v3 `codecs` array
```

The same builder backs `ndic series` and the Python/TypeScript mirrors; output
is byte-identical across all three (see [Zarr & OME-Zarr](./zarr.md) for the rules).

## Encode a plane / a volume

```rust
use ndic_core::{EncodeParams, SampleType, VolumeView, WaveletKind};

// Defaults are streaming-friendly: RPCL, 64×64 blocks, 5/3, TLM/PLT on.
let params = EncodeParams::default();

let volume = VolumeView { samples: &samples, depth: 64, height: 512, width: 512 };
// let bytes = ndic_codestream::encode(volume, SampleType::U16, &params)?;
```

Cross-axis decorrelation is not an `EncodeParams` concern — it belongs to the
`nd_lift` codec upstream ([the cross-axis transform](../architecture/nd-transform.md)):

```rust
use ndic_lift::{AxisTransform, LiftKind};

let steps = [AxisTransform {
    axis: "z".into(), dimension: 0, kind: LiftKind::Lift53, levels: 2, group: 0,
}];
// ndic_lift::forward(&mut chunk, &shape, &steps)?;   // then encode planes
```

## ZFP chunks and bricks

The `nd_zfp` core ([nd_zfp codec](../architecture/zfp.md)) compresses 1D–4D
arrays into standard ZFP streams; in fixed-rate mode every `4^d` brick sits
at a computed offset:

```rust
use ndic_zfp::{BrickIndex, ZfpMode, ZfpScalarKind};

let shape = [32usize, 256, 256];
let bytes = ndic_zfp::compress(&samples, &shape, ZfpMode::FixedRate(8.0))?;

// Decode one 4³ brick without touching the rest of the payload…
let (brick, brick_shape) =
    ndic_zfp::decompress_brick::<f32>(&bytes, &shape, 8.0, &[2, 10, 7])?;

// …or plan the ranged fetch for it (HTTP Range, Zarr partial read).
let index = BrickIndex::fixed_rate(&shape, ZfpScalarKind::F32, 8.0)?;
let (offset, len) = index.byte_range(index.linear(&[2, 10, 7])?)?;
```

## Decode — full and partial

```rust
// Full decode
// let vol = ndic_codestream::decode(&bytes)?;

// Partial: thumbnail without reading the whole file
// let index = RangeIndex::from_reader(&mut file)?;
// let plan = index.thumbnail(256);            // Vec<ByteRange>
// let thumb = decode_ranges(&header, fetch(plan), Want::Thumbnail(256))?;
```

## Error handling

Everything returns `ndic_core::Result<T>`; match on the variants when you need to
distinguish caller bugs from bad data:

```rust
use ndic_core::Error;

match ndic_codestream::decode(&bytes) {
    Ok(v) => use_volume(v),
    Err(Error::Codestream { offset, message }) => log::warn!("bad stream @{offset}: {message}"),
    Err(e) => return Err(e.into()),
}
```

## Feature flags

| Crate | Feature | Effect |
| --- | --- | --- |
| all | `std` (default) | Disable for `no_std` (WASM core builds) |
| `ndic-htj2k`, `ndic-lift` | `parallel` (default) | `rayon` across code-blocks; off for wasm32 |
| `ndic-zarr` | `zarrs` | The Zarr v3 codec registration ([Zarr & OME-Zarr](./zarr.md)) |

## Going deeper

- Parameter semantics: [`EncodeParams` docs](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-core/src/params.rs)
  and [codestream architecture](../architecture/codestream.md)
- Partial decode & range plans: [Byte-Range Access](../architecture/range-access.md)
- Cross-axis transform semantics: [nd_lift Transform](../architecture/nd-transform.md)
