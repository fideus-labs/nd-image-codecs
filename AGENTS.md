# AGENTS.md — nd-image-codecs

## Overview

nd-image-codecs is a Rust monorepo providing a **family of composable Zarr v3
codecs** for ND scientific images (OME-Zarr / OME-NGFF microscopy and
volumetric data). Rather than a monolithic format, it exposes a builder that
assembles a *series* (pipeline) of Zarr v3 codecs from an array's axis
metadata. Three families are provided:

- **nd-delta** — `transpose → numcodecs.delta → bitshuffle → zstd/lz4` (existing codecs only; fast lossless).
- **nd-lift-ht** — `transpose → nd_lift → htj2k` (cross-axis lifting + HTJ2K coefficient planes).
- **nd-zfp** — `transpose → nd_zfp` (ZFP blocks + brick index).

Cross-axis correlation (z, time, channel) is captured by an **explicit**
array-to-array codec, `nd_lift`, and never by JPEG 2000 Part 2 (MCT) syntax —
this is a deliberate IP choice. The `htj2k` codec emits only conforming JPEG
2000 **Part 1 / Part 15** codestreams. Work proceeds in six roadmap phases;
consult the roadmap before implementing anything.

## Crates

| Crate | Read |
| --- | --- |
| Core types (errors, dtypes, params, views) | [./crates/ndic-core/](./crates/ndic-core/) |
| `nd_lift` cross-axis lifting transform | [./crates/ndic-lift/](./crates/ndic-lift/) |
| HT (FBCOT) block coder | [./crates/ndic-htj2k/](./crates/ndic-htj2k/) |
| Codestream syntax + indexing (Part 1/15) | [./crates/ndic-codestream/](./crates/ndic-codestream/) |
| `nd_zfp` Rust ZFP port (2D/3D/4D) | [./crates/ndic-zfp/](./crates/ndic-zfp/) |
| Zarr v3 codecs + `codec_series` builder (Rust + WASM core) | [./crates/ndic-zarr/](./crates/ndic-zarr/) |
| `ndic` CLI | [./crates/ndic-cli/](./crates/ndic-cli/) |
| Python binding (PyO3/maturin) | [./bindings/python/nd-image-codecs/](./bindings/python/nd-image-codecs/) |
| TypeScript binding (WASM) | [./bindings/typescript/](./bindings/typescript/) |
| Benchmark layer | [./bench/](./bench/) |

## Context to load on demand

| Task | Read |
| --- | --- |
| Understand the overall design | [./docs/architecture/index.md](./docs/architecture/index.md), [./docs/architecture/overview.md](./docs/architecture/overview.md) |
| Decide what to implement next | [./docs/development/roadmap/index.md](./docs/development/roadmap/index.md) |
| Work on the codec-series builder | [./docs/architecture/codec-series.md](./docs/architecture/codec-series.md) |
| Work on the `nd_lift` transform | [./docs/architecture/nd-transform.md](./docs/architecture/nd-transform.md) |
| Work on the HT block coder | [./docs/architecture/ht-block-coder.md](./docs/architecture/ht-block-coder.md) |
| Work on the in-plane 2D wavelet | [./docs/architecture/wavelet-transform.md](./docs/architecture/wavelet-transform.md) |
| Work on the ZFP port | [./docs/architecture/zfp.md](./docs/architecture/zfp.md) |
| Work on markers / codestream IO | [./docs/architecture/codestream.md](./docs/architecture/codestream.md) |
| Thumbnails / HTTP Range access | [./docs/architecture/range-access.md](./docs/architecture/range-access.md) |
| Zarr codecs (Rust/Python/TS) | [./docs/architecture/zarr-codec.md](./docs/architecture/zarr-codec.md) |
| Run or add benchmarks | [./docs/development/benchmarking.md](./docs/development/benchmarking.md), [./bench/README.md](./bench/README.md) |
| Everyday commands | [./docs/development/commands.md](./docs/development/commands.md) |
| Publish a release (crates.io, PyPI, npm) | [./docs/development/publishing.md](./docs/development/publishing.md) |
| Rust style rules | [./docs/development/style/rust.md](./docs/development/style/rust.md) |
| Commit message format | [./docs/development/commits.md](./docs/development/commits.md) |
| Test data & conformance corpus | [./docs/development/test-data.md](./docs/development/test-data.md) |
| Build or preview the documentation site | [./docs/development/commands.md](./docs/development/commands.md) |
| Deploy the documentation site (Read the Docs) | [./docs/development/read-the-docs.md](./docs/development/read-the-docs.md), [./.readthedocs.yaml](./.readthedocs.yaml) |
| Why the documentation toolchain is what it is | [./docs/development/decisions/adr-001-documentation-toolchain.md](./docs/development/decisions/adr-001-documentation-toolchain.md) |

## Conventions

- Conventional Commits with crate scopes: `feat(lift): …`, `fix(codestream): …`,
  `docs: …` — see [./docs/development/commits.md](./docs/development/commits.md).
- Clippy `all` + `pedantic` at warn, workspace-inherited; keep `cargo clippy
  --workspace --all-targets` clean before committing (CI runs `-D warnings`).
- Every fallible public API returns `ndic_core::Result<T>`; do not add new
  error enums — extend `ndic_core::Error` message context instead. (The
  `codec_series` builder is the one exception: it has its own `SeriesError`
  because it is pure metadata with no codec dependency.)
- In-memory volume layout is row-major `[z, y, x]`, `x` fastest.
- **The `codec_series` builder is implemented three times** — Rust
  (`ndic-zarr`), pure Python (`nd_image_codecs`), and pure TypeScript. Any
  change to one must be mirrored in the others; CI asserts byte-identical
  output across all three.
- No JPEG 2000 Part 2 (MCT) markers may be emitted or parsed. Cross-axis
  decorrelation is exclusively the `nd_lift` codec.
- New performance-sensitive code must register a benchmark
  (`inventory::submit! { BenchEntry::new(...) }`) in the same PR.
- Roadmap phases are strictly ordered; do not start a phase's work before its
  predecessors' acceptance criteria are met.
- Any new page added under `docs/` must also be added to the `toc` in
  `docs/myst.yml`, or it will not appear on the documentation site — the toc is
  explicit, not filesystem-discovered.
- Links from `docs/` into source code, benchmarks, or scripts must be absolute
  `https://github.com/fideus-labs/nd-image-codecs/blob/main/…` (or
  `/tree/main/…` for a directory) URLs. The MyST project root is `docs/`, so a
  relative path that escapes it (`../../crates/…`) 404s on the rendered site;
  absolute URLs render identically on GitHub, so there is no cost. Links
  *between* pages inside `docs/` stay relative and carry **explicit link text**
  — `[Overview](./overview.md)`, never `[](./overview.md)`. MyST auto-fills an
  empty label from the target's title, but GitHub emits a literal empty anchor,
  so the link vanishes for anyone reading the file in the repository; the
  auto-filled title is also usually too long to read well mid-sentence.
- Every page under `docs/` carries YAML frontmatter with a `title` and a
  one-sentence `description` (plus `short_title` when the title is too long for
  a sidebar entry), then one `#` heading repeating that title, then `##`
  sections below it. Keep the `#`: frontmatter is not a heading, so for anyone
  reading the file on GitHub or in a plain markdown viewer it is the page's only
  title. mystmd consumes that leading `#` and titles the page from the
  frontmatter instead, so the rendered page still carries exactly one `<h1>` —
  the repetition exists in the source and never in the output, and removing the
  heading to "deduplicate" it only costs the in-repo reader. Frontmatter titles
  are plain text: mystmd renders them as a literal string, so backticks would
  show up verbatim in the sidebar, while the `#` heading is ordinary markdown
  and may use them.
- Fenced code blocks under `docs/` must carry a language the site's highlighter
  (highlight.js, via `book-theme`) actually knows, matched by exact name — it
  does not resolve aliases. Use `bash` for shell commands (**not** `sh`, `zsh`,
  or `console`, all of which silently fall back to unhighlighted `text`),
  `shell` for a `$`-prompt transcript, `rust` (not `rust,ignore`), `python`,
  `typescript`, `json`, `toml`, `yaml` — and `text` deliberately, for ASCII
  diagrams and literal strings that are not code.
- Run `cd docs && npm run check` (strict build, fails on any warning) before
  pushing documentation changes. The `docs` job in CI runs that same script on
  every pull request, so a broken link or an unresolved cross-reference will
  block the merge; the rendered site is downloadable from the run as the
  `docs-site` artifact. External links are checked by a separate monthly
  workflow that is deliberately not a PR gate.

## Key dependencies

| Dependency | Version | Notes |
| --- | --- | --- |
| `thiserror` | 2 | Error derive for `ndic_core::Error` and `SeriesError` |
| `serde` / `serde_json` | 1 | Codec-series JSON metadata |
| `rayon` | 1 | Code-block-level encode/decode parallelism |
| `wide` | 0.7 | Portable SIMD fallback lanes (native paths use `core::arch`) |
| `zarrs` | 0.23 | Zarr v3 codec traits + plugin registry (feature-gated) |
| `inventory` | 0.3 | Link-time registration: zarrs codec + bench entries |
| `zfp-sys` | (ref lane) | FFI to upstream ZFP C for port parity checks |
| `pyo3` / `numpy` | 0.24 | Python binding, `abi3-py311` |
| `wasm-bindgen` | 0.2 | TypeScript/WASM binding |
| `clap` | 4 | `ndic` and `ndic-bench` CLIs |
| `criterion` | 0.5 | Micro-benchmarks (the bench layer wraps its own timing) |
| `proptest` | 1 | Round-trip property tests (encode∘decode = identity) |
