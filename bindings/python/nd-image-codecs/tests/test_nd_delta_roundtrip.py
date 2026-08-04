"""nd-delta end-to-end round-trip via ``zarr-python``.

Authors nd-delta pipelines with :func:`nd_image_codecs.codec_series`, writes
OME-Zarr-shaped arrays through ``zarr-python`` v3, and asserts bit-exact
round-trips on realistic microscopy-like data. Every codec in the nd-delta
family already exists (``transpose``, ``numcodecs.delta``, ``bytes``,
``blosc``), so this validates the builder's output against real
implementations.
"""

from __future__ import annotations

import numpy as np
import pytest

zarr = pytest.importorskip("zarr", minversion="3.0")

from nd_image_codecs import codec_series  # noqa: E402
from synthetic import microscopy_volume  # noqa: E402


def write_and_read(
    data: np.ndarray,
    axes: list[str],
    chunks: tuple[int, ...],
    pipeline: list[dict],
) -> np.ndarray:
    """Write `data` through `pipeline` with zarr-python and read it back."""
    # Split the flat codec list at the array→bytes boundary the way
    # zarr-python's create_array expects: filters (array→array),
    # serializer (array→bytes), compressors (bytes→bytes).
    serializer_at = next(i for i, c in enumerate(pipeline) if c["name"] == "bytes")
    store: dict = {}
    arr = zarr.create_array(
        store,
        shape=data.shape,
        chunks=chunks,
        dtype=data.dtype,
        filters=pipeline[:serializer_at],
        serializer=pipeline[serializer_at],
        compressors=pipeline[serializer_at + 1 :],
        dimension_names=axes,
        fill_value=0,
    )
    arr[:] = data
    return zarr.open_array(store, mode="r")[:]


@pytest.mark.parametrize("backend", ["zstd", "lz4"])
def test_tczyx_microscopy_roundtrip(backend: str) -> None:
    axes = ["t", "c", "z", "y", "x"]
    chunks = (4, 1, 16, 32, 32)
    data = microscopy_volume(shape=(4, 2, 16, 32, 32))
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta", delta_backend=backend)
    assert [c["name"] for c in pipeline] == [
        "transpose",
        "numcodecs.delta",
        "bytes",
        "blosc",
    ]
    back = write_and_read(data, axes, chunks, pipeline)
    np.testing.assert_array_equal(back, data)


def test_zyx_volume_roundtrip() -> None:
    axes = ["z", "y", "x"]
    chunks = (16, 32, 32)
    data = microscopy_volume(shape=(1, 1, 16, 32, 32))[0, 0]
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta")
    back = write_and_read(data, axes, chunks, pipeline)
    np.testing.assert_array_equal(back, data)


def test_yx_plane_no_transpose_roundtrip() -> None:
    # With no grouped z/t there is no delta axis to move: the pipeline is
    # transpose-free and must still round-trip.
    axes = ["y", "x"]
    chunks = (32, 32)
    data = microscopy_volume(shape=(1, 1, 1, 32, 32))[0, 0, 0]
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta")
    assert pipeline[0]["name"] == "numcodecs.delta"
    back = write_and_read(data, axes, chunks, pipeline)
    np.testing.assert_array_equal(back, data)


def test_partial_edge_chunks_roundtrip() -> None:
    # Array extent not divisible by the chunk shape exercises edge chunks.
    axes = ["z", "y", "x"]
    chunks = (16, 32, 32)
    data = microscopy_volume(shape=(1, 1, 24, 50, 50))[0, 0]
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta")
    back = write_and_read(data, axes, chunks, pipeline)
    np.testing.assert_array_equal(back, data)


def test_extreme_values_roundtrip() -> None:
    # Delta wraps around at the dtype boundary; wrap-around must be exact.
    axes = ["z", "y", "x"]
    chunks = (8, 16, 16)
    rng = np.random.default_rng(11)
    data = rng.integers(0, 2**16, size=(8, 16, 16), dtype=np.uint16)
    data[0] = 0
    data[1] = np.iinfo(np.uint16).max
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta")
    back = write_and_read(data, axes, chunks, pipeline)
    np.testing.assert_array_equal(back, data)


def test_nd_delta_beats_raw_on_correlated_data() -> None:
    # Sanity-check the premise, not just correctness: on smooth volumetric
    # data the nd-delta pipeline must compress.
    axes = ["z", "y", "x"]
    chunks = (16, 64, 64)
    data = microscopy_volume(shape=(1, 1, 16, 64, 64))[0, 0]
    pipeline = codec_series(axes, list(chunks), "uint16", "nd-delta")
    serializer_at = next(i for i, c in enumerate(pipeline) if c["name"] == "bytes")
    store: dict = {}
    arr = zarr.create_array(
        store,
        shape=data.shape,
        chunks=chunks,
        dtype=data.dtype,
        filters=pipeline[:serializer_at],
        serializer=pipeline[serializer_at],
        compressors=pipeline[serializer_at + 1 :],
        fill_value=0,
    )
    arr[:] = data
    stored = sum(len(v) for k, v in store.items() if k.startswith("c/"))
    assert stored < data.nbytes, f"nd-delta did not compress: {stored} >= {data.nbytes}"
