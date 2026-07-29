# scripts/

Helper scripts, created as their roadmap phases land:

| Script | Phase | Purpose |
| --- | --- | --- |
| `fetch-conformance.sh` | 1 | Fetch + cache the OpenJPH conformance corpus (Tier 2 test data) |
| `fetch-bench-data.sh` | 1/5 | Fetch + pin Tier 3 benchmark volumes (`bench-data.lock.toml`) |
| `range-demo.sh` | 2 | Execute an `ndic index` plan with curl against a static server |
| `profile.sh` | 5 | Flamegraph/perf wrapper for a bench workload |
| `asm.sh` | 5 | Inspect release codegen of hot functions per SIMD lane |
| `ci/check-usage-docs.sh` | 6 | Extract + run every code block in docs/usage/* |

See [docs/development/test-data.md](../docs/development/test-data.md) and the
[roadmap](../docs/development/roadmap/index.md).
