---
title: Benchmarking
description: 'nd-image-codecs uses a driver-plus-registry benchmarking model: a driver CLI wraps a registry of workloads, writes per-benchmark JSON records, and diffs runs against committed baselines with a statistical regression gate.'
---

nd-image-codecs uses a driver-plus-registry benchmarking model: a driver CLI wraps a registry
of benchmark workloads, writes per-benchmark JSON records, and diffs runs against
committed baselines with a statistical regression gate. Full design:
[`bench/docs/architecture.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/docs/architecture.md); day-to-day usage:
[`bench/README.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/README.md).

## The pieces

- **`bench/rs/ndic-bench-core`** — the shared layer: `BenchEntry` (an
  `inventory`-registered benchmark descriptor), `BenchConfig` (one point of the
  codec-configuration matrix), `BenchRecord` (the JSON record schema, including
  `bytes_in`/`bytes_out` for ratio tracking), `Baseline` load/save, the
  `diff` comparer, and the regression threshold constants.
- **`bench/rs/ndic-bench-cli`** — the `ndic-bench` driver: `run`, `compare`,
  `list`; `--filter`, `--config`, `--format ascii|json|both|markdown|csv`,
  `--baseline`, `--fail-on-regression`, `--gate time|ratio|both`, `--quiet`.
- **`bench/py/`** — Python-side lanes that exercise codecs through
  `zarr-python` (`run_nd_delta.py` for the nd-delta family;
  `run_nd_lift.py` for the `transpose → nd_lift → bytes → blosc`
  validation series on the correlated z-stack fixture; shared machinery in
  `lanes.py`) plus the deterministic synthetic-fixture generators
  (`synthetic.py`). They emit the same `BenchRecord` JSON into the same
  records tree, so `ndic-bench compare` gates them like any Rust workload.
- **`bench/benchmarks.toml`** — the declarative suite manifest: which benchmarks run in
  which CI contexts, per-config sample counts, fixture sizes.
- **`bench/baselines/<name>/`** — committed baseline records (e.g. `main/`) with a
  `manifest.json` recording machine, toolchain, and git hash.
- **`bench/viewer/`** — a static-site viewer for record JSON (compare runs, plot
  history), served from `target/benchmarks/site/`.

## The configuration matrix

Every workload is swept across the **codec-configuration** matrix:

| Label | Family | SIMD | Wavelet | Lift levels |
| --- | --- | --- | --- | --- |
| `blosc-zstd` | baseline | (blosc) | — | — |
| `nd-delta-zstd` | nd-delta | (blosc) | — | — |
| `nd-delta-lz4` | nd-delta | (blosc) | — | — |
| `nd-lift-delta-zstd` | nd-lift | (blosc) | — | delta (t, z) |
| `nd-lift-53-zstd` | nd-lift | (blosc) | 5/3 | 2 (t, z) |
| `scalar-53-ht` | nd-lift-ht | off | 5/3 | 0 |
| `simd-53-ht` | nd-lift-ht | on | 5/3 | 0 |
| `simd-97-ht` | nd-lift-ht | on | 9/7 | 0 |
| `simd-53-lift-z2` | nd-lift-ht | on | 5/3 | 2 (z) |
| `zfp-rate8` | nd-zfp | on | — | — |
| `zfp-reversible` | nd-zfp | on | — | — |

The `blosc-zstd` lane is the standing comparison bar: a stock
`bytes → blosc(zstd)` pipeline on the same fixtures, so every ratio in the table
is read against what a user gets today without this project.

:::{note} Reference lanes are specified, not yet wired up
`bench/benchmarks.toml` declares three lanes that would shell out to external
implementations — OpenJPH's `ojph_compress`/`ojph_expand`, the C ZFP library via
`zfp-sys`, and Python `imagecodecs` — to ground speed and ratio claims against
the C/C++ state of the art. All three carry `enabled = false`, no driver code
reads them, and no baseline records exist for them, so **no published number
here is a comparison against a C/C++ implementation.**

What *is* verified today is correctness rather than speed, in the test suite
rather than the bench suite: `imagecodecs` interop runs in the `python` CI job
(`test_imagecodecs_interop.py`, `test_nd_zfp_roundtrip.py`), and OpenJPH
round-trip interop runs from `crates/ndic-codestream/tests/openjph_interop.rs`
when a local OpenJPH build is present — it skips otherwise, including in CI.
:::

The nd-lift-ht lanes are told apart by the `lift_ht/*` workloads:
the composed `nd_lift → htj2k` chunk path over a correlated z-stack
(`chunk_encode`/`chunk_decode`, ratio + throughput) and the byte-range plan
economy (`thumbnail_bytes`: bytes a 16-px thumbnail plan fetches, as a
fraction of the chunk). The committed dev-box baselines record the
**z-decorrelation gain**: ratio 0.2933 undecorrelated
(`simd-53-ht`) vs **0.2549** with two z-lifting levels (`simd-53-lift-z2`)
on the correlated fixture — ~13 % smaller chunks — with the blosc-backed
analog on the Python correlated fixture at 0.4007 (`nd-lift-delta-zstd`,
the nd-delta-style differencing) vs 0.3766 (`nd-lift-53-zstd`). Thumbnail
plans fetch 2.1–2.4 % of the chunk in ≤ 3 ranges.

The nd-zfp lanes run the `zfp/*` workloads over a correlated float
volume: `chunk_encode`/`chunk_decode` (ratio + throughput; `zfp-rate8`
pins the fixed-rate 0.25 ratio by construction, `zfp-reversible` the
lossless ratio) and `brick_bytes` — the fixed-rate random-access economy:
the timed region decodes one `4³` brick at its computed offset and
`bytes_out` is that brick's byte span, ~0.05 % of the chunk. The
zarr-python analog (`bench/py/run_nd_zfp.py`) runs `transpose → nd_zfp`
against a stock `bytes → blosc(zstd)` float baseline on the same fixture.

## Records & baselines

Each `(benchmark, config)` run writes one JSON `BenchRecord` to
`target/benchmarks/<git-hash>/<config>/<module>__<name>.json` (gitignored):
median/min/max plus raw per-sample nanoseconds, and — for codec workloads —
`bytes_in`/`bytes_out` so ratio is tracked alongside throughput. Refreshing a
committed baseline is an explicit, reviewed act:

```bash
python3 bench/py/run_nd_delta.py
python3 bench/py/run_nd_lift.py
cargo run -p ndic-bench-cli --release -- run
cp -r target/benchmarks/<hash>/* bench/baselines/main/   # + update manifest.json
```

(CI's `bench-baseline-refresh` workflow automates this on demand. It records
on a GitHub runner, so adopting its output moves the baseline's machine class
off the dev box the current manifest names — which is why it makes you spell
the machine out. Ratios carry over; timings do not.)

## Tiers and the nightly grid

Micro and meso lanes run on the committed synthetic fixtures and gate every
pull request. The **macro** tier runs on the fetched Tier 3 domain volumes —
real OME-Zarr data where the decorrelation gain a generator can only
approximate is the actual measurement:

```bash
scripts/fetch-bench-data.sh          # once; verifies the pinned SHA-256s
python3 bench/py/run_macro.py
```

The macro lanes skip cleanly when nothing is cached, so they never block a
run. `bench-nightly.yml` fetches first and runs the full grid, uploading the
records and opening an issue on a ratio regression (deterministic, so worth
waking someone for) while merely reporting timings (runner-dependent). See
[Test data](./test-data.md) for what is pinned and why.

## Profiling

```bash
scripts/profile.sh --filter zfp/encode                 # perf report
scripts/profile.sh --filter htj2k --flamegraph         # SVG flamegraph
```

Both build the workspace's `profiling` profile — release codegen plus line
tables — so hot loops resolve to source lines rather than addresses, and both
want `kernel.perf_event_paranoid=1`. Allocation audits use the same binary
under `valgrind --tool=dhat` or `heaptrack`; no special build is needed.

## The regression gate

Two regression kinds, selected with `--gate time|ratio|both` (default `both`):

**Time** — a benchmark regresses when, versus the baseline record:

1. its median is ≥ **10 %** slower (`REGRESSION_PCT_THRESHOLD = 0.10`), **and**
2. the increase exceeds the baseline's noise envelope (σ of the baseline samples).

Both conditions must hold — this suppresses false alarms from noisy micro-benchmarks
while still catching real slowdowns. The σ envelope is only meaningful against a
baseline captured on the same machine class.

**Ratio** — the compression ratio (`bytes_out / bytes_in`) worsened ≥ **2 %**
over the baseline (`RATIO_REGRESSION_PCT_THRESHOLD = 0.02`). Normalizing by
`bytes_in` keeps the gate meaningful when a fixture changes size, and
compressed sizes are deterministic, so this gate holds across machine classes.

Which kinds gate a pull request is derived from the baseline itself: the PR
workflow reads `bench/baselines/main/manifest.json` and gates **both** kinds
when the manifest's machine is the workflow's own runner class
(`gha-ubuntu-24.04`), ratio only otherwise. Adopting a CI-runner baseline via
`bench-baseline-refresh` therefore switches the throughput gate on without a
workflow edit.

The report's status column honors the selected gate: a threshold exceeded on
an **ungated** kind renders as `ok (time n/a: ungated)` plus a trailing note,
rather than `TIME-REGRESSED`, so a ratio-only comparison against baselines
from a different machine class cannot be misread as a real slowdown. The JSON
encoding keeps the raw per-kind booleans.

`bench-pr-gate.yml` runs the nd-delta and nd-lift lanes and any registered
Rust workloads on every PR and compares against `bench/baselines/main/` with
`--gate ratio --fail-on-regression` (CI runners differ from the baseline
machine, so only the deterministic gate fails the build), posting a sticky PR
comment with the markdown report. Exit codes: `0` clean, `1` regression (with
the flag), `2` benchmark failure.

## Adding a benchmark

In the workload crate (e.g. `ndic-htj2k`), register an entry returning a
`BenchOutput` (per-sample nanoseconds, plus `bytes_in`/`bytes_out` for codec
workloads; a bare `Vec<u64>` converts via `.into()`):

```rust
inventory::submit! {
    ndic_bench_core::BenchEntry::new(
        "htj2k", "cleanup_encode_64x64",
        |cfg| bench_cleanup_encode(cfg),
    )
}
```

Guidelines:

- Benchmark the smallest meaningful unit (one pass over one code-block; one plane
  transform), plus one end-to-end encode/decode per crate.
- Fixture data comes from `bench`-generated synthetic planes and the shared corpus in
  [test data](./test-data.md) — never generate randomness inside the timed region.
- Any PR touching a hot path (block coder, transforms, packet assembly) must include or
  update a benchmark; reviewers hold the line.
- **A newly registered benchmark is ungated until the next baseline refresh.** No
  committed record matches it, so `compare` renders it `new` with status `ok` and it
  cannot trip either kind — it is reported, not held. That is the right default (a gate
  needs a baseline to be a gate), but it means "registered" and "protected" are not the
  same state, and only a refresh closes the gap. `transform/dwt97_fwd_2048` has been in
  exactly that state since it was added.
