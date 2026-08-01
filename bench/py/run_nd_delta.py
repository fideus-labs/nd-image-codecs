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

import argparse
import json
import pathlib
import statistics
import subprocess
import sys
import time

import numpy as np
import zarr

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "bindings" / "python" / "nd-image-codecs" / "python"))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

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


def git_hash() -> str:
    try:
        return subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=REPO, check=True, capture_output=True, text=True,
        ).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return "unknown"


def split_pipeline(pipeline: list[dict]) -> dict:
    """Split a flat codec list into zarr-python's filters/serializer/compressors."""
    at = next(i for i, c in enumerate(pipeline) if c["name"] == "bytes")
    return {
        "filters": pipeline[:at],
        "serializer": pipeline[at],
        "compressors": pipeline[at + 1 :],
    }


def create(store: dict, data: np.ndarray, pipeline: list[dict]) -> zarr.Array:
    return zarr.create_array(
        store,
        shape=data.shape,
        chunks=CHUNKS,
        dtype=data.dtype,
        dimension_names=AXES,
        fill_value=0,
        **split_pipeline(pipeline),
    )


def stored_bytes(store: dict) -> int:
    return sum(len(v) for k, v in store.items() if k.startswith("c/"))


def bench_lane(lane: str, pipeline: list[dict], data: np.ndarray, samples: int, warmup: int) -> list[dict]:
    """Time encode (write all chunks) and decode (read all chunks) for one lane."""
    encode_ns: list[int] = []
    decode_ns: list[int] = []
    bytes_out = 0
    for i in range(warmup + samples):
        store: dict = {}
        arr = create(store, data, pipeline)
        t0 = time.perf_counter_ns()
        arr[:] = data
        t1 = time.perf_counter_ns()
        back = zarr.open_array(store, mode="r")[:]
        t2 = time.perf_counter_ns()
        if i == 0:
            np.testing.assert_array_equal(back, data)  # never benchmark a broken lane
            bytes_out = stored_bytes(store)
        if i >= warmup:
            encode_ns.append(t1 - t0)
            decode_ns.append(t2 - t1)
    records = []
    for op, raw_ns in [("encode", encode_ns), ("decode", decode_ns)]:
        raw_sorted = sorted(raw_ns)
        mid = len(raw_sorted) // 2
        median = (
            (raw_sorted[mid - 1] + raw_sorted[mid]) // 2
            if len(raw_sorted) % 2 == 0
            else raw_sorted[mid]
        )
        records.append({
            "name": f"nd_delta/{op}_{FIXTURE_SLUG}",
            "config": lane,
            "git_hash": git_hash(),
            "num_samples": len(raw_ns),
            "median_ns": median,
            "min_ns": min(raw_ns),
            "max_ns": max(raw_ns),
            "raw_ns": raw_ns,
            "bytes_in": int(data.nbytes),
            "bytes_out": int(bytes_out),
        })
    return records


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--out", type=pathlib.Path, default=None,
                        help="records root (default target/benchmarks/<git-hash>)")
    parser.add_argument("--lanes", default=",".join(LANES),
                        help="comma-separated lane labels to run")
    args = parser.parse_args()

    out = args.out or REPO / "target" / "benchmarks" / git_hash()
    lanes = [lane.strip() for lane in args.lanes.split(",") if lane.strip()]
    unknown = [lane for lane in lanes if lane not in LANES]
    if unknown:
        parser.error(f"unknown lanes {unknown}; available: {sorted(LANES)}")

    data = microscopy_volume(shape=SHAPE)
    for lane in lanes:
        for record in bench_lane(lane, LANES[lane], data, args.samples, args.warmup):
            lane_dir = out / lane
            lane_dir.mkdir(parents=True, exist_ok=True)
            path = lane_dir / (record["name"].replace("/", "__") + ".json")
            path.write_text(json.dumps(record, indent=2) + "\n")
            gib = record["bytes_in"] / 2**30
            throughput = gib / (record["median_ns"] / 1e9)
            ratio = record["bytes_out"] / record["bytes_in"]
            print(
                f"{record['name']:<28} [{lane:<13}] median "
                f"{record['median_ns'] / 1e6:8.2f} ms  {throughput:6.2f} GiB/s  "
                f"ratio {ratio:.4f}  (±σ {statistics.pstdev(record['raw_ns']) / 1e6:.2f} ms)"
            )
    print(f"records written under {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
