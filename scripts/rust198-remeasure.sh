#!/usr/bin/env bash
# Re-measure the Rust 1.98 migration baseline: the full bench suite plus the
# exactness suites, into one timestamped folder.
#
#   scripts/rust198-remeasure.sh                      # bench + exactness
#   scripts/rust198-remeasure.sh --label phase03-after
#   scripts/rust198-remeasure.sh --bench-only
#   scripts/rust198-remeasure.sh --tests-only --out /tmp/check
#
# From Phase 03 on, every phase that touches code or claims a speed has to
# compare like with like -- whether or not it moves a float operation. This
# script is the "like": it pins the profile, the feature set, the workload
# filter, and the output layout, so two runs differ only by the code between
# them.
#
# Output lands in target/rust198-measurements/<timestamp>[-<label>]/ (override
# the parent with --out):
#
#   summary.md                markdown bench table + provenance
#   compare-vs-main.txt       diff against bench/baselines/main (ratio is the
#                             deterministic metric; see the warning it prints)
#   records/<git-hash>/       the run's BenchRecord tree, copied out of the
#                             gitignored target/benchmarks/ so it survives
#                             `cargo clean`
#   test-output.txt           full `cargo test --workspace --release` output
#   exactness.txt             the five exactness suites run individually
#   manifest.txt              git hash, toolchain, machine, timings
#
# Two things this script exists to stop you getting wrong:
#
#   * `cargo test --workspace --release` DOES NOT LINK in this repo, on 1.98
#     and equally on 1.91. [profile.release] sets panic = "abort", so cargo
#     builds the dependency graph twice, and ndic-zarr's cdylib artifact
#     (libndic_zarr.so, no metadata hash) collides between the two builds --
#     cargo warns "output filename collision", cargo#6313, and the `ndic`
#     binary then fails to link with undefined symbols. This script passes
#     --config profile.release.panic="unwind", which collapses the two graphs
#     into one. Panic strategy cannot move a golden value.
#
#   * Six test files are #![cfg(feature = ...)] and report `ok. 0 passed`
#     without it -- green, and testing nothing. ndic-lift/tests/vectors.rs and
#     ndic-zfp/tests/checksums.rs need --features serde; ndic-zarr's four
#     {delta,htj2k,lift,zfp}_zarrs.rs need --features zarrs (34 of that
#     crate's 53 tests, measured in Phase 07). This script passes both
#     features and records the per-suite test counts so a silent zero is
#     visible in the output. Only the per-crate form is at risk: under
#     --workspace with FEATURES set, cargo's feature unification covers them.
#
# Timings are only comparable against a run from this same machine. The
# committed bench/baselines/main/ was captured on a different machine class
# (and a different ISA), so against it only the bytes_out/bytes_in ratio
# means anything -- which is exactly the invariant Phases 03-04 must hold.
#
# See docs/development/rust-198/adoption-notes.md ("How to re-measure") and
# docs/development/rust-198/float-drift-inventory.md.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Collapses the panic=abort/unwind double build; see the header.
readonly PANIC_CFG='profile.release.panic="unwind"'
# What CI runs (.github/workflows/ci.yml). A no-op under --workspace thanks to
# feature unification, decisive for the per-crate runs below.
readonly FEATURES='ndic-zarr/zarrs,ndic-lift/serde'

label=""
out_parent="$repo_root/target/rust198-measurements"
do_bench=true
do_tests=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --label) label="${2:?--label needs a value}"; shift 2 ;;
    --out) out_parent="${2:?--out needs a value}"; shift 2 ;;
    --bench-only) do_tests=false; shift ;;
    --tests-only) do_bench=false; shift ;;
    # Print the header comment block: everything after the shebang up to the
    # first non-comment line. Derived rather than a line range, so the header
    # can grow without silently truncating this output — it already had, and
    # the two doc pointers at the bottom of it were the part that vanished.
    -h|--help) awk 'NR > 1 { if (!/^#/) exit; sub(/^# ?/, ""); print }' \
      "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="$out_parent/$stamp${label:+-$label}"
mkdir -p "$out"

git_hash="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
git_short="$(git rev-parse --short HEAD 2>/dev/null || echo unknown)"
branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unknown)"
dirty=""
git diff --quiet 2>/dev/null || dirty=" (dirty working tree)"

{
  echo "nd-image-codecs — Rust 1.98 re-measurement"
  echo "captured : $stamp"
  echo "label    : ${label:-none}"
  echo "git      : $git_hash ($git_short) on $branch$dirty"
  echo "toolchain: $(rustc -V) / $(cargo -V)"
  echo "machine  : $(uname -sm), $(nproc) threads, $(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || echo unknown)"
  echo "profile  : release + $PANIC_CFG"
  echo "features : $FEATURES"
} > "$out/manifest.txt"

echo "==> $out"
sed 's/^/    /' "$out/manifest.txt"

if $do_bench; then
  echo "==> bench suite"
  bench_start=$SECONDS
  # Same --config as the test half: manifest.txt records one profile for the
  # whole run, and a bench measured under panic=abort is not that profile.
  # It also keeps both halves on one build graph instead of two.
  cargo run -p ndic-bench-cli --release --config "$PANIC_CFG" \
    -- run --format markdown > "$out/bench-table.md"

  {
    echo "# Bench run — $stamp${label:+ ($label)}"
    echo
    echo '```text'
    cat "$out/manifest.txt"
    echo '```'
    echo
    echo '> Timings compare only against another run from this same machine.'
    echo '> Against `bench/baselines/main/` (different machine class and ISA)'
    echo '> only the `bytes_out / bytes_in` ratio is meaningful.'
    echo
    cat "$out/bench-table.md"
  } > "$out/summary.md"

  # Records land in target/benchmarks/<git-hash>/; copy them somewhere
  # `cargo clean` will not reach.
  mkdir -p "$out/records"
  if [[ -d "target/benchmarks/$git_short" ]]; then
    cp -r "target/benchmarks/$git_short" "$out/records/"
  else
    # `unknown` hash outside a checkout, or a hash the CLI shortened
    # differently: take the newest tree rather than guessing.
    newest="$(find target/benchmarks -mindepth 1 -maxdepth 1 -type d -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)"
    [[ -n "$newest" ]] && cp -r "$newest" "$out/records/"
  fi
  echo "    $(find "$out/records" -name '*.json' 2>/dev/null | wc -l) records in $((SECONDS - bench_start))s"

  echo "==> compare vs baselines/main (measurement, not a gate)"
  # A regression must not abort the run: this is a measurement, and `compare`
  # exits non-zero only with --fail-on-regression, which is deliberately absent.
  cargo run -p ndic-bench-cli --release -- compare main > "$out/compare-vs-main.txt" 2>&1 || true
  cargo run -p ndic-bench-cli --release -- compare main --format json > "$out/compare-vs-main.json" 2>&1 || true
  if grep -q 'RATIO-REGRESSED' "$out/compare-vs-main.txt"; then
    echo "    !! RATIO REGRESSED — a codec's output changed. This is the invariant"
    echo "       Phases 03-04 must not break; see compare-vs-main.txt."
  else
    echo "    ratio: clean (no codec output changed)"
  fi
fi

if $do_tests; then
  echo "==> workspace tests (release)"
  test_start=$SECONDS
  test_status=0
  cargo test --workspace --release --config "$PANIC_CFG" --features "$FEATURES" \
    > "$out/test-output.txt" 2>&1 || test_status=$?
  passed="$(grep -E '^test result' "$out/test-output.txt" | awk '{p+=$4} END {print p+0}')"
  failed="$(grep -E '^test result' "$out/test-output.txt" | awk '{f+=$6} END {print f+0}')"
  echo "    $passed passed, $failed failed (exit $test_status) in $((SECONDS - test_start))s"

  echo "==> exactness suites (individually, with --nocapture)"
  : > "$out/exactness.txt"
  # `--features serde` is not optional for the first two: without it they
  # compile to zero tests and report `ok`.
  for spec in \
    "ndic-lift:vectors:serde" \
    "ndic-zfp:checksums:serde" \
    "ndic-htj2k:openjph_differential:" \
    "ndic-codestream:openjph_interop:" \
    "ndic-codestream:corpus_conformance:"
  do
    pkg="${spec%%:*}"; rest="${spec#*:}"; suite="${rest%%:*}"; feat="${rest#*:}"
    feat_args=()
    [[ -n "$feat" ]] && feat_args=(--features "$feat")
    {
      echo "######## $pkg --test $suite ${feat_args[*]} ########"
    } >> "$out/exactness.txt"
    suite_status=0
    cargo test -p "$pkg" --release --config "$PANIC_CFG" "${feat_args[@]}" \
      --test "$suite" -- --nocapture >> "$out/exactness.txt" 2>&1 || suite_status=$?
    echo "EXIT=$suite_status" >> "$out/exactness.txt"
    echo >> "$out/exactness.txt"

    n="$(grep -E '^test result' "$out/exactness.txt" | tail -1 | awk '{print $4+0}')"
    if [[ "$suite_status" -ne 0 ]]; then
      echo "    FAIL  $pkg/$suite"
      test_status=1
    elif [[ "${n:-0}" -eq 0 ]]; then
      # Green and empty is the failure mode this line exists to surface, so it
      # has to fail the run too — a measurement that tested nothing is not a
      # measurement, and cargo exited 0 on it.
      echo "    ZERO  $pkg/$suite ran 0 tests — check the feature gate"
      test_status=1
    else
      echo "    ok    $pkg/$suite ($n tests)"
    fi
  done

  [[ "$test_status" -eq 0 ]] || { echo "==> tests FAILED; see $out/test-output.txt" >&2; exit 1; }
fi

echo "==> done: $out"
