#!/usr/bin/env python3
"""Phase 1 nd-delta benchmark lanes.

Measures encode/decode throughput and compression ratio for the nd-delta
family (``nd-delta-zstd``, ``nd-delta-lz4``) against the plain ``blosc-zstd``
baseline lane, all through ``zarr-python`` on the deterministic synthetic
microscopy fixture from :mod:`synthetic`. Pipelines are authored by
``nd_image_codecs.codec_series`` — the same builder users call.

One JSON record per ``(benchmark, lane)`` is written to
``target/benchmarks/<git-hash>/<lane>/nd_delta__<name>.json`` in the exact
`BenchRecord` schema of ``ndic-bench-core``, so ``ndic-bench compare`` diffs
and gates these records like any Rust-side benchmark:

    python3 bench/py/run_nd_delta.py
    cargo run -p ndic-bench-cli --release -- compare bench/baselines/main

Usage: run_nd_delta.py [--samples N] [--warmup N] [--out DIR] [--lanes a,b]
"""

from __future__ import annotations

import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from lanes import run_lanes  # noqa: E402
from nd_image_codecs import codec_series  # noqa: E402
from synthetic import microscopy_volume  # noqa: E402

AXES = ["t", "c", "z", "y", "x"]
SHAPE = (8, 2, 32, 128, 128)  # 16 MiB of uint16
CHUNKS = (4, 1, 16, 128, 128)
FIXTURE_SLUG = "tczyx_16mib"

# lane label -> full Zarr v3 codec pipeline
LANES: dict[str, list[dict]] = {
    "blosc-zstd": [
        {"name": "bytes", "configuration": {"endian": "little"}},
        {
            "name": "blosc",
            "configuration": {
                "cname": "zstd",
                "clevel": 5,
                "shuffle": "shuffle",
                "typesize": 2,
                "blocksize": 0,
            },
        },
    ],
    "nd-delta-zstd": codec_series(AXES, list(CHUNKS), "uint16", "nd-delta"),
    "nd-delta-lz4": codec_series(AXES, list(CHUNKS), "uint16", "nd-delta", delta_backend="lz4"),
}


def main() -> int:
    data = microscopy_volume(shape=SHAPE)
    return run_lanes(__doc__, "nd_delta", FIXTURE_SLUG, LANES, data, CHUNKS, AXES)


if __name__ == "__main__":
    raise SystemExit(main())
