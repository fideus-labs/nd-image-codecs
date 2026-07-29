## Benchmarking

nd-image-codecs uses a driver-plus-registry benchmarking model: a driver CLI wraps a registry
of benchmark workloads, writes per-benchmark JSON records, and diffs runs against
committed baselines with a statistical regression gate. Full design:
[`bench/docs/architecture.md`](../../bench/docs/architecture.md); day-to-day usage:
[`bench/README.md`](../../bench/README.md).

### The pieces

- **`bench/rs/ndic-bench-core`** — the shared layer: `BenchEntry` (an
  `inventory`-registered benchmark descriptor), `BenchConfig` (one point of the
  codec-configuration matrix), `BenchRecord` (the JSON record schema), and the
  regression threshold constant.
- **`bench/rs/ndic-bench-cli`** — the `ndic-bench` driver: `run`, `compare`,
  `list`; `--filter`, `--config`, `--format ascii|json|both|markdown|csv`,
  `--baseline`, `--fail-on-regression`, `--quiet`.
- **`bench/benchmarks.toml`** — the declarative suite manifest: which benchmarks run in
  which CI contexts, per-config sample counts, fixture sizes.
- **`bench/baselines/<name>/`** — committed baseline records (e.g. `main/`) with a
  `manifest.json` recording machine, toolchain, and git hash.
- **`bench/viewer/`** — a static-site viewer for record JSON (compare runs, plot
  history), served from `target/benchmarks/site/`.

### The configuration matrix

Every workload is swept across the **codec-configuration** matrix:

| Label | Family | SIMD | Wavelet | Lift levels |
| --- | --- | --- | --- | --- |
| `nd-delta-zstd` | nd-delta | (blosc) | — | — |
| `scalar-53-ht` | nd-lift-ht | off | 5/3 | 0 |
| `simd-53-ht` | nd-lift-ht | on | 5/3 | 0 |
| `simd-97-ht` | nd-lift-ht | on | 9/7 | 0 |
| `simd-53-lift-z2` | nd-lift-ht | on | 5/3 | 2 (z) |
| `zfp-rate8` | nd-zfp | on | — | — |

plus **reference lanes**: OpenJPH's `ojph_compress`/`ojph_expand` binaries, the
reference C ZFP library (`ref-zfp`, via `zfp-sys`), and Python `imagecodecs` on
identical fixtures, so speed/ratio claims are grounded against the C/C++ state
of the art.

### Records & baselines

Each `(benchmark, config)` run writes one JSON `BenchRecord` to
`target/benchmarks/<git-hash>/<config>/<module>__<name>.json` (gitignored):
median/min/max plus raw per-sample nanoseconds. Refreshing a committed baseline is an
explicit, reviewed act:

```sh
cargo run -p ndic-bench-cli --release -- run
cp -r target/benchmarks/<hash>/* bench/baselines/main/   # + update manifest.json
```

(CI's `bench-baseline-refresh` workflow automates this on demand.)

### The regression gate

A benchmark **regresses** when, versus the baseline record:

1. its median is ≥ **10 %** slower (`REGRESSION_PCT_THRESHOLD = 0.10`), **and**
2. the increase exceeds the baseline's noise envelope (σ of the baseline samples).

Both conditions must hold — this suppresses false alarms from noisy micro-benchmarks
while still catching real slowdowns. `bench-pr-gate.yml` runs the suite on every PR
against `bench/baselines/main/` with `--fail-on-regression` and posts a sticky PR
comment with the markdown report. Exit codes: `0` clean, `1` regression (with the
flag), `2` benchmark failure.

### Adding a benchmark

In the workload crate (e.g. `ndic-htj2k`), register an entry:

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
  [test-data.md](./test-data.md) — never generate randomness inside the timed region.
- Any PR touching a hot path (block coder, transforms, packet assembly) must include or
  update a benchmark; reviewers hold the line.
