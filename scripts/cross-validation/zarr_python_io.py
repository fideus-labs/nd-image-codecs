#!/usr/bin/env python3
"""zarr-python writer/reader for the cross-ecosystem validation matrix.

One corner of the Phase 6 matrix: ``write`` builds the codec pipeline with
:func:`nd_image_codecs.codec_series` (the pure-Python builder), splits it
into zarr-python's filters/serializer/compressors, and writes the raw input
bytes into a Zarr v3 directory store; ``read`` decodes any store written by
this script, ``ndic zarr write`` (zarrs), or the zarrita.js writer back to
raw little-endian C-order element bytes. The orchestrator
(``scripts/ci/check-cross-validation.py``) drives every write x read
direction pair and compares the bytes.

The case spec is the fixture-matrix schema plus the array ``shape``::

    { "name": "...", "shape": [...], "axes": ["z","y","x"],
      "chunk_shape": [...], "dtype": "uint16", "family": "nd-delta",
      "options": { ... } }
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]

# Prefer an installed wheel (which brings the native extension); fall back to
# the in-repo pure-Python package the way the pytest conftest does. The bench
# lanes' `split_pipeline` is the canonical pipeline splitter — reuse it.
try:
    import nd_image_codecs  # noqa: F401
except ModuleNotFoundError:  # pragma: no cover - CI installs the wheel
    sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(REPO / "bench" / "py"))

import numpy as np  # noqa: E402
import zarr  # noqa: E402

import nd_image_codecs.zarr_codec  # noqa: E402  (registers nd_lift/htj2k/nd_zfp)
from lanes import split_pipeline  # noqa: E402
from nd_image_codecs import codec_series  # noqa: E402


def load_case(path: pathlib.Path) -> dict:
    case = json.loads(path.read_text())
    for key in ("shape", "axes", "chunk_shape", "dtype", "family"):
        if key not in case:
            raise SystemExit(f"case spec is missing {key!r}")
    return case


def write(args: argparse.Namespace) -> None:
    case = load_case(args.spec)
    pipeline = codec_series(
        case["axes"],
        case["chunk_shape"],
        case["dtype"],
        case["family"],
        **case.get("options", {}),
    )
    parts = split_pipeline(pipeline)
    dtype = np.dtype(case["dtype"]).newbyteorder("<")
    data = np.frombuffer(args.input.read_bytes(), dtype=dtype).reshape(case["shape"])
    arr = zarr.create_array(
        str(args.store),
        shape=data.shape,
        chunks=tuple(case["chunk_shape"]),
        dtype=case["dtype"],
        filters=parts["filters"],
        serializer=parts["serializer"],
        compressors=parts["compressors"],
        fill_value=0,
        dimension_names=case["axes"],
    )
    arr[...] = data


def read(args: argparse.Namespace) -> None:
    arr = zarr.open_array(str(args.store), mode="r")
    data = np.ascontiguousarray(arr[...])
    if data.dtype.byteorder == ">":  # pragma: no cover - LE everywhere we run
        data = data.astype(data.dtype.newbyteorder("<"))
    args.output.write_bytes(data.tobytes())


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    w = sub.add_parser("write", help="raw bytes -> Zarr v3 store")
    w.add_argument("--store", type=pathlib.Path, required=True)
    w.add_argument("--spec", type=pathlib.Path, required=True)
    w.add_argument("--input", type=pathlib.Path, required=True)
    w.set_defaults(func=write)

    r = sub.add_parser("read", help="Zarr v3 store -> raw bytes")
    r.add_argument("--store", type=pathlib.Path, required=True)
    r.add_argument("--output", type=pathlib.Path, required=True)
    r.set_defaults(func=read)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
