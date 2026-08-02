#!/usr/bin/env python3
"""Phase 2 nd-lift benchmark lanes.

Measures the ``nd_lift`` decorrelation gain in isolation — behind a stock
Blosc-Zstd backend, before the HTJ2K plane codec exists. The validation
series is ``transpose → nd_lift → bytes → blosc(zstd)``; the pipeline head is
authored by ``nd_image_codecs.codec_series`` (the nd-lift-ht family, with the
``htj2k`` tail swapped for ``bytes → blosc``) and the ``nd_lift`` codec runs
through :mod:`nd_image_codecs.zarr_codec` — the NumPy port pinned bit-exact to
the Rust crate by the committed conformance vectors.

Lanes, all on the strongly correlated z-stack fixture
(:func:`synthetic.correlated_zstack`):

- ``nd-delta-zstd`` — the Phase 1 family on this fixture, the comparison bar.
- ``nd-lift-delta-zstd`` — ``nd_lift`` first-order delta along t and z.
- ``nd-lift-53-zstd`` — ``nd_lift`` two-level 5/3 lifting along t and z.

Records land in the shared tree, so the ``ndic-bench compare`` ratio gate
holds the decorrelation gain:

    python3 bench/py/run_nd_lift.py
    cargo run -p ndic-bench-cli --release -- compare bench/baselines/main

Usage: run_nd_lift.py [--samples N] [--warmup N] [--out DIR] [--lanes a,b]
"""

from __future__ import annotations

import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import nd_image_codecs.zarr_codec  # noqa: E402, F401  (registers nd_lift with zarr)
from lanes import run_lanes  # noqa: E402
from nd_image_codecs import codec_series  # noqa: E402
from synthetic import correlated_zstack  # noqa: E402

AXES = ["t", "c", "z", "y", "x"]
SHAPE = (8, 2, 32, 128, 128)  # 16 MiB of uint16
CHUNKS = (4, 1, 16, 128, 128)
FIXTURE_SLUG = "tczyx_corr_16mib"

# The stock backend behind the transform: the nd_lift output plane is int32.
BLOSC_TAIL = [
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


def lift_series(lift: str) -> list[dict]:
    """`transpose → nd_lift` from the builder, then the stock blosc tail."""
    head = codec_series(AXES, list(CHUNKS), "uint16", "nd-lift-ht", lift=lift)
    assert head[-1]["name"] == "htj2k", "builder must end with the plane codec"
    return head[:-1] + BLOSC_TAIL


LANES: dict[str, list[dict]] = {
    "nd-delta-zstd": codec_series(AXES, list(CHUNKS), "uint16", "nd-delta"),
    "nd-lift-delta-zstd": lift_series("delta"),
    "nd-lift-53-zstd": lift_series("lift53"),
}


def main() -> int:
    data = correlated_zstack(shape=SHAPE)
    return run_lanes(__doc__, "nd_lift", FIXTURE_SLUG, LANES, data, CHUNKS, AXES)


if __name__ == "__main__":
    raise SystemExit(main())
