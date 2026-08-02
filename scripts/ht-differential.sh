#!/usr/bin/env bash
# Build the OpenJPH block-coder oracle and generate differential vectors,
# then run the ndic-htj2k differential test against them.
#
# Requires: git, cmake+ninja (or make), a C++14 compiler, network on first run.
# Usage: scripts/ht-differential.sh [num_vectors]
set -euo pipefail

N="${1:-1000}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TOOLS="$ROOT/target/tools"
OJPH="$TOOLS/openjph"

mkdir -p "$TOOLS"
if [ ! -d "$OJPH" ]; then
  git clone --depth 1 https://github.com/aous72/OpenJPH.git "$OJPH"
fi
if [ ! -f "$OJPH/build/src/core/libopenjph.so" ]; then
  # Prefer Ninja for fresh build trees; an existing tree keeps whatever
  # generator configured it (a -G override there would make cmake fail).
  GEN=()
  if [ ! -f "$OJPH/build/CMakeCache.txt" ] && command -v ninja >/dev/null 2>&1; then
    GEN=(-G Ninja)
  fi
  cmake -S "$OJPH" -B "$OJPH/build" "${GEN[@]}" -DCMAKE_BUILD_TYPE=Release \
    -DOJPH_ENABLE_TIFF_SUPPORT=OFF
  cmake --build "$OJPH/build"
fi

g++ -O2 -std=c++14 \
  -I "$OJPH/src/core/openjph" -I "$OJPH/src/core/coding" -I "$OJPH/src/core/common" \
  "$ROOT/scripts/ht_oracle.cpp" -o "$TOOLS/ht_oracle" \
  -L "$OJPH/build/src/core" -lopenjph -Wl,-rpath,"$OJPH/build/src/core"

"$TOOLS/ht_oracle" "$N" "$TOOLS/ht_vectors.bin"

cd "$ROOT"
cargo test -p ndic-htj2k --test openjph_differential -- --nocapture
