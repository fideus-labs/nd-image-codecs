# scripts/

Helper scripts, created as their roadmap phases land:

| Script | Phase | Purpose |
| --- | --- | --- |
| `gen-series-fixtures.py` | 1 | Regenerate the `codec_series` cross-language fixture matrix (`fixtures/codec-series/matrix.json`) |
| `ci/check-series-equality.py` | 1 | Assert Rust / Python / TypeScript `codec_series` output is byte-identical across the fixture matrix (the `series-equality` CI job) |
| `fetch-conformance.sh` | 3 | Fetch + cache the OpenJPH conformance corpus (Tier 2 test data) |
| `fetch-bench-data.sh` | 5 | Fetch + pin Tier 3 benchmark volumes (`bench-data.lock.toml`) |
| `range-demo.sh` | 2 | Execute an `ndic index` plan with curl against a static server |
| `profile.sh` | 5 | Flamegraph/perf wrapper for a bench workload |
| `asm.sh` | 5 | Inspect release codegen of hot functions per SIMD lane |
| `ci/check-usage-docs.sh` | 6 | Extract + run every code block in docs/usage/* |

See [docs/development/test-data.md](../docs/development/test-data.md) and the
[roadmap](../docs/development/roadmap/index.md).
