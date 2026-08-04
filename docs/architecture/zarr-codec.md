---
title: Zarr Codecs
description: 'How nd-image-codecs plugs into the Zarr v3 codec pipeline across three ecosystems: Rust zarrs, Python zarr-python, and TypeScript/WASM zarrita.js.'
---

**Crate:** [`ndic-zarr`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-zarr) + bindings · **Roadmap:**
Phases [1](../development/roadmap/phase-1-baselines-and-series.md)–[5](../development/roadmap/phase-5-nd-zfp.md)

nd-image-codecs contributes to the Zarr v3 codec pipeline in three ecosystems:
Rust ([`zarrs`](https://docs.rs/zarrs)), Python
([`zarr-python`](https://zarr.readthedocs.io) / [numcodecs](https://numcodecs.readthedocs.io)),
and TypeScript/WASM ([numcodecs.js](https://github.com/manzt/numcodecs.js) for
zarrita.js). It ships **two new codecs** and **composes a third family** from
existing codecs, all wired together by the [codec-series](./codec-series.md)
builder.

## The Zarr v3 codec model

Zarr v3 splits a codec pipeline into array→array, array→bytes, and bytes→bytes
stages ([Zarr v3 core spec](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)).
nd-image-codecs uses each stage deliberately:

| Codec | Kind | Role |
| --- | --- | --- |
| `transpose` (stock) | array → array | Put the fastest/decorrelation axes where the tail codec expects them |
| `nd_lift` (**new**) | array → array | Explicit cross-axis (z/t/c) lifting decorrelation — see [the cross-axis transform](./nd-transform.md) |
| `numcodecs.delta` (stock) | array → array | Single-axis differencing (nd-delta family) |
| `htj2k` (**new**) | array → bytes | Compress each trailing 2D plane as an independent Part 1/15 codestream + coefficient-plane index |
| `zfp` (**registered**) | array → bytes | ZFP 1D–4D blocks + brick index — see [the Rust ZFP port](./zfp.md) |
| `bytes`, `blosc`, `crc32c` (stock) | array→bytes / bytes→bytes | Endianness, entropy backend, checksums |

A JPEG 2000 or ZFP codec **must be the array→bytes stage**: it needs the chunk's
shape and dtype, and its output is an opaque byte stream. This matches how
existing image codecs slot into Zarr (cf. glencoesoftware's 2D
[zarr-jpeg2k](https://github.com/glencoesoftware/zarr-jpeg2k)).

## Chunk mapping

A Zarr chunk of shape `[…, z, y, x]` maps onto the series as follows:

- `y, x` → the 2D plane the array→bytes codec compresses;
- `z` (and grouped `t`) → decorrelated by `nd_lift` (nd-lift-ht) or carried as
  ZFP block dimensions (nd-zfp), never crossing a chunk boundary so Zarr's
  parallel read/write model stays intact;
- leading size-1 axes (OME-Zarr `t`/`c` singletons) are left in place by the
  builder — no transform is placed on a size-1 axis;
- dtype coverage: `uint8/int8/uint16/int16/uint32/int32/uint64/int64` for the
  integer paths, plus `float32/float64` for nd-zfp.

Codec configurations are produced by the builder; see
[codec series](./codec-series.md) for the JSON.

## Why this fills a real gap

Surveyed OME-Zarr practice uses general-purpose bytes compressors
(blosc/zstd/gzip) — no cross-axis decorrelation, no progressive access, no
GPU-friendly fixed-rate mode ([OME-NGFF spec](https://ngff.openmicroscopy.org/latest/),
[Blosc microscopy study](https://pmc.ncbi.nlm.nih.gov/articles/PMC9900847/)).
`zarrs` ships no JPEG 2000 or ZFP array codec; `imagecodecs` wraps OpenJPEG's
classic JPEG 2000 and ZFP but not HTJ2K encode
([imagecodecs](https://pypi.org/project/imagecodecs/)); numcodecs.js has neither.
nd-image-codecs brings (a) explicit z/t/c decorrelation, (b) fast, SIMD-friendly
HTJ2K decode in browsers via WASM, and (c) a random-access ZFP path.

## Per-ecosystem integration

| Ecosystem | Mechanism |
| --- | --- |
| **Rust / zarrs** | Implement `ArrayToArrayCodecTraits` (`nd_lift`) and `ArrayToBytesCodecTraits` (`htj2k`, `zfp`) + `CodecTraits`; register via `inventory` link-time plugin submission ([zarrs codec guide](https://book.zarrs.dev/extensions/codec.html)) |
| **Python / zarr-python v3** | PyO3 `abi3` extension via [maturin](https://www.maturin.rs/); codec classes exported through the `zarr.codecs` entry points (`nd_lift`, `htj2k`, `zfp`, `reshape`, and the deprecated `nd_zfp` alias) ([zarr extending guide](https://zarr.readthedocs.io/en/stable/user-guide/extending.html)) |
| **TypeScript / numcodecs.js** | `wasm-pack` build of the core (wasm32 + SIMD128); codec classes with `fromConfig` following the numcodecs.js per-codec convention, registerable in zarrita.js |

The `codec_series` builder itself is pure metadata and is implemented natively
in all three languages (no WASM needed) so pipeline authoring works everywhere,
byte-identically.

## Registration and naming

The project-defined codec names (`nd_lift`, `htj2k`) follow the Zarr v3
extension naming convention pending a formal registration
([ZEP 2 / extension naming](https://zarr.dev/zeps/accepted/ZEP0002.html),
[zarr-extensions](https://github.com/zarr-developers/zarr-extensions)); their
specification documents are staged in
[`spec/codecs/`](https://github.com/fideus-labs/nd-image-codecs/tree/main/spec/codecs)
in the layout zarr-extensions expects, and CI checks the schemas against
every configuration the builder emits so they cannot drift from the
implementations. The nd-delta family uses only already-registered names
(`transpose`, `numcodecs.delta`, `bytes`, `blosc`).

The nd-zfp family **adopts registered names outright**: its pipeline is
`transpose → reshape → zfp`, where both
[`zfp`](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/zfp)
and
[`reshape`](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/reshape)
are zarr-extensions registrations this project implements rather than names
it invents. The codec formerly registered here as `nd_zfp` produced
byte-identical streams; only the name and the handling of chunks above four
dimensions differed, so a second name was not worth the ecosystem
fragmentation. `nd_zfp` remains a **read alias**: metadata under that name
(recognizable by its legacy `dims` member, which selects the old in-codec
squeeze-and-pad mapping) keeps decoding byte-for-byte in every ecosystem.
Vendored copies of the registered `zfp`/`reshape` schemas sit in
[`spec/vendor/`](https://github.com/fideus-labs/nd-image-codecs/tree/main/spec/vendor)
and CI validates every builder-emitted configuration against them.

## Partial-read synergy

Because each nd-lift-ht plane is an RPCL codestream with `TLM`/`PLT`, and each
nd-zfp chunk carries a brick index, a reader holding only *part* of a chunk's
bytes can still produce a low-resolution or single-brick result — the
[byte-range access](./range-access.md) machinery applies within chunks, enabling
multiscale-on-demand for viewers even before OME-Zarr pyramid levels are
consulted.

## Testing

- Tri-ecosystem builder equality: the shared fixture matrix through Rust,
  Python, and TS asserts byte-identical pipelines.
- Round-trip: encode in Rust → decode in Python and TS on shared fixtures.
- Third-party validation: decode our output with `imagecodecs` (ZFP, JPEG 2000,
  delta) via `zarr-python`, and decode `imagecodecs` output with ours (Phase 6).
- OME-Zarr integration: write a `0.5` multiscales volume; validate with
  `ome-zarr-py` and `ngff-zarr` readers.
