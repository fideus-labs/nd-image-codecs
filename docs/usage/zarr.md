---
title: Zarr & OME-Zarr
description: Choosing between the nd-delta, nd-lift-ht, and nd-zfp families and wiring the resulting codec series into a Zarr v3 or OME-Zarr array.
---

# Zarr & OME-Zarr

> **Status:** Skeleton — the builder and nd-delta work in
> [Phase 1](../development/roadmap/phase-1-baselines-and-series.md); nd-lift-ht
> and nd-zfp land in Phases
> [4](../development/roadmap/phase-4-nd-lift-ht.md) and
> [5](../development/roadmap/phase-5-nd-zfp.md).

## The three families

nd-image-codecs authors Zarr v3 **codec series** — short pipelines of
array→array and array→bytes codecs — tuned per use case:

| Family | Series | Best for |
| --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bytes → blosc(bitshuffle, zstd/lz4)` | Fast lossless storage; works today with stock codecs |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | Scalable microscopy; streaming thumbnails and progressive resolution |
| **nd-zfp** | `transpose → nd_zfp` | GPU volume rendering, O(1) random brick access, predictable memory |

## The `codec_series` builder

Don't hand-write the JSON — describe your array and let the builder derive the
transpose order, decorrelation axes, and codec configuration. It is implemented
natively in [Rust](./rust.md), [Python](./python.md), and
[TypeScript](./typescript.md), with byte-identical output.

```python
from nd_image_codecs import codec_series

codecs = codec_series(
    axes=list("tczyx"),       # one name per dimension, e.g. ["t","c","z","y","x"]
    chunk_shape=[8, 1, 32, 256, 256],
    dtype="uint16",
    family="nd-lift-ht",      # "nd-delta" | "nd-lift-ht" | "nd-zfp"
)
arr = zarr.create_array(store, shape=..., chunks=..., dtype="uint16",
                        codecs=codecs)
```

Builder behavior (full spec:
[codec-series.md](../architecture/codec-series.md)):

- The fastest-moving dimensions are transposed into `(z-)yx` order; `t` is
  placed before `z`/`y` when its chunk extent is > 1.
- By default, decorrelation (delta/lift) is applied along `z` (chunk > 1) and
  `t` (chunk > 1); other axes can be added and defaults removed with the
  exact `decorrelate` index list or `add`/`remove` adjustments. `y`/`x` are
  never decorrelation targets.
- nd-delta places its (single) delta axis fastest-moving instead, because
  `numcodecs.delta` differences the flattened C-order stream.
- nd-zfp maps up to 4 non-singleton chunk dimensions onto ZFP block dimensions.

## Example configurations

nd-lift-ht array→bytes stage:

```json
{ "name": "htj2k",
  "configuration": { "xy_levels": 5, "reversible": true,
                     "progression": "RPCL", "index": true } }
```

nd-zfp:

```json
{ "name": "nd_zfp",
  "configuration": { "mode": "fixed_rate", "rate": 8.0, "dims": 3 } }
```

## Chunking guidance

- **Volumetric zyx data:** chunks like `(32, 1024, 1024)` — decorrelation happens
  inside each chunk; chunks stay independent.
- **Time series:** give `t` a chunk extent > 1 (e.g. 8) to group time into the
  decorrelation set.
- **Very large planes:** pair with sharding (`sharding_indexed`) — many
  sub-chunks per shard object; byte-range reads then hit sub-chunks directly.
- **Leading singleton axes** (OME-Zarr `t`/`c` of size 1) are left in place; no
  transform is placed on them.
- **Family choice:** integers + speed → nd-delta; integers + streaming/pyramids →
  nd-lift-ht; floats or GPU bricks → nd-zfp.

## Validating with independent implementations

Cross-validate pipelines with third-party codecs via
[imagecodecs](https://pypi.org/project/imagecodecs/) rather than our own
implementations:

```python
# Decode an nd-zfp chunk with imagecodecs' ZFP instead of ndic-zfp:
import imagecodecs
plain = imagecodecs.zfp_decode(chunk_bytes, shape=(32, 256, 256), dtype="f4")
```

The [Phase 6 validation matrix](../development/roadmap/phase-6-validation-and-docs.md)
automates this in CI for ZFP, JPEG 2000, and delta.

## OME-Zarr

The families complement OME-Zarr multiscales
([OME-NGFF spec](https://ngff.openmicroscopy.org/latest/)): pyramid levels give
coarse zoom steps; within each nd-lift-ht chunk, RPCL + the coefficient-plane
index enable partial low-resolution decode where the store supports byte-range
reads ([partial-read synergy](../architecture/zarr-codec.md)). Compared with the
common blosc/zstd defaults, expect substantially smaller lossless volumes on
natural images — tracked continuously in the
[bench suite](../development/benchmarking.md).

## Migration

```python
# Re-encode an existing blosc dataset (per-chunk, parallel with dask):
old = zarr.open("blosc.zarr")
new = zarr.create_array(..., codecs=codec_series(..., family="nd-lift-ht"))
new[:] = old[:]                 # dask recommended for large volumes
```

Bit-exactness on the reversible paths means migration is lossless and verifiable
with a checksum pass.
