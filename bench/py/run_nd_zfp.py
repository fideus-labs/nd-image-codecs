#!/usr/bin/env python3
"""Phase 5 nd-zfp benchmark lanes.

Measures the ``nd_zfp`` codec through ``zarr-python`` on a correlated
**float32** volume — the family's target data. The pipeline is authored by
``nd_image_codecs.codec_series`` (``transpose → nd_zfp``) and the codec runs
through :mod:`nd_image_codecs.zarr_codec`, i.e. the same native core as the
Rust ``zarrs`` codec (byte-identical chunks).

Lanes, all on the float cast of :func:`synthetic.correlated_zstack`:

- ``blosc-zstd`` — a stock ``bytes → blosc(zstd)`` float pipeline, the
  comparison bar (floats give bitshuffle-zstd little to work with).
- ``zfp-reversible`` — lossless ZFP.
- ``zfp-rate8`` — fixed-rate 8 bits/value (the GPU-brick budget; lossy,
  verified within the lane's declared error bound).

Records land in the shared tree, so the ``ndic-bench compare`` ratio gate
holds the compression ratios:

    python3 bench/py/run_nd_zfp.py
    cargo run -p ndic-bench-cli --release -- compare bench/baselines/main

Usage: run_nd_zfp.py [--samples N] [--warmup N] [--out DIR] [--lanes a,b]
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
# An *installed* package wins over the source tree: it carries the native
# extension module nd_zfp needs, which the bare source tree does not.
if importlib.util.find_spec("nd_image_codecs") is None:
    sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import numpy as np  # noqa: E402

import nd_image_codecs.zarr_codec  # noqa: E402, F401  (registers nd_zfp with zarr)
from lanes import run_lanes  # noqa: E402
from nd_image_codecs import codec_series  # noqa: E402
from synthetic import correlated_zstack  # noqa: E402

AXES = ["t", "c", "z", "y", "x"]
SHAPE = (8, 2, 32, 128, 128)  # 32 MiB of float32
CHUNKS = (4, 1, 16, 128, 128)
FIXTURE_SLUG = "tczyx_corrf32_32mib"

# The stock float baseline: no transform, bitshuffle-zstd over raw floats.
BLOSC_PIPELINE = [
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

LANES: dict[str, list[dict]] = {
    "blosc-zstd": BLOSC_PIPELINE,
    "zfp-reversible": codec_series(AXES, list(CHUNKS), "float32", "nd-zfp"),
    "zfp-rate8": codec_series(AXES, list(CHUNKS), "float32", "nd-zfp", zfp_rate=8.0),
}

# Fixed-rate 8 bits/value on the smooth fixture stays inside ±8 absolute
# (~0.15% of the ~0..5300 value span); the bound is generous on purpose —
# precision guarantees are pinned by the pytest suite, this only rejects a
# broken lane.
LOSSY_ATOL = {"zfp-rate8": 8.0}


def main() -> int:
    data = correlated_zstack(shape=SHAPE).astype(np.float32)
    return run_lanes(
        __doc__, "nd_zfp", FIXTURE_SLUG, LANES, data, CHUNKS, AXES,
        lossy_atol=LOSSY_ATOL,
    )


if __name__ == "__main__":
    raise SystemExit(main())
