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
pip install nd-image-codecs          # wheels: manylinux, musllinux, macOS, Windows (x86-64 + arm64)
pip install "nd-image-codecs[zarr]"  # + zarr-python v3 (adds the codec pipeline)
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
multiscales. Since v0.44 it moves pixels through
[zarrista](https://pypi.org/project/zarrista/) rather than zarr-python, and
zarrista's codec set is `bytes` plus gzip/zstd/blosc — it can encode none of
these families (`filters=` raises, `serializer=` is dropped silently) and
decode none of them either. So write the two halves of the store with the
library that can do each: the OME metadata and the array skeletons with
ngff-zarr, the arrays themselves with zarr-python, which resolves the
pipeline through the registered codec names. `metadata_only=` arrived in the
same release, so the recipe below wants `ngff-zarr >= 0.44`.

```python
import ngff_zarr as nz

plane = (np.arange(4 * 64 * 64, dtype=np.uint16) * 11 % 4096).reshape(4, 64, 64)
image = nz.to_ngff_image(plane, dims=("z", "y", "x"))
multiscales = nz.to_multiscales(image, scale_factors=[2], chunks=(4, 32, 32))

# The OME metadata plus one empty array per scale level, no pixels.
nz.to_ngff_zarr("volume.ome.zarr", multiscales, version="0.5", metadata_only=True)

for dataset, level in zip(multiscales.metadata.datasets, multiscales.images):
    level_data = np.asarray(level.data)
    # A coarser level can be smaller than the chunk asked for, and the
    # pipeline belongs to the chunk the codecs actually see.
    chunks = tuple(min(c, s) for c, s in zip((4, 32, 32), level_data.shape))
    pipeline = codec_series(
        axes=list(level.dims),
        chunk_shape=list(chunks),
        dtype=str(level_data.dtype),
        family="nd-lift-ht",
    )
    at = next(i for i, codec in enumerate(pipeline) if codec["name"] in SERIALIZERS)
    level_array = zarr.create_array(
        "volume.ome.zarr",
        name=dataset.path,
        shape=level_data.shape,
        chunks=chunks,
        dtype=level_data.dtype,
        filters=pipeline[:at],
        serializer=pipeline[at],
        compressors=pipeline[at + 1 :],
        fill_value=0,
        dimension_names=list(level.dims),
        overwrite=True,  # replace the skeleton, codec chain and all
    )
    level_array[...] = level_data

# ngff-zarr consolidated the skeletons; refresh so a reader that trusts the
# consolidated document sees the codecs the arrays carry.
zarr.consolidate_metadata("volume.ome.zarr")

# Round-trips, and the OME metadata validates against the NGFF schema.
root = zarr.open_group("volume.ome.zarr", mode="r")
nz.validate(dict(root.attrs), version="0.5", model="image")
np.testing.assert_array_equal(root["scale0/image"][...], plane)
```

Reading splits the same way: `nz.validate` checks the OME metadata, and the
arrays open through zarr-python — `nz.from_ngff_zarr` goes to zarrista and
fails on the codec chain. ome-zarr-py, which reads through zarr-python, opens
the store unmodified, so napari-ome-zarr and other readers built on it work.

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

The third-party interop tests
([`test_imagecodecs_interop.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bindings/python/nd-image-codecs/tests/test_imagecodecs_interop.py)
and [`test_nd_zfp_roundtrip.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bindings/python/nd-image-codecs/tests/test_nd_zfp_roundtrip.py))
automate this in the `python` CI job for ZFP, JPEG 2000, and delta.

## Notes

- The GIL is released during encode/decode, so `dask`/thread-pool parallelism
  scales.
- `imagecodecs` *decodes* our HTJ2K plane codestreams (OpenJPEG 2.5 reads
  Part 15) but does not encode them — the packages coexist and interop in
  tests.
