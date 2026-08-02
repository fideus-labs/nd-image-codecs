#!/usr/bin/env bash
# Fetch (and cache) the OpenJPH conformance corpus used by
# crates/ndic-codestream/tests/corpus_conformance.rs, and optionally build
# the OpenJPH reference tools the interop tests shell out to.
#
# Usage: scripts/fetch-conformance.sh [--with-tools]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLS="$ROOT/target/tools"
mkdir -p "$TOOLS"

if [ ! -d "$TOOLS/jp2k_test_codestreams" ]; then
  git clone --depth 1 https://github.com/aous72/jp2k_test_codestreams.git \
    "$TOOLS/jp2k_test_codestreams"
else
  echo "corpus already present: $TOOLS/jp2k_test_codestreams"
fi

if [ "${1:-}" = "--with-tools" ]; then
  OJPH="$TOOLS/openjph"
  if [ ! -d "$OJPH" ]; then
    git clone --depth 1 https://github.com/aous72/OpenJPH.git "$OJPH"
  fi
  if [ ! -f "$OJPH/build/src/apps/ojph_expand/ojph_expand" ]; then
    # Prefer Ninja when available; fall back to the default generator.
    GEN=""
    command -v ninja >/dev/null 2>&1 && GEN="-G Ninja"
    cmake -S "$OJPH" -B "$OJPH/build" $GEN -DCMAKE_BUILD_TYPE=Release \
      -DOJPH_ENABLE_TIFF_SUPPORT=OFF
    cmake --build "$OJPH/build"
  fi
  echo "ojph tools: $OJPH/build/src/apps/"
fi

echo "run: cargo test -p ndic-codestream --test corpus_conformance -- --nocapture"
