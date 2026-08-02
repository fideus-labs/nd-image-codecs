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


@pytest.mark.parametrize("dtype", ["uint32", "uint64"])
def test_plane_range_checked_without_transforms(dtype: str) -> None:
    # Widening happens whether or not there are transforms, so an unsigned
    # sample above the signed plane's maximum must be refused either way —
    # otherwise `astype` wraps it to a negative coefficient silently.
    above_plane = 2**31 if dtype == "uint32" else 2**63
    data = np.full((4,), above_plane, dtype=dtype)
    with pytest.raises(OverflowError, match="does not fit the widened"):
        _lift.forward(data, [])
    step = [{"axis": "z", "dimension": 0, "kind": "delta", "levels": 0, "group": 0}]
    with pytest.raises(OverflowError, match="does not fit the widened"):
        _lift.forward(data, step)


def test_empty_transform_list_widens_in_range_data() -> None:
    # The other half of the budget split: with no transforms there is nothing
    # to propagate, so an in-range chunk must widen cleanly rather than be
    # caught by the range check that now runs unconditionally.
    data = np.array([[0, 7], [65535, 3]], dtype=np.uint16)
    coeffs = _lift.forward(data, [])
    assert coeffs.dtype == np.dtype(np.int32)
    np.testing.assert_array_equal(coeffs, data.astype(np.int32))
    np.testing.assert_array_equal(_lift.inverse(coeffs, [], data.dtype), data)


def test_malformed_transforms_raise_value_errors() -> None:
    data = np.zeros((4, 4), dtype=np.uint16)
    # Missing required fields are argument errors, not KeyErrors.
    for bad in [{"kind": "delta"}, {"dimension": 0}]:
        with pytest.raises(ValueError, match="missing required field"):
            _lift.forward(data, [bad])
    # A negative group would make the segment stride negative, silently
    # skipping the transform on both encode and decode.
    step = [{"axis": "z", "dimension": 0, "kind": "haar", "levels": 1, "group": -1}]
    with pytest.raises(ValueError, match="group -1 must be >= 0"):
        _lift.forward(data, step)
    with pytest.raises(ValueError, match="group -1 must be >= 0"):
        _lift.inverse(data.astype(np.int32), step, np.dtype(np.uint16))


def test_from_config_requires_an_explicit_version() -> None:
    with pytest.raises(ValueError, match='explicit "version"'):
        NdLift.from_config({"transforms": []})
    assert NdLift.from_config({"version": "0.1", "transforms": []}).version == "0.1"


def test_version_must_be_a_string() -> None:
    # The Zarr metadata types `version` as a string and Rust's NdLiftConfig
    # declares it `String`, so the JSON number 0.1 must not decode here and
    # then fail serde on the Rust side.
    for bad in [0.1, 1, None, ["0", "1"]]:
        with pytest.raises(ValueError, match="version must be a string"):
            _lift.check_version(bad)
        with pytest.raises(ValueError, match="version must be a string"):
            NdLift.from_config({"version": bad, "transforms": []})


def test_integer_fields_reject_coercible_non_integers() -> None:
    # Bare int() would silently truncate: `levels: 2.9` -> 2 quietly changes
    # the decomposition depth. Rust's serde refuses these outright.
    data = np.zeros((4, 4), dtype=np.uint16)

    def step(**over: object) -> list[dict]:
        base = {"axis": "z", "dimension": 0, "kind": "haar", "levels": 2, "group": 0}
        return [{**base, **over}]

    for field, value in [
        ("levels", 2.9),
        ("group", 8.5),
        ("dimension", 1.5),
        ("levels", "2"),
        ("group", True),
        ("dimension", None),
    ]:
        with pytest.raises(ValueError, match=f"{field} must be an integer"):
            _lift.forward(data, step(**{field: value}))

    # Genuine integers still pass, including NumPy integer scalars.
    _lift.forward(data, step(levels=np.int64(2), group=np.int32(4), dimension=np.int8(0)))


def test_integer_fields_respect_the_rust_widths() -> None:
    # `levels` is a u8 and `group` a u32 in AxisTransform; serde refuses
    # anything wider, so a value that only fits a Python int must not slip by.
    data = np.zeros((4, 4), dtype=np.uint16)

    def step(**over: object) -> list[dict]:
        base = {"axis": "z", "dimension": 0, "kind": "haar", "levels": 2, "group": 0}
        return [{**base, **over}]

    with pytest.raises(ValueError, match=r"levels 256 must be >= 0 and <= 255 \(u8\)"):
        _lift.forward(data, step(levels=256))
    with pytest.raises(ValueError, match=r"group 4294967296 must be .*<= 4294967295 \(u32\)"):
        _lift.forward(data, step(group=2**32))
    # The widest in-range values are still accepted.
    _lift.forward(data, step(levels=255, group=0xFFFFFFFF))


def test_transforms_must_be_mappings() -> None:
    data = np.zeros((4, 4), dtype=np.uint16)
    for bad in ["delta", 3, None, ["dimension"]]:
        with pytest.raises(ValueError, match="transform must be a mapping"):
            _lift.forward(data, [bad])


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
    zarr = pytest.importorskip("zarr", minversion="3.1")
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


def test_zarr_codec_is_hashable_and_requires_a_version() -> None:
    pytest.importorskip("zarr", minversion="3.1")
    from nd_image_codecs.zarr_codec import NdLiftCodec

    transforms = [{"axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0}]
    codec = NdLiftCodec(transforms=transforms)
    # zarr hashes codecs (cached pipeline lookups); the frozen dataclass's
    # generated __hash__ would raise on the dict-valued `transforms` field.
    assert hash(codec) == hash(NdLiftCodec(transforms=transforms))
    assert codec == NdLiftCodec(transforms=transforms)
    assert len({codec, NdLiftCodec(transforms=transforms)}) == 1
    assert hash(codec) != hash(NdLiftCodec(transforms=[]))

    with pytest.raises(ValueError, match='explicit "version"'):
        NdLiftCodec.from_dict({"name": "nd_lift", "configuration": {"transforms": transforms}})
    restored = NdLiftCodec.from_dict(codec.to_dict())
    assert restored == codec

    # The constructor copies each transform, so mutating the caller's dict
    # afterwards must not shift the codec's hash and strand it as a dict key.
    live = [{"axis": "z", "dimension": 0, "kind": "lift53", "levels": 2, "group": 0}]
    keyed = NdLiftCodec(transforms=live)
    table = {keyed: "value"}
    live[0]["levels"] = 99
    live[0]["axis"] = "t"
    assert table[keyed] == "value", "codec key must survive mutation of the source dict"
    assert keyed.to_dict()["configuration"]["transforms"][0]["levels"] == 2


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
