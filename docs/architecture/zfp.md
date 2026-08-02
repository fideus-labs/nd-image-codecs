---
title: The nd_zfp Codec (Rust ZFP port)
short_title: nd_zfp Codec
description: nd_zfp is a clean-room Rust port of LLNL ZFP for 2D, 3D, and 4D blocks, registered as a Zarr v3 array-to-bytes codec for GPU volume rendering and random brick access.
---

**Crate:** [`ndic-zfp`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-zfp) · **Roadmap:**
[Phase 5](../development/roadmap/phase-5-nd-zfp.md)

`nd_zfp` is a clean-room Rust port of [LLNL ZFP](https://github.com/LLNL/zfp)
for **2D, 3D, and 4D** blocks, registered as a Zarr v3 **array-to-bytes** codec.
It targets GPU volume rendering, random brick access, and predictable memory:
ZFP's fixed-rate mode gives every `4^d` block a constant bit budget, so a
renderer can compute a brick's byte address in O(1) and bound its working set
exactly.

## Why ZFP as its own codec

ZFP is the standard for lossy-but-bounded floating-point scientific arrays and
is what GPU renderers and simulation post-processing already expect
([ZFP project](https://computing.llnl.gov/projects/zfp)). Exposing it as a Zarr
codec lets nd-zfp series feed those consumers directly, while the fixed-rate
mode's random access complements the streaming/pyramid strengths of nd-lift-ht.

## ZFP block algorithm (what the port reproduces)

For each `4^d` block (d = 2, 3, 4), the reference algorithm:

1. **Align** the block's values to a common exponent (for floats) → block-float
   representation.
2. **Decorrelating transform** — a fixed, reversible, near-orthogonal lifting
   transform applied along each of the d axes of the block.
3. **Reorder** coefficients by total sequency (a d-dimensional generalization of
   zig-zag ordering).
4. **Embedded coding** — negabinary conversion followed by bit-plane group
   testing, emitting bits most-significant-plane first so the stream is
   truncatable.

The four modes control where truncation happens:

| Mode | `ZfpMode` | Guarantee |
| --- | --- | --- |
| Fixed-rate | `FixedRate(bits_per_value)` | Constant bits/block → O(1) random access, bounded memory (primary GPU mode) |
| Fixed-accuracy | `FixedAccuracy(tol)` | Absolute error ≤ `tol`, variable size |
| Fixed-precision | `FixedPrecision(planes)` | Fixed number of bit planes retained |
| Reversible | `Reversible` | Bit-for-bit lossless |

Supported sample types: `f32`, `f64`, `i32`, `i64`; narrower integers are
promoted, matching the C library's guidance
([ZFP modes](https://zfp.readthedocs.io/en/release0.5.4/modes.html)).

## Compatibility contract

The port is **bitstream-compatible** with the reference C implementation:
compressed output must be byte-identical for identical parameters. This is
verified two ways:

- **Upstream checksums.** ZFP ships a test suite with per-configuration
  checksums over compressed output and round-tripped data. The port reproduces
  those vectors (dimension × type × mode × rate matrix); the committed
  checksums live in `docs/development/test-data.md`.
- **FFI reference lane.** A `zfp-sys` (bindgen to the C library) lane is the
  ground truth until pure-Rust parity is proven in CI: differential tests
  compress the same inputs with both implementations and assert byte equality,
  and cross-decode (Rust-encode → C-decode and vice-versa).

## Port strategy

1. **2D first**, scalar and correct, checksum-matched — the conformance oracle.
2. **3D then 4D**, sharing the generic d-dimensional transform and coder.
3. **SIMD lanes** for the block transform and bit-plane coder (AVX2/SSE4.1/NEON,
   WASM128), differential-tested bit-identical against the scalar path.
4. **Brick index** — an outer table mapping each `4^d` brick to its byte range;
   trivial and index-free in fixed-rate mode, explicit in the variable-size
   modes — enabling `RangeIndex`-style random access (see
   [byte-range access](./range-access.md)).

## Configuration

```json
{ "name": "nd_zfp", "configuration": { "mode": "fixed_rate", "rate": 8.0, "dims": 3 } }
```

`dims` is set by the codec-series builder to the number of non-singleton chunk
dimensions (2–4). `mode` is one of `reversible`, `fixed_rate`,
`fixed_accuracy`, `fixed_precision`, with the corresponding parameter
(`rate`/`tolerance`/`precision`).

## Testing

- Upstream checksum reproduction across the full dimension/type/mode matrix.
- `zfp-sys` differential + cross-decode equality.
- Round-trip identity in reversible mode (proptest, all supported dtypes).
- Fixed-rate random-access: decode brick *k* directly and compare to a full
  decode.
