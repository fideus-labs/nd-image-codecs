---
title: Zarr & OME-Zarr
description: Choosing between the nd-delta, nd-lift-ht, and nd-zfp families and wiring the resulting codec series into a Zarr v3 or OME-Zarr array.
---

# Zarr & OME-Zarr

:::{note} Status
The builder and nd-delta work since
[Phase 1](../development/roadmap/phase-1-baselines-and-series.md); the
nd-lift-ht family (`transpose → nd_lift → htj2k`) round-trips across zarrs
and zarr-python since
[Phase 4](../development/roadmap/phase-4-nd-lift-ht.md); nd-zfp
(`transpose → nd_zfp`) since
[Phase 5](../development/roadmap/phase-5-nd-zfp.md).
:::

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
import numpy as np
import zarr
from nd_image_codecs import codec_series

codecs = codec_series(
    axes=list("tczyx"),       # one name per dimension, e.g. ["t","c","z","y","x"]
    chunk_shape=[2, 1, 8, 64, 64],
    dtype="uint16",
    family="nd-lift-ht",      # "nd-delta" | "nd-lift-ht" | "nd-zfp"
)

# zarr-python wants the flat list split at the array→bytes codec.
SERIALIZERS = {"bytes", "htj2k", "nd_zfp"}
at = next(i for i, codec in enumerate(codecs) if codec["name"] in SERIALIZERS)

store: dict = {}
arr = zarr.create_array(
    store, shape=(2, 1, 8, 64, 64), chunks=(2, 1, 8, 64, 64), dtype="uint16",
    filters=codecs[:at], serializer=codecs[at], compressors=codecs[at + 1:],
    fill_value=0,
)
```

Builder behavior (full spec:
[codec series](../architecture/codec-series.md)):

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
# Decode an nd-zfp chunk with imagecodecs' ZFP instead of ndic-zfp. An nd_zfp
# chunk is a standard ZFP stream, header and all, so nothing is unwrapped
# first — and the stream carries its own shape and dtype.
import imagecodecs
from nd_image_codecs import _nd_image_codecs as native

field = (np.arange(4 * 16 * 16, dtype=np.float32) / 7.0).reshape(4, 16, 16)
chunk_bytes = native.nd_zfp_encode(
    field.tobytes(), [4, 16, 16], "float32", mode="reversible"
)
plain = imagecodecs.zfp_decode(chunk_bytes)
np.testing.assert_array_equal(plain, field)
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
# Re-encode an existing blosc dataset (per-chunk; use dask for large volumes).
volume = (np.arange(4 * 64 * 64, dtype=np.uint16) * 13 % 4096).reshape(4, 64, 64)
old_store: dict = {}
old = zarr.create_array(old_store, shape=volume.shape, chunks=(4, 32, 32),
                        dtype="uint16", fill_value=0)
old[...] = volume

pipeline = codec_series(axes=["z", "y", "x"], chunk_shape=[4, 32, 32],
                        dtype="uint16", family="nd-lift-ht")
at = next(i for i, codec in enumerate(pipeline) if codec["name"] in SERIALIZERS)
new_store: dict = {}
new = zarr.create_array(
    new_store, shape=old.shape, chunks=(4, 32, 32), dtype=old.dtype,
    filters=pipeline[:at], serializer=pipeline[at], compressors=pipeline[at + 1:],
    fill_value=0,
)
new[...] = old[...]
np.testing.assert_array_equal(new[...], volume)   # reversible: bit-exact
```

Bit-exactness on the reversible paths means migration is lossless and verifiable
with a checksum pass.
