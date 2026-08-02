---
title: nd-image-codecs
description: 'Composable Zarr v3 codecs for ND scientific images: cross-axis lifting, HTJ2K coefficient planes, and ZFP blocks, assembled by an axis-aware codec-series builder.'
---

# nd-image-codecs

**Composable Zarr v3 codecs for ND scientific images.**

nd-image-codecs is not one codec but a **builder** that assembles a *series*
(pipeline) of Zarr v3 codecs from an array's axis metadata. It captures
correlation along the z, time, and channel axes *explicitly* — as ordinary,
independently specified array-to-array and array-to-bytes codecs — then stores
the result with a fast entropy backend, High-Throughput JPEG 2000 (ISO/IEC
15444-15) coefficient planes, or ZFP blocks. It is a Rust core with Python and
TypeScript bindings, built for OME-Zarr / OME-NGFF.

:::{admonition} Early-stage project
:class: warning

nd-image-codecs is at version **0.0.1**, a name-reservation release. The
`codec_series` builder is fully implemented and validated across all three
language implementations; the **codec encode/decode paths are still scaffolds**
and land across the six [roadmap phases](development/roadmap/index.md). See
[publishing](development/publishing.md) for the accurate current status of
every published artifact.
:::

## The three codec families

Three families trade off ratio, speed, and access pattern:

| Family | Series (pipeline) | Built for |
| --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bitshuffle → zstd/lz4` | Fast lossless storage from **existing** Zarr codecs only |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | Scalable microscopy & volume visualization (resolution pyramids, thumbnails) |
| **nd-zfp** | `transpose → nd_zfp` | GPU volume rendering, random access, predictable (fixed-rate) memory |

Each family is produced by [`codec_series`](architecture/codec-series.md), which
chooses a transpose order and decorrelation axes from the axis names (`t`, `c`,
`z`, `y`, `x`, …) and the chunk shape — all overridable.

## The IP posture

nd-image-codecs deliberately **avoids JPEG 2000 Part 2 (the Multiple Component
Transformation, MCT)**. Cross-axis decorrelation is instead expressed as
`nd_lift`, an explicit, independently specified Zarr array-to-array codec, so the
transform runs first and ordinary 2D coding compresses the resulting planes. The
`htj2k` codec emits only conforming JPEG 2000 **Part 1** (T.800) and **Part 15 /
HTJ2K** (T.814) syntax, and `nd_zfp` is a clean-room port of
[LLNL ZFP](https://github.com/LLNL/zfp). This keeps the whole system clear of
Part 2 MCT patent concerns while still capturing the correlation that makes
scientific volumes compressible.

## Where to go next

| Section | What you'll find |
| --- | --- |
| [Architecture](architecture/index.md) | The design: the codec-series builder, the `nd_lift` transform, the HTJ2K plane codec and block coder, the ZFP port, codestream syntax, and range access |
| [Usage](usage/index.md) | Task-oriented guides for Zarr/OME-Zarr, the `ndic` CLI, Rust, Python, TypeScript, and thumbnails/streaming |
| [Development](development/index.md) | Everyday commands, benchmarking, test data, publishing, commit format, and Rust style |
| [Roadmap](development/roadmap/index.md) | The six implementation phases, in strict order, with acceptance criteria |

New to the project? Read the [architecture overview](architecture/overview.md)
for the mental model, then the [usage guide](usage/index.md) matching your
ecosystem.
