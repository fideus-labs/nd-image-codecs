# scripts/

Helper scripts, created as their roadmap phases land:

| Script | Phase | Purpose |
| --- | --- | --- |
| `gen-series-fixtures.py` | 1 | Regenerate the `codec_series` cross-language fixture matrix (`fixtures/codec-series/matrix.json`) |
| `ci/check-series-equality.py` | 1 | Assert Rust / Python / TypeScript `codec_series` output is byte-identical across the fixture matrix (the `series-equality` CI job) |
| `ci/check-cross-validation.py` | 6 | The cross-ecosystem validation matrix: every `fixtures/cross-validation` case written by zarrs / zarr-python / zarrita.js and read back by all three (the `cross-validation` CI job) |
| `cross-validation/zarr_python_io.py` | 6 | zarr-python writer/reader corner of the matrix (the zarrita.js corner is `bindings/typescript/scripts/zarrita-io.mts`; the zarrs corner is `ndic zarr`) |
| `ci/check-docs-links.py` | 1 | Check the outbound `http(s)` links cited by `docs/` — manual/pre-release, deliberately not a CI gate (external specification hosts are too flaky) |
| `fetch-conformance.sh` | 3 | Fetch + cache the OpenJPH conformance corpus (Tier 2 test data) |
| `fetch-bench-data.sh` | 6 | Fetch, verify, and cache the Tier 3 benchmark volumes pinned in `bench-data.lock.toml` (implementation: `bench/py/tier3.py`) |
| `range-demo.sh` | 2 | Execute an `ndic index` plan with curl against a static server |
| `profile.sh` | 6 | `perf record`/flamegraph wrapper for one bench workload (workspace `profiling` profile) |
| `asm.sh` | 5 | Inspect release codegen of hot functions per SIMD lane |
| `ci/check-usage-docs.py` | 6 | Extract and execute every code block in `docs/usage/*` against the current API (the `usage-docs` CI job) |

See [docs/development/test-data.md](../docs/development/test-data.md) and the
[roadmap](../docs/development/roadmap/index.md).
