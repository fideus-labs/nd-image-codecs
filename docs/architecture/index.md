---
title: Architecture
description: 'The top-level map of the nd-image-codecs architecture documentation: start with the overview and goals, then open the topic section that matches the code you are working on.'
---

# Architecture

:::{note} Document status
**Version:** 0.1 · **Status:** Draft
:::

This index is the top-level map of the nd-image-codecs architecture documentation. Start with
the overview and goals, then open the topic section that matches the code you are
working on. Every phase document in the
[roadmap](../development/roadmap/index.md) links back into these topics.

## Start here

| Document | What it covers |
| --- | --- |
| [Overview](./overview.md) | The whole design in one page: axis metadata → codec-series builder → the three families (nd-delta, nd-lift-ht, nd-zfp) |
| [Goals & Non-Goals](./goals.md) | Design goals and explicit non-goals |

## Topic sections

| Section | What's inside | Open |
| --- | --- | --- |
| Codec series | The builder: axis roles, transpose rules, decorrelation-axis defaults and overrides, the three families' pipelines, JSON output | [Codec Series](./codec-series.md) |
| nd_lift transform | The explicit array-to-array cross-axis transform: lifting math (delta / haar / 5/3), axis roles, boundary handling, versioning | [nd_lift Transform](./nd-transform.md) |
| HT block coder | FBCOT cleanup/SigProp/MagRef passes, MEL + VLC + MagSgn sub-streams, HT Sets, SIMD strategy | [HT Block Coder](./ht-block-coder.md) |
| Wavelet transform | Reversible 5/3 and irreversible 9/7 lifting, 2D in-plane geometry, boundary extension, fixed-point choices | [Wavelet Transform](./wavelet-transform.md) |
| ZFP codec | The clean-room Rust ZFP port: 2D/3D/4D blocks, the four modes, brick index, upstream parity strategy | [nd_zfp Codec](./zfp.md) |
| Codestream | Part 1 / Part 15 marker segments (`SIZ`/`COD`/`CAP`/`QCD`/`TLM`/`PLT`), progression orders, tile/precinct/packet anatomy, `.jph` boxes | [Codestream Syntax](./codestream.md) |
| Range access | How RPCL + `TLM`/`PLT` and the coefficient-plane index yield a byte-offset index; thumbnail fetch plans over HTTP Range | [Byte-Range Access](./range-access.md) |
| Zarr codecs | The Zarr v3 codec model, the three registered/composed codecs, `zarrs`/numcodecs/numcodecs.js integration, OME-Zarr fit | [Zarr Codecs](./zarr-codec.md) |

## The IP posture

nd-image-codecs deliberately **avoids JPEG 2000 Part 2 (the Multiple Component
Transformation, MCT)**. Cross-axis (z, time, channel) decorrelation is instead
expressed as an explicit, independently specified Zarr array-to-array codec,
`nd_lift`. The `htj2k` codec emits only JPEG 2000 **Part 1** (T.800) and
**Part 15 / HTJ2K** (T.814) syntax; the `nd_zfp` codec is a clean-room port of
LLNL ZFP. This keeps the whole system clear of Part 2 MCT patent/IP concerns
while still capturing the spatial correlation that makes scientific volumes
compressible.

## Authoritative external references

- ISO/IEC 15444-15 / ITU-T T.814 — High-Throughput JPEG 2000: <https://www.iso.org/standard/78321.html>, <https://www.itu.int/rec/T-REC-T.814>
- ISO/IEC 15444-1 / ITU-T T.800 — JPEG 2000 core coding system: <https://www.itu.int/rec/T-REC-T.800>
- JPEG HTJ2K white paper: <https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf>
- Taubman et al., "High throughput block coding in the HTJ2K compression standard" (ICIP 2019): <https://kakadusoftware.com/wp-content/uploads/icip2019.pdf>
- Taubman et al., "High Throughput JPEG 2000 (HTJ2K): Algorithm, Performance and Potential" (Frontiers in Signal Processing, 2022): <https://www.frontiersin.org/articles/10.3389/frsip.2022.885644/full>
- OpenJPH — the C++ HTJ2K reference this project re-imagines in Rust: <https://github.com/aous72/OpenJPH>
- LLNL ZFP — the C++ ZFP reference `ndic-zfp` ports: <https://github.com/LLNL/zfp>, <https://computing.llnl.gov/projects/zfp>
- Zarr v3 core specification: <https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html>
- Zarr extension / codec naming (ZEP 2): <https://zarr.dev/zeps/accepted/ZEP0002.html>
- numcodecs (delta, blosc, bitshuffle): <https://numcodecs.readthedocs.io>
- OME-NGFF (OME-Zarr) specification: <https://ngff.openmicroscopy.org/latest/>
