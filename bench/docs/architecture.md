# Benchmark Layer Architecture

> **Version:** 0.1
> **Status:** Draft

## Design

The layer separates **what to measure** (workloads registered where the code lives)
from **how to measure and judge** (the driver). The matrix axis — the dimension every
workload is swept across — is the **codec configuration**, not a compute backend.

```
workload crates ── inventory::submit!(BenchEntry) ──┐   (link time)
                                                     ▼
ndic-bench-cli (driver)                 registry walk
  run: for entry × config → timings → BenchRecord JSON
  target/benchmarks/<git-hash>/<config>/<module>__<name>.json
  compare: records vs bench/baselines/<name>/ → report + exit code
```

### BenchEntry (registration)

Workload crates register benchmarks next to the code under test:

```rust
inventory::submit! {
    BenchEntry::new("htj2k", "cleanup_encode_64x64", |cfg| run_it(cfg))
}
```

The CLI anchors the workload crates (`mod _anchors`) so their registrations link into
the binary. Adding a benchmark requires **no** central-list edit.

### BenchConfig (the matrix)

Built-in configs: `blosc-zstd` (the plain-compressor reference lane),
`nd-delta-zstd`, `nd-delta-lz4`, `scalar-53-ht`, `simd-53-ht`, `simd-97-ht`, `simd-53-lift-z2`, `zfp-rate8`. Later phases
add reference lanes (`ref-openjph`, `ref-imagecodecs`) that shell out to pinned
external implementations on identical fixtures and emit the same record schema.
`bench/benchmarks.toml` maps configs and workload tiers (micro/meso/macro) to CI
contexts (pr-gate / nightly / on-demand).

### Python-side lanes (`bench/py/`)

The Phase 1 nd-delta family is composed entirely of existing Zarr codecs, so
its lanes (`run_nd_delta.py`) exercise the real consumer path — pipelines
authored by `nd_image_codecs.codec_series`, executed by `zarr-python` — on the
deterministic synthetic microscopy fixture (`synthetic.py`). They emit the
same `BenchRecord` schema into the same records tree, so `ndic-bench compare`
diffs and gates them exactly like Rust workloads.

### BenchRecord (the schema)

One JSON file per `(benchmark, config)`: name, config label, git hash, sample count,
median/min/max ns, and raw per-sample ns; codec workloads add
`bytes_in`/`bytes_out` so compression ratio is tracked alongside throughput.
Raw samples are kept so the comparer can
compute noise envelopes without re-running, and so the viewer can plot distributions.
Phase 6 extends records with `psnr` for rate–distortion gating.

### Baselines

`bench/baselines/<name>/` mirrors a records tree plus `manifest.json` (machine class,
toolchain, git hash, date). Baselines change only through the reviewed
`bench-baseline-refresh` workflow so gate drift is always an explicit decision.

### The gate

For each record pair (current, baseline), two regression kinds
(`--gate time|ratio|both`, default `both`):

```
time_regressed  = median_cur ≥ median_base × (1 + 0.10)
               && (median_cur − median_base) > σ(baseline raw samples)
ratio_regressed = bytes_out_cur ≥ bytes_out_base × (1 + 0.02)
```

Both time conditions — the percentage catches real slowdowns, the σ envelope suppresses
noisy micro-benchmarks. The ratio gate is deterministic, so it holds across
machine classes; the PR gate runs `--gate ratio` because CI runners differ from
the baseline capture machine. `--fail-on-regression` turns any regressed pair
into exit code 1; the PR gate posts the markdown report as a sticky comment.

### Reports

`--format ascii|json|both|markdown|csv`; `--quiet`/`-v`. Markdown is used by CI
comments; csv feeds ad-hoc analysis; the viewer (static site generated into
`target/benchmarks/site/`) renders record histories and config/ref-lane overlays.

## Workflows (`.github/workflows/`)

| Workflow | Trigger | What it does |
| --- | --- | --- |
| `bench-pr-gate.yml` | PR | Curated subset vs `baselines/main`, `--fail-on-regression`, sticky comment |
| `bench-nightly.yml` (Phase 6) | cron | Full grid incl. ref lanes; publishes viewer site; auto-files regressions |
| `bench-baseline-refresh.yml` (Phase 6) | manual | Re-records baselines on the pinned runner class; opens a PR |

## Invariants

- Timed regions never allocate fixtures or RNG state; setup happens outside.
- Records are append-only artifacts; nothing under `target/` is committed.
- A benchmark rename is a breaking change to baselines — do it with a baseline refresh
  in the same PR.
