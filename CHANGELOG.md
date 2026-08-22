<!-- Hand-written ahead of the generator. CHANGELOG.md is produced by
     `uvx --from commitizen cz changelog --incremental` (see .cz.toml), which
     inserts the next `## vX.Y.Z` section below this point from the Conventional
     Commits themselves.

     Merge this Unreleased block into that section by hand at release time —
     do NOT just delete it. Two things the generator does not reproduce, both
     confirmed by running
     `uvx --from commitizen cz changelog --unreleased-version=v0.3.0 --dry-run`
     on this branch:

     1. Every commit of the Rust 1.98 migration except the MSRV bump itself
        carries a `MAESTRO: ` subject prefix. `.cz.toml`'s `commit_parser` is
        applied with `re.match`, so it anchors on the type and none of them
        parse — each is dropped silently, and the MSRV bump is all of this
        branch's work that survives into the generated section. Stripping the
        prefix from those subjects before tagging is the other way out.
     2. `!` alone does not reach 💥 Breaking Changes here: the MSRV commit is
        `build!:` and renders under 📦 Build. `change_type_map` can only rename
        a `change_type` the parser captured, and the parser's breaking
        alternative (`\w+!`) captures none — so no commit shape reaches that
        section without a `.cz.toml` change. -->

## Unreleased

**Nothing this release encodes or decodes changes — in any codec, lossy or
lossless.** The Rust 1.98 migration is byte-compatible with 0.2.4 in both
directions; the evidence is under ⚡ Performance below. What *is*
consumer-visible is the new Rust 1.98 floor, which is a hard requirement for
anyone building against the crates.

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
  See [Rust 1.98 Adoption](docs/development/rust-198/index.md) for the whole
  migration in one page, and
  [Adoption Notes](docs/development/rust-198/adoption-notes.md) for the
  phase-by-phase record.

- **codestream**: replace `bitio::HeaderBitReader::new_at` with `new_in`

  `new_at(sub, base)` took a sub-slice and a byte offset the caller had already
  added `sub`'s own start into. `new_in(parent, sub, base)` takes the two slices
  and derives that start with `subslice_range`, so the pair can no longer
  disagree; `terminate()` correspondingly returns the header length measured in
  `parent` rather than in `sub`. Migration: `new_at(&buf[n..], base + n)` becomes
  `new_in(&buf, &buf[n..], base)`, and callers drop the matching `n +` from what
  `terminate()` returns. `HeaderBitReader::new` is unchanged.

### ⚡ Performance

- **htj2k**: measure and reject algebraic float in the 9/7 DWT

  Rust 1.98's `f32::algebraic_add` / `algebraic_mul` let the backend reassociate
  and contract float arithmetic. They were applied to the irreversible CDF 9/7
  lifting kernel, measured, and **reverted**: this workspace builds for the
  baseline `x86-64` target, which has no FMA to contract into, so both forms
  compiled to identical instructions and produced bit-identical output over 1 M
  samples. **No `algebraic_*` call ships anywhere in the workspace.** What lands
  is the written evidence and a `transform/dwt97_fwd_2048` benchmark, so the
  experiment is not repeated.

  **For anyone reading or writing data through the Python or TypeScript
  binding, this means no encoded byte moved** — the lossy paths included, which
  is worth stating plainly, because a silent lossy-output change is the kind
  that gets discovered months later:

  - Arrays written by 0.2.4 or earlier decode identically. No codec identifier,
    configuration default, or on-disk layout changed.
  - Re-encoding the same array with this release gives you the same bytes.
    Verified rather than assumed: the 104-case ZFP checksum matrix — **72 of
    those cases lossy** (`fixed_rate`, `fixed_precision`, `fixed_accuracy`),
    each pinned by an FNV-1a-64 over its exact stream — reproduces byte for
    byte; 2000 OpenJPH differential vectors still match the oracle; the interop
    suite stays bit-exact in both directions against `ojph_compress` /
    `ojph_expand`; 7 conformance corpus streams decode bit-exactly; `nd_lift`
    reproduces its committed vectors exactly; and 47 ratio-carrying benchmark
    records across `htj2k`, `lift_ht`, `nd_delta`, `nd_lift`, `nd_zfp`, and
    `zfp` reproduce the committed baseline's compression ratios to `f64` bit
    equality.
  - The one place a float result could have moved is the irreversible 9/7
    wavelet. The revert makes that kernel executable-identical to 0.2.4, so
    even a direct `ndic_htj2k::dwt::forward_97` caller gets the same floats —
    and no codec reaches it in any case: `ndic-codestream`'s writer accepts
    only `WaveletKind::Reversible53` and returns `Unsupported` otherwise, which
    makes `htj2k` and `nd-lift-ht` lossless end to end. Lossy compression here
    is ZFP's three modes, whose arithmetic belongs to `zfp-rs` and was not
    touched.

  The one measurable speed change in this release is the DWT row splitter,
  under ♻️ Refactoring below. Method and numbers for all of the above:
  [Rust 1.98 Adoption](docs/development/rust-198/index.md).

### ♻️ Refactoring

- **codestream**: derive marker and packet offsets from the slices themselves

  The main-header and tile-part marker loops recomputed `pos + 2 + len` in a
  bounds check and again in the cursor advance, with the payload slice built
  between them; the packet-header reader threaded a `SOP` skip through four
  expressions. Both now take the offset from the slice with `subslice_range`.
  Byte-identical output and input handling — 65 captured plan, packet-dump, and
  decoded-image artifacts are unchanged across the conversion, and three tests
  were added, including for the previously uncovered `Scod` bit 1 `SOP` path.
  See [Ergonomic Sweep](docs/development/rust-198/ergonomic-sweep.md).

- **htj2k**: make the DWT row splitter safe and deny `unsafe_code` workspace-wide

  `ndic-htj2k` is the only published crate with an `unsafe` surface, and it is
  now smaller. `dwt::simd::split_three` built three row slices out of one raw
  pointer, with disjointness argued in a comment; it uses `split_at_mut` on the
  destination row instead, and the block is gone. Output is bit-identical — the
  differential test against the scalar oracle covers 9 geometries × 6 level
  counts, and the byte-exact conformance suites are unchanged.

  **It is also faster, which was not the goal.** That splitter sits in the
  vertical pass of the 5/3 wavelet, which every `htj2k` encode and decode runs,
  so removing the raw pointer moved the shipped path: `transform/dwt53_fwd_2048`
  is **10–17 % faster** on its SIMD lanes. `htj2k/plane_encode_1024` — the whole
  plane encode, of which that transform is one component — improved **1.5–3.8 %**
  over the same range of commits, not separately attributed. Plane decode is
  unchanged (−0.9 % to 0.0 %, inside the noise), and no other workload moved
  with a consistent sign. The splitter figure was measured on x86-64/AVX2 by
  interleaving two binaries built from one tree differing only in that
  function's body; the code is not target-specific, so aarch64 and the WASM
  bundle run the same change, unmeasured. Compressed output is identical either
  way — this is throughput only.

  What remains is the NEON and AVX2 kernel code, which cannot be written without
  `unsafe` while `core::simd` is unstable. Its `allow(unsafe_code)` moved from
  the whole file (507 lines) to the two `#[cfg]`-selected kernel modules (81
  lines), so exactly one is live per target and **none on either wasm target** —
  the published WASM bundle is built from `unsafe`-free first-party code. Across
  the crate that is 10 `unsafe` keywords down to 9, and the one block resting on
  a hand-written aliasing argument down to none.

  For contributors: `[workspace.lints.rust]` now sets `unsafe_code = "deny"` and
  `unsafe_op_in_unsafe_fn = "deny"`. Full inventory and the reasoning behind every
  kept block: [Unsafe Audit](docs/development/rust-198/unsafe-audit.md).

### 📦 Build

- ship a README with every published crate

  `ndic-lift`, `ndic-zfp`, and `ndic-zarr` inherited `homepage` from the
  workspace but not `readme`, so cargo found no README in their own directories
  and published none — their crates.io pages have rendered empty since 0.1.0.
  All seven published crates now inherit both. This is packaging metadata only;
  no code, no dependency, and no version changed.

- keep `__pycache__` out of the Python wheel

  maturin copies `python-source` verbatim, so a wheel built from a working tree
  that had run the test suite carried that tree's `__pycache__` — bytecode for
  interpreter versions the wheel was not built against. The release workflow
  never produced one (every job builds from a fresh checkout); the hand-publish
  path in [Publishing](docs/development/publishing.md) did.

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
