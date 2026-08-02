---
title: Overview
description: 'The whole nd-image-codecs design in one page: capture cross-axis correlation with an explicit transform, then compress the transformed planes with a proven 2D entropy backend.'
---

nd-image-codecs is a **family of composable Zarr v3 codecs** for ND scientific
images. Instead of one monolithic format, it provides a *builder* that
assembles a *series* (pipeline) of Zarr v3 codecs from an array's axis
metadata, plus the two novel codecs those pipelines need (`nd_lift` and
`nd_zfp`) and a fast HTJ2K plane codec (`htj2k`).

The unifying idea: **capture cross-axis correlation with an explicit,
independently specified transform, then compress the transformed planes with a
proven 2D/entropy backend.** Cross-axis decorrelation is never hidden inside a
JPEG 2000 Part 2 MCT — it is its own Zarr array-to-array codec (`nd_lift`), so
the whole system is free of Part 2 IP.

## The three families

```text
                 axis names (t,c,z,y,x…) + chunk shape + dtype
                                    │
                                    ▼
                         codec_series() builder
                    (chooses transpose order + decorrelation axes)
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
   ── nd-delta ──              ── nd-lift-ht ──             ── nd-zfp ──
   transpose                  transpose                    transpose
   numcodecs.delta            nd_lift  (array→array)        nd_zfp (array→bytes)
   bytes                      htj2k    (array→bytes)          │  ZFP 2/3/4D blocks
   blosc(bitshuffle,zstd/lz4)   │  Part 1/15 planes           │  + brick index
        │                        │  + coeff-plane index        │
        ▼                        ▼                             ▼
   fast lossless           scalable microscopy &         GPU volume rendering,
   from existing codecs    volume visualization          random access, fixed rate
```

| Family | Pipeline | Lossless? | Built for |
| --- | --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bitshuffle → zstd/lz4` | Yes | Fast lossless storage from **existing** Zarr codecs only |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | 5/3 lossless or 9/7 lossy | Resolution pyramids, thumbnails, streaming |
| **nd-zfp** | `transpose → nd_zfp` | Reversible or fixed-rate/-accuracy/-precision | GPU bricks, random access, predictable memory |

## Why this combination

- **Explicit cross-axis decorrelation.** A 1D lifting transform (`delta`,
  reversible `haar`, or reversible `5/3`) along z / time / channel captures the
  correlation that a per-plane 2D codec cannot see — the same *effect* a Part 2
  MCT-across-slices would give, but expressed as an ordinary, documented Zarr
  codec with no Part 2 syntax ([Zarr v3 core spec](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)).
- **HTJ2K for the planes.** HTJ2K (ISO/IEC 15444-15,
  [ITU-T T.814](https://www.itu.int/rec/T-REC-T.814)) replaces the EBCOT
  Tier-1 arithmetic coder with the FBCOT block coder, decoding ~10× faster than
  classic JPEG 2000 while keeping the wavelet, quantization, and RPCL
  progression that make thumbnails cheap
  ([JPEG white paper](https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf)).
- **ZFP for GPU/random-access.** ZFP's fixed-rate mode gives constant bits per
  block, so a renderer can index any brick in O(1) and bound memory exactly —
  the property volume renderers need
  ([LLNL ZFP](https://computing.llnl.gov/projects/zfp)).
- **Reuse over reinvention.** nd-delta is built entirely from existing,
  well-tested Zarr codecs (`numcodecs.delta` + `blosc` with bitshuffle); we add
  only the axis-aware transpose in front. Blosc+bitshuffle is already the
  workhorse of OME-Zarr microscopy
  ([Blosc microscopy study](https://pmc.ncbi.nlm.nih.gov/articles/PMC9900847/)).

## The codec-series builder

The builder is the heart of the system and is the one component fully
implemented today. Given each dimension's index and axis name plus the chunk
shape, it:

1. Chooses a **transpose order** that moves the fastest-moving dimensions into
   `(z)yx` order, placing `t` before `z` only when its chunk size (the grouping
   size) is not 1.
2. Chooses the **decorrelation axes** — by default `z`, and `t` when its chunk
   size is not 1 — all overridable with explicit / add / remove index lists.
3. Emits the family-specific tail.

It is implemented three times — Rust (`ndic-zarr`), pure Python
(`nd_image_codecs`), and pure TypeScript — with CI asserting byte-identical
output. See [codec-series.md](./codec-series.md).

## Crate boundaries

The workspace forms an acyclic graph:

| Crate | Role | Analogue |
| --- | --- | --- |
| `ndic-core` | Types only: errors, dtypes, params, views | OpenJPH `others/` |
| `ndic-lift` | `nd_lift` cross-axis lifting transform | — |
| `ndic-htj2k` | FBCOT block encode/decode | OpenJPH `coding/` |
| `ndic-codestream` | Part 1/15 markers, packets, tiles, byte-range index | OpenJPH `codestream/` |
| `ndic-zfp` | `nd_zfp` Rust ZFP port | LLNL ZFP |
| `ndic-zarr` | The three Zarr v3 codecs + `codec_series` builder | — |
| `ndic-cli` | `ndic` binary | `ojph_compress` / `ojph_expand` |

Two deliberate departures from the C++ references: **multithreading** (OpenJPH
is single-threaded; nd-image-codecs parallelizes across code-blocks with
`rayon`) and **runtime-dispatched SIMD in Rust** (`core::arch` intrinsics
behind `is_x86_feature_detected!` / `is_aarch64_feature_detected!`, with a
portable `wide` fallback — including first-class NEON, which OpenJPH still
stubs out).

## Data model

The canonical in-memory layout is row-major `[…, z, y, x]` with `x` fastest
(`ndic_core::VolumeView`). Sample types are 8/16/32/64-bit signed and unsigned
integers and 32/64-bit floats (`SampleType`); the reversible 5/3 and ZFP
reversible paths are bit-exact, the 9/7 and fixed-rate ZFP paths are lossy.
Defaults are chosen so that a series produced with no options is immediately
OME-Zarr-friendly: RPCL progression, 64×64 code-blocks, `TLM`+`PLT` emitted,
decorrelation along grouped z (and grouped t).
