# nd-image-codecs benchmarks

The benchmark layer for nd-image-codecs: a driver CLI over an `inventory`-registered set of
workloads, JSON records per run, committed baselines, and a statistical regression gate
in CI.

## Layout

```
bench/
├── rs/
│   ├── ndic-bench-core/   # BenchEntry registry, BenchConfig, BenchRecord, baseline IO, gate
│   └── ndic-bench-cli/    # the `ndic-bench` driver (run / compare / list)
├── py/                        # Python-side lanes (nd-delta via zarr-python) + synthetic fixtures
├── baselines/                 # committed baseline records, e.g. baselines/main/
├── benchmarks.toml            # suite manifest: workloads × configs × CI contexts
├── viewer/                    # static-site record viewer (target/benchmarks/site/)
└── docs/architecture.md       # full design
```

## Quick start

```sh
python3 bench/py/run_nd_delta.py                       # the Phase 1 nd-delta lanes (needs zarr>=3)
python3 bench/py/run_nd_lift.py                        # the Phase 2 nd-lift lanes (needs zarr>=3)
cargo run -p ndic-bench-cli --release -- list
cargo run -p ndic-bench-cli --release -- run
cargo run -p ndic-bench-cli --release -- run --filter htj2k --config simd-53-ht --format markdown
cargo run -p ndic-bench-cli --release -- compare main --fail-on-regression   # gate the latest run
```

Records land in `target/benchmarks/<git-hash>/<config>/<module>__<name>.json`
(gitignored). The Python lanes write the **same record schema** into the same
tree, so `ndic-bench compare` diffs and gates them identically. Baselines are
refreshed only via the reviewed `bench-baseline-refresh` workflow (or the
equivalent manual copy + PR).

## The gate

Two regression kinds, selected with `--gate time|ratio|both` (default `both`):

- **time** — median ≥ 10 % over baseline **and** beyond the baseline's σ noise
  envelope (`REGRESSION_PCT_THRESHOLD` in `ndic-bench-core`). Only meaningful
  against a baseline captured on the same machine class.
- **ratio** — compression ratio (`bytes_out / bytes_in`) ≥ 2 % worse than
  baseline (`RATIO_REGRESSION_PCT_THRESHOLD`); normalizing by `bytes_in`
  keeps the gate meaningful across fixture-size changes. Deterministic, so
  the PR gate uses `--gate ratio` even though CI runners differ from the
  baseline machine.

Exit codes: `0` clean, `1` regression with `--fail-on-regression`, `2` benchmark
failure.

Full details: [docs/architecture.md](./docs/architecture.md) and
[docs/development/benchmarking.md](../docs/development/benchmarking.md).
