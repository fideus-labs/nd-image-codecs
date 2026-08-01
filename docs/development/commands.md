## Development Commands

All commands run from the repository root.

### Build & check

| Command | Purpose |
| --- | --- |
| `cargo check --workspace` | Fast type-check of every crate |
| `cargo build --workspace --release` | Optimized build (thin LTO, 1 codegen unit) |
| `cargo build -p ndic-cli --release` | Just the `ndic` binary |
| `cargo build -p ndic-zarr --target wasm32-unknown-unknown` | WASM core check (SIMD128 flags come from `.cargo/config.toml`) |

### Test

| Command | Purpose |
| --- | --- |
| `cargo test --workspace` | All unit, integration, and doc tests |
| `cargo test -p ndic-htj2k` | One crate |
| `cargo test --workspace --release` | Slow proptest/round-trip suites at full speed |
| `PROPTEST_CASES=4096 cargo test -p ndic-lift` | Deeper property-test runs |

### Lint & format

| Command | Purpose |
| --- | --- |
| `cargo fmt --all` | Format (rustfmt defaults, no `rustfmt.toml`) |
| `cargo fmt --all --check` | CI-style format check |
| `cargo clippy --workspace --all-targets` | Clippy `all` + `pedantic` (must be warning-clean) |
| `cargo doc --workspace --no-deps` | Build API docs; `missing_docs` is a warn lint |

### Benchmarks

| Command | Purpose |
| --- | --- |
| `python3 bench/py/run_nd_delta.py` | The Phase 1 nd-delta lanes via `zarr-python` (needs `zarr>=3`) |
| `cargo run -p ndic-bench-cli --release -- list` | List registered Rust benchmarks |
| `cargo run -p ndic-bench-cli --release -- run` | Full matrix run, JSON records under `target/benchmarks/` |
| `cargo run -p ndic-bench-cli --release -- run --filter htj2k --config simd-53-ht` | Subset run |
| `cargo run -p ndic-bench-cli --release -- run --baseline main --fail-on-regression` | Run + gate against the committed baseline |
| `cargo run -p ndic-bench-cli --release -- compare main --gate ratio --fail-on-regression` | The PR gate, locally |
| `cargo run -p ndic-bench-cli --release -- compare bench/baselines/main` | Diff latest run against the committed baseline |

See [benchmarking.md](./benchmarking.md) for record layout, baselines, and the gate.

### Bindings

| Command | Purpose |
| --- | --- |
| `cd bindings/python/nd-image-codecs && maturin develop --release` | Build + install the Python package into the active venv |
| `cd bindings/python/nd-image-codecs && pytest` | Python tests (pure-Python builder + nd-delta round-trip; needs `pytest zarr numpy`) |
| `cd bindings/typescript && npm run build:wasm && npm run build` | WASM + TypeScript build |
| `cd bindings/typescript && npm test` | TS tests (vitest, incl. the fixture matrix) |
| `python3 scripts/ci/check-series-equality.py` | Cross-language `codec_series` equality over the fixture matrix |
| `python3 scripts/gen-series-fixtures.py` | Regenerate `fixtures/codec-series/matrix.json` (only on deliberate builder changes) |

### CLI smoke

| Command | Purpose |
| --- | --- |
| `cargo run -p ndic-cli -- inspect fixtures/tiny.jph` | Print codestream structure |
| `cargo run -p ndic-cli -- index fixtures/tiny.jph --target thumbnail` | Print the byte-range plan |

### Release

| Command | Purpose |
| --- | --- |
| `cargo publish --workspace --dry-run` | Rehearse the crates.io release (skips `publish = false` members) |
| `cargo publish --workspace` | Publish all seven crates in dependency order |
| `maturin sdist -m bindings/python/nd-image-codecs/Cargo.toml -o dist` | Build the PyPI source distribution |
| `maturin build --release -m bindings/python/nd-image-codecs/Cargo.toml -o dist` | Build the abi3 wheel for this platform |
| `twine check dist/*` | Validate the PyPI artifacts before upload |
| `twine upload dist/*` | Upload to PyPI |
| `cd bindings/typescript && npm publish --access public` | Publish `@fideus-labs/nd-image-codecs` |
| `cd bindings/javascript && npm publish` | Publish the unscoped `nd-image-codecs` name placeholder |

See [publishing.md](./publishing.md) for the full manual release procedure,
prerequisites, version-bump locations, and verification steps.
