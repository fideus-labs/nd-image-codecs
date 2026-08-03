---
title: Phase 5 — The nd_zfp Codec (ZFP over zfp-rs)
short_title: Phase 5 — nd_zfp
description: Phase 5 delivers the nd_zfp codec — pure-Rust LLNL ZFP for 1D–4D data, bit-identical to the C implementation — plus the brick index and the Zarr codec wrapper.
---

# Phase 5 — The `nd_zfp` Codec (ZFP over zfp-rs)

**Depends on:** Phase 1 · **Gates:** Phase 6 · **Architecture:** [nd_zfp Codec](../../architecture/zfp.md)

Phase 5 delivers pure-Rust [LLNL ZFP](https://github.com/LLNL/zfp) for
1D–4D data that **matches the C implementation byte-for-byte**, plus the
brick index and the Zarr codec wrapper. Fixed-rate mode is the priority: it is
what gives GPU volume renderers O(1) random brick access and predictable memory.

:::{note} Implementation decision
This phase was planned as a clean-room in-repo port. It shipped instead on
the [`zfp-rs`](https://crates.io/crates/zfp-rs) crate — an existing
pure-Rust ZFP that is bit-identical to the C reference on little-endian
targets and reproduces the upstream test suite's checksums against
`zfp-sys` in its own CI — so `ndic-zfp` maintains only the Zarr chunk
semantics (dimension squeezing, narrow-integer promotion), the computed
brick index, and the codec/binding surface. The `zfp-sys` FFI lane below is
therefore delegated to `zfp-rs`'s suite; our differential ground truth in
this repo is `imagecodecs` (the C library via FFI) in the Python tests.
:::

## What to build

1. **`zfp-sys` FFI lane** (dev-dependency): bindgen to the reference C library;
   the ground-truth oracle for every differential test until parity is proven.
2. **Upstream test vectors**: extract the C test suite's per-configuration
   checksums (dimension × type × mode × rate matrix) into committed fixtures
   (see [test data](../test-data.md)).
3. **2D core, scalar**: block-float alignment, the reversible decorrelating
   block transform, total-sequency reordering, negabinary + bit-plane group
   coding; fixed-rate first, then fixed-accuracy, fixed-precision, reversible.
4. **3D and 4D**: generalize transform/reorder/coder over block dimension d
   (`4^d` blocks, partial-block padding rules).
5. **Types**: `f32`/`f64`/`i32`/`i64` (+ promotion for narrower integers).
6. **Brick index**: computed addressing in fixed-rate mode; explicit byte-range
   table in variable-size modes.
7. **The `nd_zfp` Zarr codec** (`ndic-zarr` + Python entry point + TS class):
   `mode`/`rate`/`tolerance`/`precision`/`dims` config; builder already emits it.
8. **SIMD lanes** for the block transform and coder (AVX2/NEON/WASM128),
   bit-identical to scalar.

## Order of work

FFI lane + vectors → 2D fixed-rate scalar (checksum-matched) → remaining 2D
modes → 3D → 4D → integer types → brick index → Zarr wrapper → SIMD.

## Spec / reference anchors

- ZFP algorithm & modes: <https://computing.llnl.gov/projects/zfp>,
  <https://zfp.readthedocs.io/en/release0.5.4/modes.html>
- Reference sources + test suite: <https://github.com/LLNL/zfp>
- Zarr wrapping precedent: `imagecodecs` numcodecs ZFP: <https://pypi.org/project/imagecodecs/>

## Tests & benchmarks

- **Checksum reproduction**: byte-identical compressed output vs upstream
  vectors for the full matrix.
- **Differential vs `zfp-sys`**: same input/params ⇒ same bytes; cross-decode
  both directions.
- Reversible-mode round-trip proptest, all types and dims.
- Fixed-rate random access: decode brick *k* alone equals full-decode slice.
- Bench lanes: `zfp-rate8`, `zfp-reversible` vs `ref-zfp` (C via FFI) and
  `imagecodecs` ZFP — throughput and ratio.

## Acceptance criteria

- [x] Upstream checksum matrix reproduced bit-exactly (2D/3D/4D × 4 modes ×
      supported types). (Delegated to `zfp-rs`'s CI against the upstream
      suite; this repo pins its own stream matrix in
      `fixtures/zfp/checksums.json` — `crates/ndic-zfp/tests/checksums.rs` —
      and asserts byte-identity with `imagecodecs` output.)
- [x] Cross-decode with the C library succeeds in both directions.
      (`test_streams_match_the_c_reference_via_imagecodecs`: byte-identical
      streams both modes, cross-decode both directions.)
- [x] Fixed-rate bricks are individually decodable at computed offsets.
      (`BrickIndex` + `decompress_brick`, proptested against the full
      decode including clipped edge bricks; the zarrs partial decoder
      fetches and decodes only the bricks a subset touches.)
- [x] `nd_zfp` registers in zarrs/zarr-python/numcodecs.js; builder pipelines
      round-trip via `zarr-python` against `imagecodecs` ZFP where modes overlap.
      (`zfp_zarrs.rs`, `test_nd_zfp_roundtrip.py`, `nd-zfp.test.ts`.)
- [ ] Rust throughput within 1.5× of the C library on the bench corpus (scalar),
      target parity with SIMD. *Measured (dev box, arm64, 32×128×128 f32 vs
      `imagecodecs`): encode is **faster** than C (0.61× reversible, 0.70×
      fixed-rate); serial decode is 1.62–1.66× — narrowly outside the
      target. Remaining: decode-side optimization (zfp-rs also offers a
      rayon execution path for fixed-rate decode, which the C library does
      not parallelize).*
