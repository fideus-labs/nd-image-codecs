#!/usr/bin/env python3
"""Tier 3 benchmark volumes: fetch, verify, and load.

Real domain data for the macro-tier bench lanes, pinned by URL and by the
SHA-256 of its decoded bytes in ``scripts/bench-data.lock.toml``. Run as a
script (via ``scripts/fetch-bench-data.sh``) it fetches and verifies; imported
by a lane runner, :func:`load` returns whatever is cached and :func:`available`
says whether a lane can run at all — the macro lanes skip cleanly on a machine
that has never fetched, which is every CI runner except the nightly one.

The digest covers the *decoded* C-order little-endian bytes rather than any
stored representation, so an upstream re-chunk or recompression shows up as
the content change it is.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import pathlib
import sys
import tomllib

import numpy as np

REPO = pathlib.Path(__file__).resolve().parents[2]
LOCK = REPO / "scripts" / "bench-data.lock.toml"


def cache_dir() -> pathlib.Path:
    override = os.environ.get("NDIC_BENCH_DATA_DIR")
    if override:
        return pathlib.Path(override)
    return pathlib.Path.home() / ".cache" / "nd-image-codecs" / "bench-data"


def volumes() -> list[dict]:
    """Every pinned Tier 3 volume, in lock-file order."""
    return tomllib.loads(LOCK.read_text()).get("volume", [])


def entry(slug: str) -> dict:
    for volume in volumes():
        if volume["slug"] == slug:
            return volume
    raise KeyError(f"no Tier 3 volume named {slug!r} in {LOCK}")


def path_for(slug: str) -> pathlib.Path:
    return cache_dir() / f"{slug}.npy"


def available(slug: str) -> bool:
    """Is this volume cached? Macro lanes gate on this."""
    return path_for(slug).is_file()


def load(slug: str) -> np.ndarray:
    """Load a cached volume, checking it still matches the lock file."""
    volume = entry(slug)
    path = path_for(slug)
    if not path.is_file():
        raise FileNotFoundError(
            f"{path} is not cached; run scripts/fetch-bench-data.sh --only {slug}"
        )
    data = np.load(path)
    if list(data.shape) != volume["shape"] or data.dtype.name != volume["dtype"]:
        raise ValueError(
            f"{slug}: cached {list(data.shape)} {data.dtype} does not match the "
            f"lock file's {volume['shape']} {volume['dtype']}"
        )
    return data


def digest(data: np.ndarray) -> str:
    return hashlib.sha256(np.ascontiguousarray(data).tobytes()).hexdigest()


def fetch(volume: dict, check_only: bool) -> bool:
    """Fetch (or verify) one volume. Returns True when it ends up valid."""
    slug = volume["slug"]
    path = path_for(slug)
    if path.is_file():
        data = np.load(path)
        got = digest(data)
        if got == volume["sha256"]:
            print(f"  {slug}: cached, digest ok")
            return True
        print(f"  {slug}: cached copy has digest {got}, expected {volume['sha256']}",
              file=sys.stderr)
        if check_only:
            return False
        print(f"  {slug}: re-fetching")
    elif check_only:
        print(f"  {slug}: not cached", file=sys.stderr)
        return False

    import zarr  # imported here so --check works without a remote-capable zarr

    print(f"  {slug}: fetching {volume['url']}", flush=True)
    array = zarr.open_array(volume["url"], mode="r")
    data = np.ascontiguousarray(array[...])
    got = digest(data)
    if got != volume["sha256"]:
        print(
            f"  {slug}: fetched digest {got} != pinned {volume['sha256']}\n"
            f"    The upstream data changed. Do not silently re-pin: confirm the "
            f"change is intended, then update the lock file and refresh any "
            f"baselines captured against the old content in the same PR.",
            file=sys.stderr,
        )
        return False
    if data.nbytes != volume["raw_bytes"]:
        print(f"  {slug}: {data.nbytes} bytes, lock file says {volume['raw_bytes']}",
              file=sys.stderr)
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    np.save(path, data)
    print(f"  {slug}: {data.nbytes / 1e6:.1f} MB cached at {path}")
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true",
                        help="verify the cache without fetching")
    parser.add_argument("--only", help="operate on one slug")
    args = parser.parse_args()

    selected = volumes()
    if args.only:
        selected = [v for v in selected if v["slug"] == args.only]
        if not selected:
            print(f"no Tier 3 volume named {args.only!r}", file=sys.stderr)
            return 1

    print(f"Tier 3 benchmark volumes -> {cache_dir()}")
    # Materialized first: `all()` on a generator would stop at the first
    # failure and leave every later volume unfetched and unreported.
    results = [fetch(volume, args.check) for volume in selected]
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())
