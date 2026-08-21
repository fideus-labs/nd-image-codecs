<!-- Hand-written ahead of the generator. CHANGELOG.md is produced by
     `uvx --from commitizen cz changelog --incremental` (see .cz.toml), which
     inserts the next `## vX.Y.Z` section below this point from the Conventional
     Commits themselves. Delete this Unreleased block when that runs at release
     time — the commit it describes is already shaped to regenerate it. -->

## Unreleased

### 💥 Breaking Changes

- **build**: raise the minimum supported Rust version from 1.91 to 1.98

  The workspace targets the Rust 1.98 standard library, so the MSRV is set by
  this project rather than tracked from the `zarrs` dependency. This is
  consumer-visible across all three distributions:

  - **crates.io** — depending on `ndic-core`, `ndic-htj2k`, `ndic-codestream`,
    `ndic-lift`, `ndic-zfp`, `ndic-zarr`, or `ndic-cli` now requires a 1.98 or
    newer toolchain; `cargo` will refuse the version rather than fail to build.
  - **PyPI** and **npm** — the published wheels and the prebuilt WASM bundle are
    unaffected, since they ship compiled. Building either binding from source
    requires 1.98.

  `rust-toolchain.toml` pins 1.98.0, so contributors are moved automatically.
  See [Rust 1.98 Adoption Notes](docs/development/rust-198/adoption-notes.md).

### ♻️ Refactoring

- **htj2k**: make the DWT row splitter safe and deny `unsafe_code` workspace-wide

  `ndic-htj2k` is the only published crate with an `unsafe` surface, and it is
  now smaller. `dwt::simd::split_three` built three row slices out of one raw
  pointer, with disjointness argued in a comment; it uses `split_at_mut` on the
  destination row instead, and the block is gone. Output is bit-identical — the
  differential test against the scalar oracle covers 9 geometries × 6 level
  counts, and the byte-exact conformance suites are unchanged.

  What remains is the NEON and AVX2 kernel code, which cannot be written without
  `unsafe` while `core::simd` is unstable. Its `allow(unsafe_code)` moved from
  the whole file (507 lines) to the two `#[cfg]`-selected kernel modules, so
  exactly one is live per target and **none on either wasm target** — the
  published WASM bundle is built from `unsafe`-free first-party code.

  For contributors: `[workspace.lints.rust]` now sets `unsafe_code = "deny"` and
  `unsafe_op_in_unsafe_fn = "deny"`. Full inventory and the reasoning behind every
  kept block: [Unsafe Audit](docs/development/rust-198/unsafe-audit.md).

## v0.2.5 (2026-08-21)

### 🐛 Bug Fixes

- **ci**: pin wasmtime below 48 so ngff-zarr can still be imported
- **ci**: stop a release once for approval instead of twice

### 📚 Documentation

- **release**: correct the npm unpublish rules and where the run pauses

## v0.2.4 (2026-08-18)

### 🐛 Bug Fixes

- **ci**: ship the WASM core in the npm tarball, and fix the check that missed it

## v0.2.3 (2026-08-11)

### 🐛 Bug Fixes

- **python**: drop the numcodecs dependency to restore three wheel platforms

### 📚 Documentation

- sign and use nd-image-codecs message prefix for tags

## v0.2.2 (2026-08-10)

### 🐛 Bug Fixes

- **ci**: repin documented pins written as TOML literal strings
- **ci**: stamp the release version into the usage docs too

### 📚 Documentation

- Change tag command for release

## v0.2.1 (2026-08-04)

### ✨ Features

- **ci**: publish releases from a tag push with Trusted Publishing

### 🐛 Bug Fixes

- **ci**: close the moved-tag gaps and stop guessing on registry errors

### 📚 Documentation

- require two reviewers where self-review is prevented
- fix codec identifiers, a stale page count, and an unlinked test
- correct the ZFP provenance and unimplemented reference lanes
- retire the implementation roadmap and gate the myst toc in CI

## v0.1.0 (2026-08-04)

### ✨ Features

- **zfp**: adopt the registered zfp codec, with reshape collapsing singletons
- **ci**: cross-ecosystem validation matrix (zarrs × zarr-python × zarrita.js)
- **zarr**: numcodecs.delta codec + ndic zarr store I/O
- **zarr**: nd_lift WASM core + TypeScript encode/decode
- **ts**: nd_zfp WASM path — chunk-meta codec class + tests
- **py**: nd_zfp pyo3 chunk functions and the zarr-python NdZfpCodec
- **zarr**: register nd_zfp as a zarrs codec with brick-selective reads
- **zfp**: nd_zfp core over zfp-rs — modes, chunk codec, brick index
- **bindings**: wire htj2k to Python (pyo3) and TypeScript (WASM) cores
- **cli**: ndic index and thumbnail with HTTP Range execution; expand --partial
- **htj2k**: the htj2k Zarr codec, coefficient-plane index, and RangeIndex plans
- **codestream**: tiny committed fixtures, conformance fetch script, roadmap tick
- **cli**: ndic compress/expand/inspect on 2D PGM/PPM/PNG/raw images
- **htj2k**: SIMD DWT lane (NEON/AVX2/portable) and bench registrations
- **codestream**: .jph box format and multi-precinct decode; corpus conformance
- **codestream**: Part 1/15 codestream writer and reader with TLM/PLT
- **htj2k**: 2D DWT — reversible 5/3 and irreversible 9/7 lifting
- **htj2k**: scalar HT (FBCOT) block coder ported from OpenJPH
- **lift**: implement the nd_lift transform, zarrs codec, and validation lanes (Phase 2)
- **bench**: live harness — record IO, baselines, regression gates, nd-delta lanes
- **cli**: full option set for ndic series
- scaffold nd-image-codecs workspace

### 🐛 Bug Fixes

- **scripts**: drop unsatisfiable range members before the union span
- address PR #9 round-3 review findings
- **py**: export Zfp in __all__; clarify the schema-vs-parser invariant
- address PR #9 review findings
- address PR #8 review findings (CodeRabbit)
- address PR #8 review findings (Copilot)
- address PR #7 review findings (CodeRabbit + Copilot)
- **codestream**: bound adjusted missing MSBs after placeholder passes (PR #6 review)
- **scripts**: executable check and generator handling for ojph builds
- **codestream,cli**: harden parsers against malformed input (PR #6 review)
- **bindings**: reject a non-string axis in the Python validator
- **zarr**: own the partial decoder; pin fill values and the blosc typesize
- **lift**: wrapping arithmetic in the inverse kernels
- **lift**: guard zero-extent chunks and align the three config validators
- **bench**: report statuses honor the active gate
- **bench**: fail fast on unknown --config labels; strict zip in equality check
- **bench**: gate on compression ratio, not absolute bytes_out
- **ci**: grant bench-pr-gate pull-requests:write for the sticky comment

### ⚡ Performance

- **bench**: derive the PR gate from the baseline manifest's machine class
- **bench**: Tier 3 macro lanes, nightly + baseline-refresh workflows, profiling
- **zarr**: bulk widen/narrow in the nd_lift codec instead of per-element try_from
- **lift**: row-wise lifting — stream contiguous rows instead of gathering strided lines

### ♻️ Refactoring

- **bench**: move the Rust workloads into the bench driver

### 📚 Documentation

- fix codec identifiers, a stale page count, and an unlinked test
- correct the ZFP provenance and unimplemented reference lanes
- retire the implementation roadmap and gate the myst toc in CI
- check off the Phase 6 acceptance criteria
- **spec**: stage zarr-extensions codec specifications
- execute every usage-doc code block in CI
- import and register NdZfp in the zarrita example (CodeRabbit)
- Phase 5 status — nd_zfp over zfp-rs, architecture + usage updates
- address review feedback on the bench and publishing docs
- state the frontmatter/H1 rule as it actually behaves
- enable the zarrs feature in the Rust dependency example
- pin the site theme to a commit archive
- give every intra-docs link explicit text
- correct the ADR link-check paragraph, fix ts fences, record the canonical-URL limit
- polish the MyST site and record the toolchain decision
- deploy the MyST site to Read the Docs
- add page frontmatter, fix escaping links, gate on strict build
- scaffold the MyST documentation site
- remove JPEG 2000 submention in intro
- tick Phase 1 acceptance criteria; refresh commands, test-data, series docs
- add publishing a release docs

### 📊 Benchmarks

- **zfp**: dev-box baselines for the new nd-zfp lanes
- **zfp**: family lanes zfp-rate8/zfp-reversible + brick economy
- **lift-ht**: family-lane workloads + refreshed dev-box baselines

### 🧪 Tests

- **py**: OME-Zarr 0.5 multiscales lane via ngff-zarr and ome-zarr-py
- **py**: imagecodecs third-party lanes for JPEG 2000 and delta
- **py**: nd-delta end-to-end round-trip via zarr-python
- **zarr**: shared codec-series fixture matrix, tri-language tests, CI equality gate

### 🤖 Continuous Integration

- drop the removed bench feature from the clippy step
- block non-public destinations in the docs link check
- schedule the docs link check and document the docs gate
- build and publish the MyST site from the docs job
- cover ARM64 on Linux, Windows, and macOS in the test matrix

### 🎨 Style

- **htj2k,codestream**: rename single-char bindings for clippy pedantic
- **codestream**: clear remaining clippy pedantic warnings
