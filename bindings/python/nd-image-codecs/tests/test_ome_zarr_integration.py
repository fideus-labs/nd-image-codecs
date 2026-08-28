"""OME-Zarr 0.5 integration: multiscales written with each codec family.

The OME-Zarr lane splits the write between the two libraries that can each
do half of it: `ngff-zarr <https://github.com/fideus-labs/ngff-zarr>`_ models
the multiscales and writes the OME metadata (``to_ngff_zarr`` with
``metadata_only=True``, which creates every scale level's array and writes no
pixels), and zarr-python re-creates each of those arrays with the codec-series
pipeline and fills it. The store must then

- validate against the OME-Zarr JSON schema (``ngff_zarr.validate``),
- read back through zarr-python, and
- open and match through ome-zarr-py's ``Reader``.

ngff-zarr writes its pixels through zarrista from 0.44 on, and zarrista's
codec set is ``bytes`` plus gzip/zstd/blosc: ``filters=`` raises, ``serializer=``
is dropped silently, and ``from_ngff_zarr`` cannot open an array whose chain
holds anything else (``codec numcodecs.delta is not supported``). Only
zarr-python resolves a pipeline by the registered codec names, so the arrays
go through it; ngff-zarr stays the authority on the OME metadata.

nd-delta datasets are composed entirely of stock registered codecs
(``transpose``, ``numcodecs.delta``, ``bytes``, ``blosc``), so any Zarr
v3-capable OME-Zarr viewer (napari-ome-zarr, viv, …) opens them with no
extra plugins — confirming a GUI viewer stays a manual step, but the
metadata/codec surface those viewers consume is exactly what these tests
pin. The nd-lift-ht and nd-zfp datasets additionally need our entry-point
codecs importable, which is what the ecosystem registration provides.
"""

from __future__ import annotations

import json
import pathlib

import numpy as np
import pytest

ngff_zarr = pytest.importorskip("ngff_zarr")
pytest.importorskip("ome_zarr")
zarr = pytest.importorskip("zarr", minversion="3.0")

from lanes import split_pipeline  # noqa: E402  (bench/py, via conftest)
from nd_image_codecs import codec_series  # noqa: E402

try:
    from nd_image_codecs import _nd_image_codecs as _native
except ImportError:  # pragma: no cover - pure-Python checkout without the wheel
    _native = None

AXES = ("z", "y", "x")
SHAPE = (8, 64, 64)
CHUNKS = (4, 32, 32)

FAMILIES = [
    pytest.param("nd-delta", "uint16", {}, id="nd-delta"),
    pytest.param(
        "nd-lift-ht",
        "uint16",
        {"xy_levels": 2},
        id="nd-lift-ht",
        marks=pytest.mark.skipif(_native is None, reason="needs the native extension"),
    ),
    pytest.param(
        "nd-zfp",
        "float32",
        {},
        id="nd-zfp",
        marks=pytest.mark.skipif(_native is None, reason="needs the native extension"),
    ),
]


def multiscales_datasets(root) -> list[str]:
    """The scale-level array paths a store's OME metadata declares, coarsest last."""
    return [
        dataset["path"] for dataset in root.attrs["ome"]["multiscales"][0]["datasets"]
    ]


def volume(dtype: str) -> np.ndarray:
    rng = np.random.default_rng(20260803)
    if np.dtype(dtype).kind == "f":
        return (rng.standard_normal(SHAPE) * 100).astype(dtype)
    return rng.integers(0, 4000, size=SHAPE, dtype=dtype)


def write_multiscales(
    path: pathlib.Path, family: str, dtype: str, options: dict
) -> np.ndarray:
    """Write an OME-Zarr 0.5 multiscales store, one library per half.

    ngff-zarr writes the OME metadata and the array skeletons; zarr-python
    re-creates each skeleton with the family's codec series and fills it.
    """
    data = volume(dtype)
    image = ngff_zarr.to_ngff_image(data, dims=AXES)
    multiscales = ngff_zarr.to_multiscales(image, scale_factors=[2], chunks=CHUNKS)
    ngff_zarr.to_ngff_zarr(str(path), multiscales, version="0.5", metadata_only=True)

    for dataset, level in zip(
        multiscales.metadata.datasets, multiscales.images, strict=True
    ):
        level_data = np.asarray(level.data)
        # The coarser levels are smaller than the requested chunk shape, and
        # the pipeline is built for the chunk the codecs actually see.
        chunks = tuple(min(c, s) for c, s in zip(CHUNKS, level_data.shape))
        parts = split_pipeline(
            codec_series(
                list(level.dims), list(chunks), str(level_data.dtype), family, **options
            )
        )
        array = zarr.create_array(
            str(path),
            name=dataset.path,
            shape=level_data.shape,
            chunks=chunks,
            dtype=level_data.dtype,
            filters=parts["filters"],
            serializer=parts["serializer"],
            compressors=parts["compressors"],
            fill_value=0,
            dimension_names=list(level.dims),
            overwrite=True,  # replace the skeleton, codec chain and all
        )
        array[...] = level_data

    # ngff-zarr consolidated the skeletons it wrote; refresh so a reader that
    # trusts the consolidated document sees the codecs the arrays carry.
    zarr.consolidate_metadata(str(path))
    return data


@pytest.mark.parametrize(("family", "dtype", "options"), FAMILIES)
def test_multiscales_validate_and_open(
    tmp_path: pathlib.Path, family: str, dtype: str, options: dict
) -> None:
    store = tmp_path / "volume.ome.zarr"
    data = write_multiscales(store, family, dtype, options)

    # The pipeline actually landed in every scale level's array metadata.
    arrays = [p for p in store.rglob("zarr.json")
              if json.loads(p.read_text()).get("node_type") == "array"]
    assert len(arrays) >= 2, "multiscales must hold at least two levels"
    expected = {
        "nd-delta": "numcodecs.delta",
        "nd-lift-ht": "htj2k",
        "nd-zfp": "zfp",
    }[family]
    for meta_path in arrays:
        names = [c["name"] for c in json.loads(meta_path.read_text())["codecs"]]
        assert expected in names, f"{meta_path}: {names}"

    # ngff-zarr: JSON-schema validation of the OME metadata; zarr-python: the
    # full read (from_ngff_zarr cannot decode these chains, see the module docs).
    root = zarr.open_group(str(store), mode="r")
    ngff_zarr.validate(dict(root.attrs), version="0.5", model="image")
    full_scale = root[multiscales_datasets(root)[0]]
    assert tuple(full_scale.metadata.dimension_names) == AXES
    np.testing.assert_array_equal(full_scale[...], data)

    # ome-zarr-py: the reader used by napari-ome-zarr resolves the same store.
    from ome_zarr.io import parse_url
    from ome_zarr.reader import Reader

    nodes = list(Reader(parse_url(str(store), mode="r"))())
    assert nodes, "ome-zarr-py must discover the multiscales node"
    np.testing.assert_array_equal(np.asarray(nodes[0].data[0]), data)


def test_lossy_zfp_multiscales_validate(tmp_path: pathlib.Path) -> None:
    """Fixed-rate nd-zfp: metadata still validates; data is close, not equal."""
    if _native is None:
        pytest.skip("needs the native extension")
    store = tmp_path / "lossy.ome.zarr"
    data = write_multiscales(store, "nd-zfp", "float32", {"zfp_rate": 16})
    root = zarr.open_group(str(store), mode="r")
    ngff_zarr.validate(dict(root.attrs), version="0.5", model="image")
    back = root[multiscales_datasets(root)[0]][...]
    assert back.shape == data.shape
    assert float(np.abs(back - data).max()) < 1.0
