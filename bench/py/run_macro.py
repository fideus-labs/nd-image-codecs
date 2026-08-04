#!/usr/bin/env python3
"""Macro-tier benchmark lanes on the Tier 3 domain volumes.

Runs the three families plus the ``blosc-zstd`` reference lane over real
OME-Zarr data — the anisotropic-z EM stack and the multi-timepoint
fluorescence series pinned in ``scripts/bench-data.lock.toml`` — where the
decorrelation gain a synthetic generator can only approximate is the actual
measurement. Records land in the same tree and schema as every other lane, so
``ndic-bench compare`` gates them identically.

The volumes are fetched, not committed, so this runner **skips cleanly** when
the cache is empty: the nightly workflow fetches first, and a developer runs
``scripts/fetch-bench-data.sh`` once. Selecting a volume explicitly with
``--volume`` turns a missing cache into an error instead, so a workflow that
means to measure cannot silently measure nothing.

Usage: run_macro.py [--volume SLUG] [--samples N] [--warmup N] [--out DIR]
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import pathlib
import statistics
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
# An *installed* package wins over the source tree: it carries the native
# extension module the htj2k and nd_zfp lanes need, which the bare source
# tree does not.
if importlib.util.find_spec("nd_image_codecs") is None:
    sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import tier3  # noqa: E402
from lanes import bench_lane, git_hash  # noqa: E402
from nd_image_codecs import codec_series  # noqa: E402

#: Chunk shape per pinned volume, sized like a real OME-Zarr chunking (a few
#: MiB) and keeping nd-zfp within its 4-non-singleton-dimension limit.
CHUNKS = {
    "idr0062A-anisotropic-z": (1, 32, 137, 135),
    "idr0052A-timeseries": (4, 1, 31, 64, 64),
}


def lanes_for(axes: list[str], chunks: tuple[int, ...], dtype: str) -> dict[str, list[dict]]:
    """The macro lane set: the reference lane plus one per family."""
    itemsize = 2 if dtype == "uint16" else 4
    return {
        "blosc-zstd": [
            {"name": "bytes", "configuration": {"endian": "little"}},
            {
                "name": "blosc",
                "configuration": {
                    "cname": "zstd",
                    "clevel": 5,
                    "shuffle": "shuffle",
                    "typesize": itemsize,
                    "blocksize": 0,
                },
            },
        ],
        "nd-delta-zstd": codec_series(axes, list(chunks), dtype, "nd-delta"),
        "simd-53-lift-z2": codec_series(axes, list(chunks), dtype, "nd-lift-ht", xy_levels=5),
        "zfp-reversible": codec_series(axes, list(chunks), dtype, "nd-zfp"),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--volume", help="one slug (missing cache becomes an error)")
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--warmup", type=int, default=1)
    parser.add_argument("--out", type=pathlib.Path, default=None)
    parser.add_argument("--lanes", default=None, help="comma-separated lane labels")
    args = parser.parse_args()
    if args.samples < 1:
        parser.error("--samples must be at least 1")
    if args.warmup < 0:
        parser.error("--warmup must be non-negative")

    slugs = [args.volume] if args.volume else [v["slug"] for v in tier3.volumes()]
    missing = [slug for slug in slugs if not tier3.available(slug)]
    if args.volume and missing:
        print(f"{args.volume} is not cached; run scripts/fetch-bench-data.sh --only {args.volume}",
              file=sys.stderr)
        return 1
    slugs = [slug for slug in slugs if slug not in missing]
    if not slugs:
        print("no Tier 3 volumes cached; skipping the macro lanes "
              "(run scripts/fetch-bench-data.sh to enable them)")
        return 0

    missing_chunks = [slug for slug in slugs if slug not in CHUNKS]
    if missing_chunks:
        print(
            f"no CHUNKS entry for {missing_chunks} in bench/py/run_macro.py; "
            "a new lock-file volume needs a chunk shape here before it can be benched",
            file=sys.stderr,
        )
        return 1

    out = args.out or REPO / "target" / "benchmarks" / git_hash()
    for slug in slugs:
        volume = tier3.entry(slug)
        data = tier3.load(slug)
        chunks = CHUNKS[slug]
        axes = volume["axes"]
        lanes = lanes_for(axes, chunks, volume["dtype"])
        selected = (
            [lane.strip() for lane in args.lanes.split(",") if lane.strip()]
            if args.lanes
            else list(lanes)
        )
        unknown = [lane for lane in selected if lane not in lanes]
        if unknown:
            parser.error(f"unknown lanes {unknown}; available: {sorted(lanes)}")
        print(f"{slug}: {list(data.shape)} {data.dtype}, chunks {chunks}")
        for lane in selected:
            records = bench_lane(
                "macro", slug.replace("-", "_"), lane, lanes[lane],
                data, chunks, axes, args.samples, args.warmup,
            )
            for record in records:
                lane_dir = out / lane
                lane_dir.mkdir(parents=True, exist_ok=True)
                path = lane_dir / (record["name"].replace("/", "__") + ".json")
                path.write_text(json.dumps(record, indent=2) + "\n")
                gib = record["bytes_in"] / 2**30
                throughput = gib / (record["median_ns"] / 1e9)
                ratio = record["bytes_out"] / record["bytes_in"]
                print(
                    f"{record['name']:<40} [{lane:<18}] median "
                    f"{record['median_ns'] / 1e6:8.2f} ms  {throughput:6.2f} GiB/s  "
                    f"ratio {ratio:.4f}  (±σ {statistics.pstdev(record['raw_ns']) / 1e6:.2f} ms)"
                )
    print(f"records written under {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
