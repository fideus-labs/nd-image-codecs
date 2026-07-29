# nd-image-codecs benchmarks

The benchmark layer for nd-image-codecs: a driver CLI over an `inventory`-registered set of
workloads, JSON records per run, committed baselines, and a statistical regression gate
in CI.

## Layout

```
bench/
├── rs/
│   ├── ndic-bench-core/   # BenchEntry registry, BenchConfig, BenchRecord, gate constant
│   └── ndic-bench-cli/    # the `ndic-bench` driver (run / compare / list)
├── baselines/                 # committed baseline records, e.g. baselines/main/
├── benchmarks.toml            # suite manifest: workloads × configs × CI contexts
├── viewer/                    # static-site record viewer (target/benchmarks/site/)
└── docs/architecture.md       # full design
```

## Quick start

```sh
cargo run -p ndic-bench-cli --release -- list
cargo run -p ndic-bench-cli --release -- run
cargo run -p ndic-bench-cli --release -- run --filter htj2k --config simd-53-ht --format markdown
cargo run -p ndic-bench-cli --release -- run --baseline main --fail-on-regression   # the PR gate
```

Records land in `target/benchmarks/<git-hash>/<config>/<module>__<name>.json`
(gitignored). Baselines are refreshed only via the reviewed
`bench-baseline-refresh` workflow (or the equivalent manual copy + PR).

## The gate

Regression = median ≥ 10 % over baseline **and** beyond the baseline's σ noise envelope
(`REGRESSION_PCT_THRESHOLD` in `ndic-bench-core`). Exit codes: `0` clean,
`1` regression with `--fail-on-regression`, `2` benchmark failure.

Full details: [docs/architecture.md](./docs/architecture.md) and
[docs/development/benchmarking.md](../docs/development/benchmarking.md).
