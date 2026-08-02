---
title: HT Block Coder (FBCOT)
description: The FBCOT block coder of ISO/IEC 15444-15 replaces Part 1's EBCOT Tier-1 MQ arithmetic coder, trading roughly 5-10 % compression efficiency for an order-of-magnitude speedup.
---

> Crate: [`ndic-htj2k`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-htj2k) · Roadmap:
> [Phase 3](../development/roadmap/phase-3-htj2k-core.md)

nd-image-codecs implements the **FBCOT** (Fast Block Coding with Optimized Truncation) algorithm
of ISO/IEC 15444-15 / [ITU-T T.814](https://www.itu.int/rec/T-REC-T.814). FBCOT replaces
the Part 1 EBCOT Tier-1 MQ arithmetic coder — the dominant cost in classic JPEG 2000 —
while leaving every other JPEG 2000 mechanism (wavelets, quantization, packets,
progression) untouched. Published results report roughly **10× faster lossy** and **30×+
faster lossless** coding at a small (~5–10 %) compression-efficiency cost
([JPEG HTJ2K white paper](https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf),
[Taubman et al., ICIP 2019](https://kakadusoftware.com/wp-content/uploads/icip2019.pdf)).

## Pass structure

Classic EBCOT codes every bit-plane with three passes (SigProp, MagRef, Cleanup), all
arithmetic-coded and all needed for reconstruction. HT restructures this:

- The **HT Cleanup pass** is *self-contained*: one Cleanup pass fully encodes all sample
  magnitudes/signs down to some bit-plane `p`. It is not a refinement of earlier passes.
- The **HT SigProp** and **HT MagRef** passes optionally refine one further bit-plane
  below the Cleanup pass, using raw (uncoded) bits in HT's bit ordering.
- A decoder therefore touches **at most 3 passes** per code-block, regardless of bit
  depth; an encoder that wants rate-control flexibility can emit multiple **HT Sets**
  (each Set = Cleanup [+ SigProp] [+ MagRef], up to 2 Sets / 6 passes signaled per block)
  and discard whole Sets at assembly time — the "Optimized Truncation" in FBCOT
  ([Taubman et al., Frontiers 2022](https://www.frontiersin.org/articles/10.3389/frsip.2022.885644/full)).

## Cleanup-pass sub-streams

The Cleanup pass concurrently emits three byte-aligned sub-bitstreams within one
codeword segment:

| Stream | Contents | Coding tool |
| --- | --- | --- |
| **MagSgn** | Magnitude bits + sign for significant samples | Raw bits, per-sample counts bounded by UVLC exponents |
| **MEL** | Adaptive run-length coding of significance events | 13-state MEL coder (adapted from the JPEG-LS MELCODE) |
| **VLC** | Per-quad significance patterns + exponent residuals | Context-dependent variable-length codes (CxtVLC, codewords ≤ 7 bits) + U-VLC |

MagSgn grows forward from the start of the segment; the MEL and VLC streams share the
remaining bytes, growing toward each other, with an interface word at the segment tail
locating the boundary. Samples are processed in **2×2 quads** (two quad-rows at a time);
per-quad significance patterns and magnitude exponents come from small lookup tables
indexed by causal neighbor context. There is **no arithmetic-coder feedback loop in the
sample path**, which is what makes the whole pass table-driven, branch-light, and
vectorizable ([T.814](https://www.itu.int/rec/T-REC-T.814)).

## Implementation strategy (mirroring, then extending, OpenJPH)

OpenJPH's coding layer (`src/core/coding/ojph_block_encoder*.cpp`,
`ojph_block_decoder*.cpp` in [aous72/OpenJPH](https://github.com/aous72/OpenJPH)) ships
per-ISA translation units — generic, SSE, SSE2, SSSE3, SSE4, AVX2, AVX-512, WASM SIMD —
selected at runtime by CPU detection (`ojph_arch.cpp`). nd-image-codecs mirrors this:

- One **scalar reference implementation** first — the conformance oracle, `no_std`-clean,
  used by proptest round-trip suites and differential tests.
- **SIMD lanes** in sibling modules gated by `target_arch`/`target_feature`, selected at
  runtime via `is_x86_feature_detected!` / `is_aarch64_feature_detected!`; WASM builds
  use `simd128` (enabled workspace-wide in `.cargo/config.toml`). NEON is a first-class
  lane here — in OpenJPH it is still a placeholder.
- **`rayon` across code-blocks**: each 64×64 block is independent by construction, so
  encode/decode parallelism is embarrassing; OpenJPH is single-threaded by design and
  leaves this on the table.
- Bit-plane count per block bounds: HT supports up to **38 bit-planes** conceptually but
  typical microscopy / volume targets are ≤ 16-bit integers plus wavelet gain; `MAGB` in the `CAP` marker
  carries the bound (see [codestream.md](./codestream.md)).

## What we don't build

- No MQ **encoder** (legacy J2K-1 output is a non-goal; see [goals.md](./goals.md)).
- No iterative PCRD-opt over many coding passes: rate control starts quantizer-driven
  like OpenJPH, with HT-Set truncation as the later refinement mechanism.

## Test surface

- Round-trip property tests: random planes × dtypes × block sizes, encode∘decode
  identity on the 5/3 path.
- Conformance decode against the OpenJPH test corpus
  ([aous72/jp2k_test_codestreams](https://github.com/aous72/jp2k_test_codestreams)) — see
  [test-data.md](../development/test-data.md).
- Differential fuzzing scalar-vs-SIMD lanes (must be bit-identical).
