"""The ``nd_zfp`` zarr-python codec: full nd-zfp series round-trips and the
differential lane against ``imagecodecs`` (the LLNL C reference via FFI).

Needs the native extension module (``maturin develop`` / an installed
wheel); the whole module is skipped when it is absent, like the other
optional-dependency lanes.
"""

from __future__ import annotations

import numpy as np
import pytest

zarr = pytest.importorskip("zarr", reason="zarr-python v3 not installed")

from nd_image_codecs import codec_series  # noqa: E402
from nd_image_codecs.zarr_codec import NdZfpCodec  # noqa: E402

try:
    from nd_image_codecs import _nd_image_codecs as _native
except ImportError:  # pragma: no cover - source-tree run without the wheel
    _native = None

needs_native = pytest.mark.skipif(
    _native is None or not hasattr(_native, "nd_zfp_encode"),
    reason="native extension module not built (run `maturin develop`)",
)

AXES = ["t", "c", "z", "y", "x"]
SHAPE = (2, 2, 8, 32, 32)
CHUNKS = (2, 1, 4, 32, 32)


def smooth_volume(dtype: str) -> np.ndarray:
    """Smooth data with per-dtype scaling (float dtypes get fractions)."""
    t, c, z, y, x = np.meshgrid(*[np.arange(s) for s in SHAPE], indexing="ij")
    base = z * 13 + (x + y) % 7 + t * 5 + c
    if np.issubdtype(np.dtype(dtype), np.floating):
        return (base / 3.0 - 11.5).astype(dtype)
    info = np.iinfo(dtype)
    scale = max(1, (min(int(info.max), 2**26) - 1) // int(base.max()))
    data = base * scale
    if info.min < 0:
        data = data - (data.max() // 2)
    return data.astype(dtype)


@needs_native
@pytest.mark.parametrize(
    "dtype",
    ["uint8", "int8", "uint16", "int16", "int32", "int64", "float32", "float64"],
)
def test_nd_zfp_series_roundtrips_reversibly(dtype: str) -> None:
    data = smooth_volume(dtype)
    pipeline = codec_series(AXES, list(CHUNKS), dtype, "nd-zfp")
    # The array-to-bytes serializer is nd_zfp itself: no "bytes" codec.
    assert pipeline[-1]["name"] == "nd_zfp"
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
    # Every stored chunk is a ZFP stream (magic + codec version 5).
    chunks = [v.to_bytes() for k, v in store.items() if k.startswith("c/")]
    assert chunks and all(v[:4] == b"zfp\x05" for v in chunks)


@needs_native
def test_fixed_rate_bounds_the_error_and_serves_subsets() -> None:
    data = smooth_volume("float32")
    pipeline = codec_series(AXES, list(CHUNKS), "float32", "nd-zfp", zfp_rate=16.0)
    assert pipeline[-1]["configuration"]["mode"] == "fixed_rate"
    store: dict = {}
    array = zarr.create_array(
        store,
        shape=SHAPE,
        chunks=CHUNKS,
        dtype="float32",
        filters=pipeline[:-1],
        serializer=pipeline[-1],
        compressors=[],
        fill_value=0,
    )
    array[...] = data
    back = array[...]
    assert float(np.abs(back - data).max()) < 0.5
    # Sub-chunk reads must agree exactly with the full (lossy) decode.
    np.testing.assert_array_equal(array[1, 0, 2:5, 4:9, 3:7], back[1, 0, 2:5, 4:9, 3:7])


@needs_native
def test_chunks_are_byte_identical_to_the_rust_codec_fixture() -> None:
    """Python encodes the committed micro-fixture byte-for-byte.

    The Rust `zarrs` codec pins the same file
    (``fixtures/zfp/tiny-chunk-4x8x8-rate8.zfp``), so this is the
    cross-ecosystem byte-identity gate.
    """
    from conftest import REPO

    shape = [4, 8, 8]
    data = ((np.arange(np.prod(shape), dtype=np.float32) * 7 % 4096) / 3.0).astype(
        np.float32
    )
    blob = _native.nd_zfp_encode(
        data.tobytes(), list(shape), "float32", mode="fixed_rate", rate=8.0
    )
    committed = (REPO / "fixtures" / "zfp" / "tiny-chunk-4x8x8-rate8.zfp").read_bytes()
    assert blob == committed
    back = _native.nd_zfp_decode(
        blob, list(shape), "float32", mode="fixed_rate", rate=8.0
    )
    assert len(back) == data.nbytes


@needs_native
def test_streams_match_the_c_reference_via_imagecodecs() -> None:
    """The differential lane: same input/params ⇒ byte-identical streams to
    the LLNL C library (via ``imagecodecs``), and cross-decode succeeds in
    both directions."""
    imagecodecs = pytest.importorskip("imagecodecs")
    if not getattr(imagecodecs, "ZFP", None):  # pragma: no cover
        pytest.skip("imagecodecs built without ZFP")

    data = ((np.arange(4 * 8 * 8, dtype=np.float32).reshape(4, 8, 8) * 7 % 4096) / 3.0).astype(
        np.float32
    )

    ours = _native.nd_zfp_encode(data.tobytes(), [4, 8, 8], "float32")
    theirs = imagecodecs.zfp_encode(data, mode=imagecodecs.ZFP.MODE.REVERSIBLE)
    assert ours == theirs, "reversible streams must be byte-identical to the C library"

    ours8 = _native.nd_zfp_encode(
        data.tobytes(), [4, 8, 8], "float32", mode="fixed_rate", rate=8.0
    )
    theirs8 = imagecodecs.zfp_encode(data, mode=imagecodecs.ZFP.MODE.FIXED_RATE, level=8)
    assert ours8 == theirs8, "fixed-rate streams must be byte-identical to the C library"

    # Cross-decode: C decodes ours, we decode C's.
    np.testing.assert_array_equal(imagecodecs.zfp_decode(ours), data)
    back = np.frombuffer(
        _native.nd_zfp_decode(theirs, [4, 8, 8], "float32"), dtype=np.float32
    ).reshape(4, 8, 8)
    np.testing.assert_array_equal(back, data)


@needs_native
def test_metadata_round_trips_and_bad_configs_are_refused() -> None:
    codec = NdZfpCodec.from_dict(
        {"name": "nd_zfp", "configuration": {"mode": "fixed_rate", "rate": 8.0}}
    )
    assert codec.to_dict() == {
        "name": "nd_zfp",
        "configuration": {"mode": "fixed_rate", "rate": 8.0, "dims": 3},
    }
    assert NdZfpCodec.from_dict({"name": "nd_zfp"}).to_dict() == {
        "name": "nd_zfp",
        "configuration": {"mode": "reversible", "dims": 3},
    }
    with pytest.raises(ValueError, match="mode"):
        NdZfpCodec(mode="zstd")
    with pytest.raises(ValueError, match="rate"):
        NdZfpCodec(mode="fixed_rate")
    with pytest.raises(ValueError, match="rate"):
        NdZfpCodec(mode="reversible", rate=8.0)
    with pytest.raises(ValueError, match="dims"):
        NdZfpCodec(dims=5)


@needs_native
def test_unsupported_dtypes_are_refused() -> None:
    import zarr.core.dtype as zdt

    codec = NdZfpCodec()
    with pytest.raises(ValueError, match="nd_zfp"):
        codec.validate(shape=(4, 8, 8), dtype=zdt.UInt32(), chunk_grid=None)
