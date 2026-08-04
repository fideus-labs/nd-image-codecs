#!/usr/bin/env bash
# Profile one benchmark workload: perf record + report, or a flamegraph.
#
#   scripts/profile.sh --filter zfp/encode                 # perf report
#   scripts/profile.sh --filter htj2k --flamegraph         # SVG flamegraph
#   scripts/profile.sh --filter lift --config simd-53-ht -- --samples 5
#
# --filter selects benchmarks by `<module>/<name>` substring, exactly as
# `ndic-bench run --filter` does; everything after `--` is passed through to
# the bench CLI. Output lands in target/profiles/.
#
# Needs `perf` (linux-tools) for the report mode and `cargo flamegraph`
# (cargo install flamegraph) for the SVG. Both want unprivileged perf events:
#
#   sudo sysctl -w kernel.perf_event_paranoid=1
#
# Builds with the workspace's `profiling` profile — release codegen plus
# line tables, so hot loops resolve to source lines instead of addresses.
# Allocation audits are a separate tool: run the same binary under
# `valgrind --tool=dhat` or `heaptrack`, which needs no further setup.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

filter=""
config=()
flamegraph=false
passthrough=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --filter) filter="${2:?--filter needs a value}"; shift 2 ;;
    --config) config+=(--config "${2:?--config needs a value}"); shift 2 ;;
    --flamegraph) flamegraph=true; shift ;;
    --) shift; passthrough=("$@"); break ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$filter" ]]; then
  echo "--filter is required; list the benchmarks with:" >&2
  echo "  cargo run -p ndic-bench-cli --release -- list" >&2
  exit 2
fi

out_dir="target/profiles"
mkdir -p "$out_dir"
slug="$(printf '%s' "$filter" | tr -c 'A-Za-z0-9._-' '-')"

if [[ "$flamegraph" == true ]]; then
  if ! cargo flamegraph --version >/dev/null 2>&1; then
    echo "cargo flamegraph is not installed: cargo install flamegraph" >&2
    exit 1
  fi
  svg="$out_dir/$slug.svg"
  echo "flamegraph → $svg"
  cargo flamegraph --output "$svg" --profile profiling \
    -p ndic-bench-cli --bin ndic-bench -- \
    run --filter "$filter" "${config[@]+"${config[@]}"}" --quiet \
    "${passthrough[@]+"${passthrough[@]}"}"
  exit 0
fi

if ! command -v perf >/dev/null 2>&1; then
  echo "perf is not installed (apt install linux-tools-common linux-tools-generic)" >&2
  exit 1
fi

# Build first so the compile does not land in the profile.
cargo build -p ndic-bench-cli --profile profiling
data="$out_dir/$slug.perf.data"
echo "perf record → $data"
perf record -g --call-graph dwarf -o "$data" -- \
  target/profiling/ndic-bench run --filter "$filter" \
  "${config[@]+"${config[@]}"}" --quiet "${passthrough[@]+"${passthrough[@]}"}"
perf report -i "$data" --stdio --percent-limit 0.5 | tee "$out_dir/$slug.txt"
echo
echo "full report: perf report -i $data"
