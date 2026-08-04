"""Third-party validation against ``imagecodecs`` (JPEG 2000 and delta).

Where codec semantics overlap, ``imagecodecs`` must accept our output and we
must accept its — the Phase 6 third-party lane of the validation matrix:

- **JPEG 2000**: every plane codestream inside an ``htj2k`` chunk container
  is a conforming Part 15 (HTJ2K) codestream, so ``imagecodecs.jpeg2k_decode``
  (OpenJPEG >= 2.5) must reproduce the plane bit-exactly — including the
  ``int32`` coefficient planes ``nd_lift`` hands down, whose per-plane
  ``Ssiz`` declares the actual dynamic range. The reverse direction has no
  overlap to test: ``imagecodecs.jpeg2k_encode`` emits classic Part 1 EBCOT
  codestreams, and this project's block decoder is deliberately HT-only
  (decoding third-party *HT* output is covered by the OpenJPH corpus and
  differential suites under ``crates/``).
- **delta**: the nd-delta family stores ``numcodecs.delta`` output, which
  must be byte-identical to ``imagecodecs.delta_encode`` and cross-decode in
  both directions.
- **ZFP**: covered by ``test_nd_zfp_roundtrip.py``, which pins byte-identity
  with ``imagecodecs.zfp_encode`` both ways.
"""

from __future__ import annotations

import struct

import numpy as np
import pytest

imagecodecs = pytest.importorskip("imagecodecs")
numcodecs = pytest.importorskip("numcodecs")

try:
    from nd_image_codecs import _nd_image_codecs as _native
except ImportError:  # pragma: no cover - pure-Python checkout without the wheel
    _native = None

needs_native = pytest.mark.skipif(
    _native is None, reason="native extension not built (pip install the wheel)"
)


def ndht_planes(blob: bytes) -> list[bytes]:
    """Parse an ``htj2k`` chunk container into its plane codestreams.

    Mirrors the version 1 layout in ``ndic-codestream/src/container.rs``:
    magic, version, flags, xy_levels, ndim, ``u32`` dims (transformed order,
    trailing two = y, x), then — with the index flag — per plane
    ``u64 offset | u32 len | (xy_levels + 1) * u32`` resolution prefixes.
    """
    assert blob[:4] == b"ndht", "not an htj2k chunk container"
    version, flags, xy_levels, ndim = blob[4], blob[5], blob[6], blob[7]
    assert version == 1
    assert flags & 0x01, "these tests encode with the coefficient-plane index"
    dims = struct.unpack_from(f"<{ndim}I", blob, 8)
    offset = 8 + 4 * ndim
    num_planes = int(np.prod(dims[:-2], dtype=np.int64)) if ndim > 2 else 1
    planes = []
    for _ in range(num_planes):
        plane_offset, plane_len = struct.unpack_from("<QI", blob, offset)
        offset += 12 + 4 * (xy_levels + 1)
        planes.append(blob[plane_offset : plane_offset + plane_len])
    return planes


@needs_native
def test_imagecodecs_decodes_every_plane_of_an_htj2k_chunk() -> None:
    """OpenJPEG reproduces each uint16 plane of a 3D chunk bit-exactly."""
    shape = [4, 16, 16]
    data = (np.arange(np.prod(shape), dtype=np.uint16) * 7 % 4096).reshape(shape)
    blob = _native.htj2k_encode(data.tobytes(), list(shape), "uint16", xy_levels=2)
    planes = ndht_planes(blob)
    assert len(planes) == 4
    for z, codestream in enumerate(planes):
        assert codestream[:2] == b"\xff\x4f", "SOC opens every plane codestream"
        img = imagecodecs.jpeg2k_decode(codestream)
        np.testing.assert_array_equal(img, data[z])


@needs_native
def test_imagecodecs_decodes_int32_coefficient_planes() -> None:
    """The nd_lift plane path: signed values, Ssiz declaring actual range.

    OpenJPEG chooses the narrowest output dtype for the declared precision,
    so compare after widening.
    """
    shape = [16, 16]
    plane = ((np.arange(256, dtype=np.int32) * 11 % 4001) - 2000).reshape(shape)
    blob = _native.htj2k_encode(plane.tobytes(), list(shape), "int32", xy_levels=2)
    (codestream,) = ndht_planes(blob)
    img = imagecodecs.jpeg2k_decode(codestream)
    np.testing.assert_array_equal(img.astype(np.int32), plane)


@pytest.mark.parametrize("dtype", ["|u1", "<u2", "<i2", "<u4", "<i4", "<u8", "<i8"])
def test_delta_streams_match_imagecodecs_both_ways(dtype: str) -> None:
    """numcodecs.delta output == imagecodecs delta output, cross-decoded.

    Integer dtypes only: on floats the two disagree by design (numcodecs
    subtracts float values, imagecodecs diffs the underlying bit patterns),
    so there is no overlapping semantics to validate there.
    """
    dt = np.dtype(dtype)
    arr = (np.arange(1000) * 13 % 5000).astype(dt)
    codec = numcodecs.Delta(dtype=dtype)

    ours = np.frombuffer(codec.encode(arr), dtype=dt)
    theirs = imagecodecs.delta_encode(arr)
    np.testing.assert_array_equal(ours, theirs)

    # They accept ours; we accept theirs.
    np.testing.assert_array_equal(imagecodecs.delta_decode(ours.copy()), arr)
    np.testing.assert_array_equal(
        np.frombuffer(codec.decode(theirs.tobytes()), dtype=dt), arr
    )


def test_delta_wrapping_matches() -> None:
    """Alternating extremes overflow every delta; both wrap identically."""
    arr = np.where(np.arange(64) % 2 == 0, 0, 255).astype(np.uint8)
    codec = numcodecs.Delta(dtype="|u1")
    ours = np.frombuffer(codec.encode(arr), dtype=np.uint8)
    theirs = imagecodecs.delta_encode(arr)
    np.testing.assert_array_equal(ours, theirs)
    np.testing.assert_array_equal(imagecodecs.delta_decode(ours.copy()), arr)
