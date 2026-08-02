"""``nd_lift`` NumPy reference implementation and zarr-python codec tests.

Pins the NumPy transform to the committed conformance vectors
(``fixtures/nd-lift/vectors.json``) — the same file the Rust test suite
enforces, so the two implementations cannot drift apart — and validates the
end-to-end ``transpose → nd_lift → bytes → blosc`` pipeline through
``zarr-python`` (roadmap Phase 2).
"""

from __future__ import annotations

import json
import pathlib

import numpy as np
import pytest

from nd_image_codecs import NdLift, _lift

VECTORS = pathlib.Path(__file__).resolve().parents[4] / "fixtures" / "nd-lift" / "vectors.json"


def vector_cases() -> list[dict]:
    return json.loads(VECTORS.read_text())["cases"]


@pytest.mark.parametrize("case", vector_cases(), ids=lambda c: c["name"])
def test_conformance_vectors(case: dict) -> None:
    plane = np.int32 if case["plane"] == "i32" else np.int64
    shape = tuple(case["shape"])
    cfg = case["configuration"]
    data = np.asarray(case["input"], dtype=plane).reshape(shape)
    expected = np.asarray(case["expected"], dtype=plane).reshape(shape)
    coeffs = _lift.forward(data, cfg["transforms"], cfg["version"])
    np.testing.assert_array_equal(coeffs, expected, err_msg=f"forward mismatch: {case['name']}")
    back = _lift.inverse(coeffs, cfg["transforms"], data.dtype, cfg["version"])
    np.testing.assert_array_equal(back, data, err_msg=f"round-trip mismatch: {case['name']}")


@pytest.mark.parametrize("kind", ["delta", "haar", "lift53"])
@pytest.mark.parametrize(
    "dtype", ["uint8", "int8", "uint16", "int16", "uint32", "int32", "uint64", "int64"]
)
def test_roundtrip_every_integer_dtype(kind: str, dtype: str) -> None:
    rng = np.random.default_rng(3)
    info = np.iinfo(dtype)
    # Use the full dtype range where it fits the overflow budget; cap 32/64-bit
    # dtypes so multi-level growth stays inside their coefficient plane.
    cap = 2**24 if np.dtype(dtype).itemsize <= 4 else 2**40
    lo, hi = max(info.min, -cap), min(info.max, cap)
    data = rng.integers(lo, hi + 1, size=(7, 3, 6, 5), dtype=dtype)
    transforms = [
        {"axis": "z", "dimension": 0, "kind": kind, "levels": 2, "group": 0},
        {"axis": "t", "dimension": 2, "kind": kind, "levels": 2, "group": 4},
    ]
    coeffs = _lift.forward(data, transforms)
    expected_plane = np.int32 if np.dtype(dtype).itemsize <= 4 else np.int64
    assert coeffs.dtype == expected_plane
    back = _lift.inverse(coeffs, transforms, data.dtype)
    np.testing.assert_array_equal(back, data)


def test_version_gate() -> None:
    data = np.zeros((4,), dtype=np.uint16)
    step = [{"axis": "z", "dimension": 0, "kind": "delta", "levels": 0, "group": 0}]
    for bad in ["0.2", "1.0", "nonsense"]:
        with pytest.raises(ValueError, match="not supported"):
            _lift.forward(data, step, version=bad)
    with pytest.raises(ValueError, match="not supported"):
        NdLift(transforms=step, version="1.0")


def test_overflow_budget_refused() -> None:
    data = np.full((8,), np.uint32(2**31 - 1), dtype=np.uint32)
    step = [{"axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0}]
    with pytest.raises(OverflowError, match="overflow budget"):
        _lift.forward(data, step)


def test_config_class_serializes_rust_accepted_configs() -> None:
    codec = NdLift(
        transforms=[{"axis": "z", "dimension": 2, "kind": "lift53", "levels": 2, "group": 0}]
    )
    assert codec.to_dict() == {
        "name": "nd_lift",
        "configuration": {
            "version": "0.1",
            "transforms": [
                {"axis": "z", "dimension": 2, "kind": "lift53", "levels": 2, "group": 0}
            ],
        },
    }
    again = NdLift.from_config(codec.to_dict()["configuration"])
    assert again.to_dict() == codec.to_dict()


# ---------------------------------------------------------------------------
# zarr-python integration
# ---------------------------------------------------------------------------
def _zarr_roundtrip_pipeline(
    data: np.ndarray, chunks: tuple[int, ...], pipeline: list[dict]
) -> tuple[np.ndarray, int]:
    zarr = pytest.importorskip("zarr", minversion="3.0")
    import nd_image_codecs.zarr_codec  # noqa: F401  (registers nd_lift)

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
    return zarr.open_array(store, mode="r")[:], stored


@pytest.mark.parametrize("kind", ["delta", "lift53"])
def test_zarr_pipeline_roundtrip(kind: str) -> None:
    from synthetic import microscopy_volume

    data = microscopy_volume(shape=(1, 1, 16, 32, 32))[0, 0]
    levels = 0 if kind == "delta" else 2
    pipeline = [
        {"name": "transpose", "configuration": {"order": [0, 1, 2]}},
        {
            "name": "nd_lift",
            "configuration": {
                "version": "0.1",
                "transforms": [
                    {"axis": "z", "dimension": 0, "kind": kind, "levels": levels, "group": 0}
                ],
            },
        },
        {"name": "bytes", "configuration": {"endian": "little"}},
        {
            "name": "blosc",
            "configuration": {
                "cname": "zstd",
                "clevel": 5,
                "shuffle": "bitshuffle",
                "typesize": 4,
                "blocksize": 0,
            },
        },
    ]
    back, stored = _zarr_roundtrip_pipeline(data, (8, 32, 32), pipeline)
    np.testing.assert_array_equal(back, data)
    assert stored < data.nbytes, "the lift pipeline must compress correlated volumes"


def test_zarr_pipeline_partial_edge_chunks() -> None:
    from synthetic import microscopy_volume

    data = microscopy_volume(shape=(1, 1, 24, 50, 50))[0, 0]
    pipeline = [
        {
            "name": "nd_lift",
            "configuration": {
                "version": "0.1",
                "transforms": [
                    {"axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0}
                ],
            },
        },
        {"name": "bytes", "configuration": {"endian": "little"}},
    ]
    back, _ = _zarr_roundtrip_pipeline(data, (16, 32, 32), pipeline)
    np.testing.assert_array_equal(back, data)
