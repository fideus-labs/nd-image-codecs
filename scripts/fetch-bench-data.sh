#!/usr/bin/env bash
# Fetch and cache the Tier 3 benchmark volumes pinned in
# scripts/bench-data.lock.toml, verifying each against its SHA-256.
#
#   scripts/fetch-bench-data.sh              # fetch everything missing
#   scripts/fetch-bench-data.sh --check      # verify the cache, fetch nothing
#   scripts/fetch-bench-data.sh --only SLUG  # one entry
#
# Volumes land in ~/.cache/nd-image-codecs/bench-data/<slug>.npy (override
# with NDIC_BENCH_DATA_DIR). The macro-tier bench lanes load whatever is
# cached and skip cleanly when nothing is, so this is never required to run
# the suite — see bench/py/tier3.py and docs/development/test-data.md.
#
# Needs Python with zarr>=3.1 + numpy (the same environment the bench lanes
# use); network access to the pinned hosts.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec "${PYTHON:-python3}" "$repo_root/bench/py/tier3.py" "$@"
