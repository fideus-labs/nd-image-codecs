---
title: Python
description: 'Using nd-image-codecs from Python: the maturin-built nd_image_codecs package, the pure-Python codec_series builder, and the zarr-python codec entry points.'
---

# Python

:::{caution} Status: Skeleton
The package builds today (maturin) and the
`codec_series` builder works; codec behavior lands per the
[roadmap](../development/roadmap/index.md).
:::

```bash
pip install nd-image-codecs          # wheels: manylinux, macOS, Windows (post-Phase 4)
# from source:
cd bindings/python/nd-image-codecs && maturin develop --release
```

## Build a codec series

Works today — the builder is pure Python (mirrors the Rust implementation
byte-for-byte):

```python
import zarr
from nd_image_codecs import codec_series

codecs = codec_series(
    axes=list("tczyx"),
    chunk_shape=[8, 1, 32, 256, 256],
    dtype="uint16",
    family="nd-lift-ht",          # "nd-delta" | "nd-lift-ht" | "nd-zfp"
)
arr = zarr.create_array(
    store="volume.zarr", shape=(100, 1, 512, 4096, 4096),
    chunks=(8, 1, 32, 1024, 1024), dtype="uint16", codecs=codecs,
)
arr[:] = volume
```

Override which axes get decorrelated with `decorrelate=` (an exact dimension
index list) or adjust the defaults with `add_decorrelate=` /
`remove_decorrelate=`; see [](./zarr.md) for the rules.

## Codec classes and entry points

The three codecs register through `zarr.codecs` entry points (`nd_lift`,
`htj2k`, `nd_zfp`), so pipelines authored by the builder resolve by name.
Config classes are importable for direct use:

```python
from nd_image_codecs import NdLift, Htj2k, NdZfp

Htj2k(xy_levels=5, reversible=True).to_dict()
NdZfp(mode="fixed_rate", rate=8.0, dims=3).to_dict()
```

## nd-delta works today

The nd-delta family uses only stock codecs (`transpose`, `numcodecs.delta`,
`bytes`, `blosc`), so `codec_series(..., family="nd-delta")` pipelines run on
any current zarr-python without this package's native module.

## OME-Zarr

```python
import ngff_zarr as nz

image = nz.from_ngff_zarr("input.ome.zarr")
# write multiscales with nd-image-codecs families (post-Phase 4):
# nz.to_ngff_zarr("output.ome.zarr", image, codecs=codec_series(...))
```

([ngff-zarr](https://github.com/fideus-labs/ngff-zarr) integration example —
final API tracked in Phase 6.)

## Validation with imagecodecs

For independent verification, decode nd-image-codecs output with
[imagecodecs](https://pypi.org/project/imagecodecs/) (ZFP, JPEG 2000, delta)
instead of our implementations — the pattern the
[Phase 6 matrix](../development/roadmap/phase-6-validation-and-docs.md)
automates.

## Notes

- The GIL is released during encode/decode, so `dask`/thread-pool parallelism
  scales.
- `imagecodecs` can *decode* classic JPEG 2000 but not encode HTJ2K — the
  packages coexist and interop in tests.
