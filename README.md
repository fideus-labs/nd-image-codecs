<p align="center">
  <img src="docs/assets/nd-image-codecs-logo.svg" alt="nd-image-codecs" width="160" />
</p>

<h1 align="center">nd-image-codecs</h1>

<p align="center">
  <a href="https://github.com/fideus-labs/nd-image-codecs/actions/workflows/ci.yml"><img src="https://github.com/fideus-labs/nd-image-codecs/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
  <a href="https://github.com/fideus-labs/nd-image-codecs/actions/workflows/bench-pr-gate.yml"><img src="https://github.com/fideus-labs/nd-image-codecs/actions/workflows/bench-pr-gate.yml/badge.svg" alt="Bench" /></a>
  <a href="https://nd-image-codecs.readthedocs.io/en/latest/"><img src="https://readthedocs.org/projects/nd-image-codecs/badge/?version=latest" alt="Documentation" /></a>
  <a href="LICENSE.txt"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License" /></a>
</p>

<p align="center">
  <strong>Composable Zarr v3 codecs for ND scientific images.</strong>
</p>

<p align="center">
  📖 <a href="https://nd-image-codecs.readthedocs.io/en/latest/"><strong>Read the documentation</strong></a>
  — architecture, usage guides, and contributor docs, rendered and searchable.
</p>

<p align="center">
  A family of Zarr v3 codecs that capture correlation along z, time, and
  channel axes <em>explicitly</em> — as ordinary, independently specified
  array-to-array and array-to-bytes codecs — then store the result with a fast
  entropy backend, High-Throughput JPEG 2000 (ISO/IEC 15444-15) coefficient
  planes, or ZFP blocks. Rust core with Python and TypeScript bindings, built
  for OME-Zarr / OME-NGFF.
</p>

## ✨ The three codec families

nd-image-codecs is not one codec but a **builder** that assembles a *series*
(pipeline) of Zarr v3 codecs from an array's axis metadata. Three families
trade off ratio, speed, and access pattern:

| Family | Series (pipeline) | Built for |
| --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bitshuffle → zstd/lz4` | Fast lossless storage from **existing** Zarr codecs only |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | Scalable microscopy & volume visualization (resolution pyramids, thumbnails) |
| **nd-zfp** | `transpose → reshape → zfp` | GPU volume rendering, random access, predictable (fixed-rate) memory |

Each family is produced by [`codec_series`](docs/architecture/codec-series.md),
which chooses a transpose order and decorrelation axes from the axis names
(`t`, `c`, `z`, `y`, `x`, …) and chunk shape — all overridable.

## 🧱 The codecs

- **`nd_lift`** — an explicit, independently specified **array-to-array** Zarr
  v3 codec that applies a 1D lifting transform (`delta`, reversible `haar`, or
  reversible `5/3`) along chosen non-spatial axes (z, time, channel). This is
  how nd-image-codecs captures cross-axis correlation *without* JPEG 2000 Part 2
  MCT syntax: the transform runs first, then ordinary 2D coding compresses the
  resulting planes. See [`docs/architecture/nd-transform.md`](docs/architecture/nd-transform.md).
- **`htj2k`** — an **array-to-bytes** codec that compresses each trailing 2D
  (y, x) plane as an independent, conforming JPEG 2000 **Part 1 / Part 15**
  (HTJ2K) codestream, with an outer coefficient-plane byte index for
  range-request thumbnails. The FBCOT block coder (MEL / VLC / MagSgn) decodes
  roughly an order of magnitude faster than classic JPEG 2000.
- **`zfp`** — an **array-to-bytes** codec (the zarr-extensions registered name): a clean-room Rust port of
  [LLNL ZFP](https://github.com/LLNL/zfp) for 2D/3D/4D blocks with fixed-rate,
  fixed-accuracy, fixed-precision, and reversible modes, plus a brick index for
  random access. See [`docs/architecture/zfp.md`](docs/architecture/zfp.md).

## 🧩 Crates

| Crate | Description |
| --- | --- |
| [`crates/ndic-core`](crates/ndic-core/) | Shared types: errors, sample dtypes, encode parameters, plane/volume views |
| [`crates/ndic-lift`](crates/ndic-lift/) | The `nd_lift` cross-axis lifting transform (`delta` / `haar` / `5/3`) |
| [`crates/ndic-htj2k`](crates/ndic-htj2k/) | The HT (FBCOT) block coder: cleanup, SigProp, MagRef passes and inverses |
| [`crates/ndic-codestream`](crates/ndic-codestream/) | Part 1 / Part 15 codestream reader/writer, marker segments (`SIZ`/`COD`/`CAP`/`TLM`/`PLT`), byte-range index |
| [`crates/ndic-zfp`](crates/ndic-zfp/) | The `zfp` Rust ZFP port (2D/3D/4D), reproducing upstream test vectors |
| [`crates/ndic-zarr`](crates/ndic-zarr/) | The three Zarr v3 codecs + the `codec_series` builder (also the WASM core for TypeScript) |
| [`crates/ndic-cli`](crates/ndic-cli/) | `ndic` CLI: `compress` / `expand` / `series` / `inspect` / `index` / `thumbnail` |

## 🔗 Bindings

| Binding | Path | Ecosystem |
| --- | --- | --- |
| Python | [`bindings/python/nd-image-codecs`](bindings/python/nd-image-codecs/) | `zarr-python` v3 + `numcodecs` (PyO3 / maturin, abi3) |
| TypeScript | [`bindings/typescript`](bindings/typescript/) | `numcodecs.js` / zarrita.js (wasm-bindgen, WASM SIMD128) |

The `codec_series` builder is implemented three times — Rust, pure Python, and
pure TypeScript — and CI asserts all three produce byte-identical pipelines, so
you can author array metadata from any ecosystem.

## 🚀 Quick start

```sh
# Emit a Zarr v3 codec series for a t,c,z,y,x uint16 array (nd-lift-ht family).
# Grouped t (chunk>1) and z get an nd_lift transform; planes go to htj2k.
ndic series --axes t,c,z,y,x --chunks 8,1,32,256,256 --dtype uint16 --family nd-lift-ht

# Fast lossless storage from existing codecs only:
ndic series --axes t,c,z,y,x --chunks 1,1,32,256,256 --dtype uint16 --family nd-delta

# Fixed-rate float volume for GPU bricks:
ndic series --axes z,y,x --chunks 64,256,256 --dtype float32 --family nd-zfp
```

```python
from nd_image_codecs import codec_series

# Same builder, pure Python — drop straight into a zarr-python array's codecs.
codecs = codec_series(["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256],
                      "uint16", "nd-lift-ht")
```

> All three families encode and decode for real as of 0.1.0. The API is not
> stable before 1.0 — see [publishing](docs/development/publishing.md) for what
> is on each registry.

## 🛠️ Development

### Prerequisites

- Rust 1.91+ (see [`rust-toolchain.toml`](rust-toolchain.toml); wasm targets included)
- Python 3.11+ and [maturin](https://www.maturin.rs/) for the Python binding
- Node 20+ and [wasm-pack](https://rustwasm.github.io/wasm-pack/) for the TypeScript binding

### Setup

```sh
git clone https://github.com/fideus-labs/nd-image-codecs
cd nd-image-codecs
cargo check --workspace
cargo test --workspace
```

### Monorepo structure

```
nd-image-codecs/
├── crates/            # Rust core (core, lift, htj2k, codestream, zfp, zarr, cli)
├── bindings/          # python/ (PyO3+maturin), typescript/ (wasm-bindgen)
├── bench/             # benchmark driver, baselines, viewer, docs
├── docs/              # architecture/, development/, usage/
├── scripts/           # CI and development helper scripts
└── .github/workflows/ # ci.yml, bench-pr-gate.yml
```

### Commands

| Command | Purpose |
| --- | --- |
| `cargo test --workspace` | Run all unit, integration, and doc tests |
| `cargo clippy --workspace --all-targets` | Lint (clippy `all` + `pedantic`) |
| `cargo fmt --all` | Format |
| `cargo run -p ndic-cli -- series --chunks 1,1,32,256,256` | Run the `ndic` CLI from source |
| `cargo run -p ndic-bench-cli -- run` | Run the benchmark suite |
| `cargo run -p ndic-bench-cli -- run --baseline main --fail-on-regression` | PR regression gate, locally |

See [docs/development/commands.md](docs/development/commands.md) for the full list.

## 📚 Documentation

The rendered site is at **<https://nd-image-codecs.readthedocs.io/en/latest/>** —
read it there rather than browsing the markdown below. Deployment and the manual
Read the Docs setup: [docs/development/read-the-docs.md](docs/development/read-the-docs.md).

- [Architecture](docs/architecture/index.md) — the codec series builder, the `nd_lift` transform, the HTJ2K plane codec, the ZFP port, codestream syntax, and range access
- [Development](docs/development/) — commands, style, commits, benchmarking, test data, [publishing](docs/development/publishing.md)
- [Usage](docs/usage/index.md) — CLI, Rust, Python, TypeScript, and Zarr/OME-Zarr guides

## 🤝 Contributing

Start with [AGENTS.md](AGENTS.md) for the repository map and conventions, and the
[open issues](https://github.com/fideus-labs/nd-image-codecs/issues) for what to
pick up next. All participation is governed by our
[Code of Conduct](CODE_OF_CONDUCT.md).

## 📄 License

MIT — see [LICENSE.txt](LICENSE.txt). Copyright (c) Fideus Labs LLC.

nd-image-codecs is an independent clean-room Rust project. Its HTJ2K coding is
inspired by [OpenJPH](https://github.com/aous72/OpenJPH) (BSD-2-Clause, © Aous
Naman) and its ZFP codec is a clean-room port of
[LLNL ZFP](https://github.com/LLNL/zfp) (BSD-3-Clause); both port ideas and
conformance behavior, not source code. No JPEG 2000 Part 2 (MCT) syntax is used.
