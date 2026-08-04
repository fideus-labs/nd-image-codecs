---
title: Development Commands
description: Every command needed to build, check, test, lint, benchmark, and document nd-image-codecs, all run from the repository root.
---

All commands run from the repository root.

## Build & check

| Command | Purpose |
| --- | --- |
| `cargo check --workspace` | Fast type-check of every crate |
| `cargo build --workspace --release` | Optimized build (thin LTO, 1 codegen unit) |
| `cargo build -p ndic-cli --release` | Just the `ndic` binary |
| `cargo build -p ndic-zarr --target wasm32-unknown-unknown` | WASM core check (SIMD128 flags come from `.cargo/config.toml`) |

## Test

| Command | Purpose |
| --- | --- |
| `cargo test --workspace` | All unit, integration, and doc tests |
| `cargo test --workspace --features ndic-zarr/zarrs,ndic-lift/serde` | Plus the feature-gated surfaces (zarrs codec registration, conformance vectors) — what CI runs |
| `cargo test -p ndic-htj2k` | One crate |
| `cargo test --workspace --release` | Slow proptest/round-trip suites at full speed |
| `PROPTEST_CASES=4096 cargo test -p ndic-lift` | Deeper property-test runs |

## Lint & format

| Command | Purpose |
| --- | --- |
| `cargo fmt --all` | Format (rustfmt defaults, no `rustfmt.toml`) |
| `cargo fmt --all --check` | CI-style format check |
| `cargo clippy --workspace --all-targets` | Clippy `all` + `pedantic` (must be warning-clean) |
| `cargo doc --workspace --no-deps` | Build API docs; `missing_docs` is a warn lint |
| `python3 scripts/ci/check-package-versions.py` | Assert the published version agrees across every manifest, lockfile, and `__version__` fallback |

## Benchmarks

| Command | Purpose |
| --- | --- |
| `python3 bench/py/run_nd_delta.py` | The nd-delta lanes via `zarr-python` (needs `zarr>=3`) |
| `python3 bench/py/run_nd_lift.py` | The nd-lift lanes (`transpose → nd_lift → bytes → blosc`) via `zarr-python` |
| `cargo run -p ndic-bench-cli --release -- list` | List registered Rust benchmarks |
| `cargo run -p ndic-bench-cli --release -- run` | Full matrix run, JSON records under `target/benchmarks/` |
| `cargo run -p ndic-bench-cli --release -- run --filter htj2k --config simd-53-ht` | Subset run |
| `cargo run -p ndic-bench-cli --release -- run --baseline main --fail-on-regression` | Run + gate against the committed baseline |
| `cargo run -p ndic-bench-cli --release -- compare main --gate ratio --fail-on-regression` | The PR gate, locally |
| `cargo run -p ndic-bench-cli --release -- compare bench/baselines/main` | Diff latest run against the committed baseline |

See [benchmarking](./benchmarking.md) for record layout, baselines, and the gate.

## Bindings

| Command | Purpose |
| --- | --- |
| `cd bindings/python/nd-image-codecs && maturin develop --release` | Build + install the Python package into the active venv |
| `cd bindings/python/nd-image-codecs && pytest` | Python tests (pure-Python builder + nd-delta round-trip; needs `pytest zarr numpy`) |
| `cd bindings/typescript && npm run build:wasm && npm run build` | WASM + TypeScript build |
| `cd bindings/typescript && npm test` | TS tests (vitest, incl. the fixture matrix) |
| `python3 scripts/ci/check-series-equality.py` | Cross-language `codec_series` equality over the fixture matrix |
| `python3 scripts/gen-series-fixtures.py` | Regenerate `fixtures/codec-series/matrix.json` (only on deliberate builder changes) |

## CLI smoke

| Command | Purpose |
| --- | --- |
| `cargo run -p ndic-cli -- inspect fixtures/tiny.jph` | Print codestream structure |
| `cargo run -p ndic-cli -- index fixtures/tiny.jph --target thumbnail` | Print the byte-range plan |

## Documentation site

The [mystmd](https://mystmd.org) site is a self-contained npm package under
`docs/`, pinned by `docs/package-lock.json`. The `book-theme` that renders it is
pinned separately, as a commit archive URL in `site.template` — mystmd fetches
templates outside npm, so the lockfile does not cover it
([ADR 001](./decisions/adr-001-documentation-toolchain.md) Decision 7). Run
these from `docs/`.

| Command | Purpose |
| --- | --- |
| `cd docs && npm ci` | First-time setup: install the pinned mystmd toolchain from the lockfile |
| `cd docs && npm start` | Live-reloading preview on <http://localhost:3000> |
| `cd docs && npm run build` | Static HTML site into `docs/_build/html/` (gitignored) |
| `cd docs && npm run check` | Strict build — fails on any warning |
| `cd docs && npm run clean` | Remove `docs/_build/` |
| `python3 scripts/ci/check-docs-links.py` | Check every outbound `http(s)` link in `docs/` (scheduled monthly — **never** a pull request gate) |

**Run `npm run check` before pushing any documentation change.** It is the same
build as `npm run build` with `--strict`, so an unresolved cross-reference, a
duplicate identifier, a missing image, or a malformed directive fails loudly
instead of shipping.

Every page under `docs/` must be listed in the `toc` in `docs/myst.yml` or it
will not appear on the site; the toc is explicit so the curated reading order of
each section is preserved. Every page also carries YAML frontmatter with a
`title` and `description`, and links between pages carry explicit link text
(`[Overview](./overview.md)`). MyST would auto-fill an empty label from the
target's title, but GitHub renders `[](./overview.md)` as an empty anchor — and
these files are read directly in the repository as well as on the site.

Code fences need a language the site's highlighter knows **by exact name** — it
does not resolve aliases, and an unknown name degrades silently to unhighlighted
plain text rather than failing the strict build:

| Content | Use | Not |
| --- | --- | --- |
| Shell commands | `bash` | `sh`, `zsh` |
| A `$`-prompt session with output | `shell` | `console`, `shell-session` |
| Rust | `rust` | `rust,ignore` |
| Everything else | `python`, `typescript`, `json`, `toml`, `yaml` | |
| ASCII diagrams, literal strings, sample output that is not code | `text` | a bare ``` fence |

### What CI enforces

The `docs` job in [`.github/workflows/ci.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/ci.yml)
runs `npm run check` on **every pull request**, alongside `cargo doc`. There is
no CI-only flag to discover — it is the same script, run the same way, so a
green check locally means a green job. Because `--strict` promotes every mystmd
warning to an error, the job fails on a broken relative link between pages, an
unresolved cross-reference, a page missing from the `toc`, a missing image, or a
malformed directive. The MyST steps run before the Rust toolchain install, so a
broken link is reported in a fraction of the time a full rustdoc build takes.

The same job uploads the rendered site as a workflow artifact named `docs-site`,
retained 7 days. Download it from the **Summary** page of the pull request's CI
run to read a documentation change exactly as it will appear, without waiting on
the Read the Docs preview.

External links are checked **separately, and never gate a pull request**. The
`Docs Link Check` workflow
([`.github/workflows/docs-link-check.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/docs-link-check.yml))
runs `scripts/ci/check-docs-links.py` monthly, plus on demand from
**Actions → Docs Link Check → Run workflow**. Keeping it off the pull request
trigger is deliberate: the roughly 90 specification and vendor URLs cited here
(ISO, ITU, LLNL, frontiersin, kakadusoftware) rate-limit, bot-block, and go
offline independently of this repository, and a check that is routinely red for
someone else's outage is one contributors learn to ignore. It exits non-zero
only for a definitively dead target; blocked and unreachable hosts are reported
as warnings. Use `--timeout` and `--jobs` to adjust patience and concurrency
when running it by hand.

For the deployment side — how the same `npm run check` script builds the
published site, and what a maintainer must set up by hand — see
[Read the Docs deployment](./read-the-docs.md).

## Release

Publishing happens in CI, on a `vX.Y.Z` tag push. These are the commands around
that — everything a maintainer runs before the tag, and the pieces of the
workflow that also run standalone.

| Command | Purpose |
| --- | --- |
| `scripts/release/prepare-release.sh 0.2.0` | The pre-tag commit: version everywhere, changelog entry, and the tagging steps to run next |
| `python3 scripts/release/set-version.py 0.2.0` | Write one version into all 23 locations, then verify it |
| `python3 scripts/release/parse-tag.py v0.2.0` | What the release workflow derives from a tag (version, prerelease flag, npm dist-tag) |
| `python3 scripts/release/publish-crates.py 0.2.0 --dry-run` | Package and verify all seven crates without uploading — the workflow's `verify` job |
| `uvx --from commitizen cz changelog --unreleased-version=v0.2.0 --dry-run` | Preview the changelog section for the next release |
| `uvx --from commitizen cz commit` | Write a Conventional Commit message interactively |
| `uvx --with pytest --from pytest pytest scripts/tests -q` | Test the release scripts (what the `package-versions` CI job runs) |
| `gh run watch --workflow=release.yml` | Follow a release in progress |

Publishing by hand is the fallback for when CI cannot run, and it needs
credentials this project deliberately does not keep. The commands are in
[publishing](./publishing.md), along with the tag-push procedure, where the
version lives, and what to do when a release fails partway.
