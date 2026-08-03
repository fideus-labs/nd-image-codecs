---
title: Phase 3 — HTJ2K Core in Rust
short_title: Phase 3 — HTJ2K Core
description: 'Phase 3 is the largest single build: a pure-Rust HTJ2K implementation modeled on OpenJPH — the FBCOT block coder, the 2D wavelet, and the codestream layer.'
---

# Phase 3 — HTJ2K Core in Rust

**Depends on:** Phase 1 · **Gates:** Phase 4 · **Architecture:** [HT Block Coder](../../architecture/ht-block-coder.md),
[Wavelet Transform](../../architecture/wavelet-transform.md),
[Codestream Syntax](../../architecture/codestream.md)

Phase 3 is the largest single build: a pure-Rust HTJ2K implementation modeled on
[OpenJPH](https://github.com/aous72/OpenJPH) — the FBCOT block coder, the 2D
DWT, and Part 1/15 codestream syntax with always-on `TLM`/`PLT` indexing.

## What to build

1. **HT block decoder first** (`ndic-htj2k`): MEL, VLC, and MagSgn segment
   decoders → cleanup pass, then SigProp/MagRef refinement. The decoder is the
   conformance oracle — OpenJPH-encoded streams supply ground truth from day one.
2. **HT block encoder**: cleanup + refinement emitters, HT Set assembly,
   `Scup`/length signaling.
3. **2D DWT** (`Reversible53`, then `Irreversible97` + quantization).
4. **Codestream writer/reader** (`ndic-codestream`): `SIZ COD QCD CAP COM TLM` /
   `SOT PLT SOD` … `EOC`; RPCL packet sequencing; `CAP`/`Ccap15` HT signaling;
   pull parser over `Read + Seek` with `TLM`/`PLT`-driven packet index.
5. **`.jph` box format** (Part 15 analogue of `.jp2`).
6. **SIMD lanes** (`ndic-simd`): AVX2/SSE4.1/NEON/WASM128 for DWT and block-coder
   hot loops, differential-tested against scalar.
7. **`ndic` CLI**: `compress`/`expand`/`inspect` working end-to-end on 2D images.

## Order of work

Decoder → encoder → codestream I/O → `.jph` → SIMD → CLI. Round-trip tests
activate as soon as both coder directions exist; OpenJPH differential tests run
throughout.

## Spec / reference anchors

- ITU-T T.814 (HTJ2K): <https://www.itu.int/rec/T-REC-T.814> / ISO/IEC 15444-15: <https://www.iso.org/standard/78321.html>
- ITU-T T.800 (Part 1): <https://www.itu.int/rec/T-REC-T.800>
- OpenJPH sources (block coder, transforms, codestream): <https://github.com/aous72/OpenJPH>
- FBCOT: Taubman et al., ICIP 2019: <https://kakadusoftware.com/wp-content/uploads/icip2019.pdf>; Frontiers 2022: <https://www.frontiersin.org/articles/10.3389/frsip.2022.885644/full>

## Tests & benchmarks

- Decode conformance against OpenJPH-encoded corpus (bit-exact samples).
- Encode → OpenJPH-decode differential (our streams decode identically).
- 5/3 round-trip proptest; 9/7 error bounds; fuzzing on the reader.
- Bench lanes: `scalar-53-ht`, `simd-53-ht` vs `ojph_compress`/`ojph_expand` and
  `imagecodecs` JPEG 2000 (throughput + ratio).

## Acceptance criteria

- [x] Bit-exact decode of the OpenJPH conformance corpus — every non-tiled,
  non-subsampled reversible-5/3 stream (`corpus_conformance.rs`; incl. RGB
  with RCT, multi-precinct grids, 16-bit, 4×4/1024×4 code-blocks).
  *Remaining:* multi-tile (needs general-parity DWT), 4:2:0 YUV
  (subsampling), and the 9/7 decode path.
- [x] Lossless 5/3 round-trip on `u8`/`i8`/`u16`/`i16`. *Caveat:* 32-bit
  dtypes exceed the scalar coder's 32-bit datapath (`K_max > 30`) and are
  rejected with a clear error — same bound as OpenJPH's 32-bit path; a
  64-bit datapath is future work.
- [x] Our encoded streams decode bit-exactly under OpenJPH
  (`openjph_interop.rs`); block-level: byte-identical segments and
  bit-exact decode against `ojph_{en,de}code_codeblock32` on 1000 random
  blocks (`scripts/ht-differential.sh`). Kakadu demo tools were not
  available in the development environment.
- [x] `TLM`/`PLT` always emitted; `Codestream::packet_index` reconstructs
  every packet span from the markers alone and is validated against the
  walked offsets.
- [x] SIMD lanes bit-identical to scalar (differential-tested); 6.8× scalar
  DWT throughput measured on NEON (2048² five-level forward). The AVX2 lane
  shares the row-oriented structure and compiles for x86-64; the ≥2×
  measurement on AVX2 hardware runs in CI.
- [x] `ndic compress/expand/inspect` work on 2D PGM/PPM/PNG/raw inputs
  (plus partial-resolution expansion and `.jph` boxing).
