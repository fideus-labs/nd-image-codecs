"""Shared machinery for the Python-side benchmark lanes.

Each lane runner (``run_nd_delta.py``, ``run_nd_lift.py``) declares a module
slug, a fixture, and a ``lane label → Zarr v3 codec pipeline`` mapping, then
delegates here. Lanes execute through ``zarr-python`` and emit one JSON record
per ``(benchmark, lane)`` in the exact ``BenchRecord`` schema of
``ndic-bench-core``, into the same records tree the Rust workloads use — so
``ndic-bench compare`` diffs and gates them identically.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import statistics
import subprocess
import time

import numpy as np
import zarr

REPO = pathlib.Path(__file__).resolve().parents[2]


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


def stored_bytes(store: dict) -> int:
    return sum(len(v) for k, v in store.items() if k.startswith("c/"))


def bench_lane(
    module: str,
    fixture_slug: str,
    lane: str,
    pipeline: list[dict],
    data: np.ndarray,
    chunks: tuple[int, ...],
    axes: list[str],
    samples: int,
    warmup: int,
) -> list[dict]:
    """Time encode (write all chunks) and decode (read all chunks) for one lane."""
    encode_ns: list[int] = []
    decode_ns: list[int] = []
    bytes_out = 0
    for i in range(warmup + samples):
        store: dict = {}
        arr = zarr.create_array(
            store,
            shape=data.shape,
            chunks=chunks,
            dtype=data.dtype,
            dimension_names=axes,
            fill_value=0,
            **split_pipeline(pipeline),
        )
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
            "name": f"{module}/{op}_{fixture_slug}",
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


def run_lanes(
    description: str,
    module: str,
    fixture_slug: str,
    lanes: dict[str, list[dict]],
    data: np.ndarray,
    chunks: tuple[int, ...],
    axes: list[str],
) -> int:
    """The shared lane-runner CLI: parse args, run lanes, write records."""
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument("--samples", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--out", type=pathlib.Path, default=None,
                        help="records root (default target/benchmarks/<git-hash>)")
    parser.add_argument("--lanes", default=",".join(lanes),
                        help="comma-separated lane labels to run")
    args = parser.parse_args()

    # Reject unusable inputs up front: zero samples leaves `raw_ns` empty and
    # `min()` raises mid-run, a negative warmup silently records a different
    # sample count than requested, and an empty lane list exits 0 having
    # written nothing — which reads like a successful run.
    if args.samples < 1:
        parser.error("--samples must be at least 1")
    if args.warmup < 0:
        parser.error("--warmup must be non-negative")

    out = args.out or REPO / "target" / "benchmarks" / git_hash()
    selected = [lane.strip() for lane in args.lanes.split(",") if lane.strip()]
    if not selected:
        parser.error(f"--lanes must select at least one lane; available: {sorted(lanes)}")
    unknown = [lane for lane in selected if lane not in lanes]
    if unknown:
        parser.error(f"unknown lanes {unknown}; available: {sorted(lanes)}")

    for lane in selected:
        records = bench_lane(
            module, fixture_slug, lane, lanes[lane], data, chunks, axes,
            args.samples, args.warmup,
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
                f"{record['name']:<28} [{lane:<18}] median "
                f"{record['median_ns'] / 1e6:8.2f} ms  {throughput:6.2f} GiB/s  "
                f"ratio {ratio:.4f}  (±σ {statistics.pstdev(record['raw_ns']) / 1e6:.2f} ms)"
            )
    print(f"records written under {out}")
    return 0
