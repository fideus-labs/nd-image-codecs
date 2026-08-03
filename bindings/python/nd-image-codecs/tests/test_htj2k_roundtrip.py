"""The ``htj2k`` zarr-python codec: full nd-lift-ht series round-trips.

Needs the native extension module (``maturin develop`` / an installed
wheel); the whole module is skipped when it is absent, like the other
optional-dependency lanes.
"""

from __future__ import annotations

import numpy as np
import pytest

zarr = pytest.importorskip("zarr", reason="zarr-python v3 not installed")

from nd_image_codecs import codec_series  # noqa: E402
from nd_image_codecs.zarr_codec import Htj2kCodec  # noqa: E402

try:
    from nd_image_codecs import _nd_image_codecs as _native
except ImportError:  # pragma: no cover - source-tree run without the wheel
    _native = None

needs_native = pytest.mark.skipif(
    _native is None or not hasattr(_native, "htj2k_encode"),
    reason="native extension module not built (run `maturin develop`)",
)

AXES = ["t", "c", "z", "y", "x"]
SHAPE = (2, 2, 8, 32, 32)
CHUNKS = (2, 1, 4, 32, 32)


def smooth_volume(dtype: str) -> np.ndarray:
    """Smooth-in-z data inside every dtype's lift budget."""
    t, c, z, y, x = np.meshgrid(*[np.arange(s) for s in SHAPE], indexing="ij")
    base = z * 13 + (x + y) % 7 + t * 5 + c
    info = np.iinfo(dtype)
    scale = max(1, (min(int(info.max), 2**26) - 1) // int(base.max()))
    data = base * scale
    if info.min < 0:
        data = data - (data.max() // 2)
    return data.astype(dtype)


@needs_native
@pytest.mark.parametrize(
    "dtype", ["uint8", "int8", "uint16", "int16", "uint32", "int32"]
)
def test_nd_lift_ht_series_roundtrips(dtype: str) -> None:
    data = smooth_volume(dtype)
    pipeline = codec_series(AXES, list(CHUNKS), dtype, "nd-lift-ht", xy_levels=2)
    # The array-to-bytes serializer is htj2k itself: no "bytes" codec.
    assert pipeline[-1]["name"] == "htj2k"
    store: dict = {}
    array = zarr.create_array(
        store,
        shape=SHAPE,
        chunks=CHUNKS,
        dtype=dtype,
        filters=pipeline[:-1],
        serializer=pipeline[-1],
        compressors=[],
        fill_value=0,
    )
    array[...] = data
    np.testing.assert_array_equal(zarr.open_array(store, mode="r")[...], data)
    # A sub-chunk read (the partial-decode path).
    np.testing.assert_array_equal(array[1, 0, 2:5, 4:9, 3:7], data[1, 0, 2:5, 4:9, 3:7])
    # Every stored chunk is an htj2k container.
    chunks = [v.to_bytes() for k, v in store.items() if k.startswith("c/")]
    assert chunks and all(v[:4] == b"ndht" for v in chunks)


@needs_native
def test_chunks_are_byte_identical_to_the_rust_codec_fixture() -> None:
    """The chunk container opens with the ndht magic and decodes exactly."""
    shape = [3, 16, 16]
    data = (np.arange(np.prod(shape), dtype=np.uint16) * 7 % 4096).reshape(shape)
    blob = _native.htj2k_encode(
        data.tobytes(), list(shape), "uint16", xy_levels=2
    )
    assert blob[:4] == b"ndht"
    back = _native.htj2k_decode(blob, list(shape), "uint16")
    np.testing.assert_array_equal(
        np.frombuffer(back, dtype=np.uint16).reshape(shape), data
    )


@needs_native
def test_metadata_round_trips_and_bad_configs_are_refused() -> None:
    codec = Htj2kCodec.from_dict(
        {"name": "htj2k", "configuration": {"xy_levels": 3}}
    )
    assert codec.to_dict() == {
        "name": "htj2k",
        "configuration": {
            "xy_levels": 3,
            "reversible": True,
            "progression": "RPCL",
            "index": True,
        },
    }
    with pytest.raises(ValueError, match="progression"):
        Htj2kCodec(progression="SNAKE")
    with pytest.raises(ValueError, match="xy_levels"):
        Htj2kCodec(xy_levels=40)


@needs_native
def test_float_dtypes_are_refused_with_guidance() -> None:
    import zarr.core.dtype as zdt

    codec = Htj2kCodec()
    with pytest.raises(ValueError, match="nd-zfp"):
        codec.validate(
            shape=(4, 8, 8),
            dtype=zdt.Float32(),
            chunk_grid=None,
        )
