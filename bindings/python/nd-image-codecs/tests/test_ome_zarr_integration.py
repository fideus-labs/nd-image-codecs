"""OME-Zarr 0.5 integration: multiscales written with each codec family.

The OME-Zarr lane: `ngff-zarr <https://github.com/fideus-labs/ngff-zarr>`_
writes OME-Zarr 0.5 multiscales (its ``to_ngff_zarr`` forwards codec kwargs
to ``zarr.create_array``, so the codec-series pipelines drop straight in),
then the store must

- validate and read back through ``ngff_zarr.from_ngff_zarr(validate=True)``
  (JSON-schema validation of the OME metadata), and
- open and match through ome-zarr-py's ``Reader``.

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


def volume(dtype: str) -> np.ndarray:
    rng = np.random.default_rng(20260803)
    if np.dtype(dtype).kind == "f":
        return (rng.standard_normal(SHAPE) * 100).astype(dtype)
    return rng.integers(0, 4000, size=SHAPE, dtype=dtype)


def write_multiscales(
    path: pathlib.Path, family: str, dtype: str, options: dict
) -> np.ndarray:
    """Write an OME-Zarr 0.5 multiscales store through ngff-zarr."""
    data = volume(dtype)
    image = ngff_zarr.to_ngff_image(data, dims=AXES)
    multiscales = ngff_zarr.to_multiscales(image, scale_factors=[2], chunks=CHUNKS)
    parts = split_pipeline(
        codec_series(list(AXES), list(CHUNKS), dtype, family, **options)
    )
    ngff_zarr.to_ngff_zarr(
        str(path),
        multiscales,
        version="0.5",
        filters=parts["filters"],
        serializer=parts["serializer"],
        compressors=parts["compressors"],
    )
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

    # ngff-zarr: JSON-schema validation of the OME metadata + full read.
    multiscales = ngff_zarr.from_ngff_zarr(str(store), validate=True)
    image = multiscales.images[0]
    assert tuple(image.dims) == AXES
    np.testing.assert_array_equal(np.asarray(image.data), data)

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
    multiscales = ngff_zarr.from_ngff_zarr(str(store), validate=True)
    back = np.asarray(multiscales.images[0].data)
    assert back.shape == data.shape
    assert float(np.abs(back - data).max()) < 1.0
