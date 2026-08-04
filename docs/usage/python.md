---
title: Python
description: 'Using nd-image-codecs from Python: the maturin-built nd_image_codecs package, the pure-Python codec_series builder, and the zarr-python codec entry points.'
---

# Python

:::{note} Status
All three families work through `zarr-python` v3. Every snippet on this page
is executed by CI against the installed package.
:::

<!-- docs-check: skip — installs the package the checker already has installed -->
```bash
pip install nd-image-codecs          # wheels: manylinux, macOS, Windows
# from source:
cd bindings/python/nd-image-codecs && maturin develop --release
```

## Build a codec series

The builder is pure Python and mirrors the Rust implementation
byte-for-byte:

```python
import numpy as np
import zarr
from nd_image_codecs import codec_series

codecs = codec_series(
    axes=list("tczyx"),
    chunk_shape=[2, 1, 8, 64, 64],
    dtype="uint16",
    family="nd-lift-ht",          # "nd-delta" | "nd-lift-ht" | "nd-zfp"
)
```

`zarr.create_array` does not take one flat codec list: it wants the pipeline
split at the array→bytes boundary into `filters` (array→array), `serializer`
(array→bytes), and `compressors` (bytes→bytes).

```python
SERIALIZERS = {"bytes", "htj2k", "zfp"}
at = next(i for i, codec in enumerate(codecs) if codec["name"] in SERIALIZERS)

volume = (np.arange(2 * 1 * 8 * 64 * 64, dtype=np.uint16) * 7 % 4096).reshape(
    2, 1, 8, 64, 64
)
store: dict = {}
arr = zarr.create_array(
    store,
    shape=volume.shape,
    chunks=(2, 1, 8, 64, 64),
    dtype="uint16",
    filters=codecs[:at],
    serializer=codecs[at],
    compressors=codecs[at + 1 :],
    fill_value=0,
    dimension_names=list("tczyx"),
)
arr[...] = volume
np.testing.assert_array_equal(zarr.open_array(store, mode="r")[...], volume)
```

Override which axes get decorrelated with `decorrelate=` (an exact dimension
index list) or adjust the defaults with `add_decorrelate=` /
`remove_decorrelate=`; see [Zarr & OME-Zarr](./zarr.md) for the rules.

## Codec classes and entry points

The three codecs register through `zarr.codecs` entry points (`nd_lift`,
`htj2k`, `zfp`, `reshape` — plus the deprecated `nd_zfp` read alias), so
pipelines authored by the builder resolve by name.
Config classes are importable for direct use:

```python
from nd_image_codecs import NdLift, Htj2k, NdZfp

assert Htj2k(xy_levels=5, reversible=True).to_dict() == {
    "name": "htj2k",
    "configuration": {
        "xy_levels": 5,
        "reversible": True,
        "progression": "RPCL",
        "index": True,
    },
}
assert NdZfp(mode="fixed_rate", rate=8.0).to_dict() == {
    "name": "zfp",
    "configuration": {"mode": "fixed_rate", "rate": 8.0},
}
```

## nd-delta works today without the native module

The nd-delta family uses only stock codecs (`transpose`, `numcodecs.delta`,
`bytes`, `blosc`), so `codec_series(..., family="nd-delta")` pipelines run on
any current zarr-python even from the pure-Python source tree:

```python
delta = codec_series(
    axes=["z", "y", "x"], chunk_shape=[4, 32, 32], dtype="uint16", family="nd-delta"
)
assert [codec["name"] for codec in delta] == [
    "transpose",
    "numcodecs.delta",
    "bytes",
    "blosc",
]
```

## OME-Zarr

[ngff-zarr](https://github.com/fideus-labs/ngff-zarr) writes OME-Zarr 0.5
multiscales, and forwards codec keywords to `zarr.create_array` — so a
codec-series pipeline drops straight in:

```python
import ngff_zarr as nz

plane = (np.arange(4 * 64 * 64, dtype=np.uint16) * 11 % 4096).reshape(4, 64, 64)
image = nz.to_ngff_image(plane, dims=("z", "y", "x"))
multiscales = nz.to_multiscales(image, scale_factors=[2], chunks=(4, 32, 32))

pipeline = codec_series(
    axes=["z", "y", "x"], chunk_shape=[4, 32, 32], dtype="uint16", family="nd-lift-ht"
)
at = next(i for i, codec in enumerate(pipeline) if codec["name"] in SERIALIZERS)
nz.to_ngff_zarr(
    "volume.ome.zarr",
    multiscales,
    version="0.5",
    filters=pipeline[:at],
    serializer=pipeline[at],
    compressors=pipeline[at + 1 :],
)

# Round-trips, and the OME metadata validates against the NGFF schema.
back = nz.from_ngff_zarr("volume.ome.zarr", validate=True)
np.testing.assert_array_equal(np.asarray(back.images[0].data), plane)
```

ome-zarr-py opens the same store, so napari-ome-zarr and other readers built
on it work unmodified.

## Validation with imagecodecs

For independent verification, decode nd-image-codecs output with
[imagecodecs](https://pypi.org/project/imagecodecs/) instead of our own
implementations. A `zfp` chunk is a standard ZFP stream:

```python
import imagecodecs
from nd_image_codecs import _nd_image_codecs as native

field = (np.arange(4 * 8 * 8, dtype=np.float32) / 3.0).reshape(4, 8, 8)
chunk = native.nd_zfp_encode(field.tobytes(), [4, 8, 8], "float32", mode="reversible")
assert chunk == imagecodecs.zfp_encode(field)           # byte-identical
np.testing.assert_array_equal(imagecodecs.zfp_decode(chunk), field)
```

The [Phase 6 matrix](../development/roadmap/phase-6-validation-and-docs.md)
automates this in CI for ZFP, JPEG 2000, and delta.

## Notes

- The GIL is released during encode/decode, so `dask`/thread-pool parallelism
  scales.
- `imagecodecs` *decodes* our HTJ2K plane codestreams (OpenJPEG 2.5 reads
  Part 15) but does not encode them — the packages coexist and interop in
  tests.
