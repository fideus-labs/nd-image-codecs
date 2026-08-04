---
title: The zfp Codec (ZFP over zfp-rs)
short_title: zfp Codec
description: The zfp codec wraps the pure-Rust zfp-rs implementation of LLNL ZFP for 1D–4D blocks, implementing the zarr-extensions-registered zfp array-to-bytes codec for GPU volume rendering and O(1) random brick access.
---

**Crate:** [`ndic-zfp`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-zfp)

The `zfp` codec is [LLNL ZFP](https://github.com/LLNL/zfp) for **1D–4D** chunks,
registered as a Zarr v3 **array-to-bytes** codec. It targets GPU volume
rendering, random brick access, and predictable memory: ZFP's fixed-rate
mode gives every `4^d` block a constant bit budget, so a renderer can
compute a brick's byte address in O(1) and bound its working set exactly.

The block transform and coder are **not** maintained in this repository:
`ndic-zfp` delegates to [`zfp-rs`](https://crates.io/crates/zfp-rs), a
pure-Rust ground-up implementation that produces bit-for-bit identical
streams to the reference C library on little-endian targets — and proves it
in its own CI against the upstream test suite's checksums and `zfp-sys`.
What `ndic-zfp` adds on top is the Zarr chunk semantics (dimension
squeezing, narrow-integer promotion), the computed brick index, and the
codec/binding surface.

## Why ZFP as its own codec

ZFP is the standard for lossy-but-bounded floating-point scientific arrays
and is what GPU renderers and simulation post-processing already expect
([ZFP project](https://computing.llnl.gov/projects/zfp)). Exposing it as a
Zarr codec lets nd-zfp series feed those consumers directly, while the
fixed-rate mode's random access complements the streaming/pyramid strengths
of nd-lift-ht.

## Stream format

Chunks are **standard ZFP streams**: the full ZFP header (32-bit magic,
52-bit field metadata, 12- or 64-bit compression mode) followed by the
compressed blocks, padded to a 64-bit word. This is the byte layout
`zfp -h`, `zfpy`, and `imagecodecs`' numcodecs ZFP produce, so this codec
chunks cross-decode with those implementations where modes overlap — the
Python test suite asserts byte-identical output against `imagecodecs`
(the C library via FFI) and cross-decodes in both directions. On decode the
header must declare exactly the shape, scalar type, and mode the array
metadata implies; anything else is refused as malformed.

## Modes

| Mode | `ZfpMode` | Guarantee |
| --- | --- | --- |
| Fixed-rate | `FixedRate(bits_per_value)` | Constant bits/block → O(1) random access, bounded memory (primary GPU mode) |
| Fixed-accuracy | `FixedAccuracy(tol)` | Absolute error ≤ `tol` (float data), variable size |
| Fixed-precision | `FixedPrecision(planes)` | Fixed number of bit planes retained |
| Reversible | `Reversible` | Bit-for-bit lossless |

Native sample types: `f32`, `f64`, `i32`, `i64`. The chunk layer promotes
`u8`/`i8`/`u16`/`i16` into `i32` exactly as the C library's
`zfp_promote_*` helpers do (shift into the high-order bits, biasing
unsigned types), so lossy budgets track the samples' actual range and
reversible mode round-trips bit-exactly
([ZFP modes](https://zfp.readthedocs.io/en/release0.5.4/modes.html)).

## Chunk semantics

The chunk shape maps **directly** onto the ZFP field, 1–4 dimensions,
exactly as the [registered `zfp`
codec](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/zfp)
specifies. Singleton dimensions (a size-1 channel, an unchunked `t`) are
collapsed by a `reshape` codec the series builder places upstream — so the
`transpose → reshape → zfp` series moves them out of ZFP's way instead of
wasting `4^d` block volume on them, and the `zfp` stage itself stays
exactly what any other implementation of the registered codec expects.

A configuration carrying the **legacy `dims` member** — data written under
the deprecated `nd_zfp` name, before the registered name was adopted —
selects the old in-codec mapping instead: singleton axes squeezed away,
the remainder left-padded with size-1 axes up to `dims`. The codec answers
to both names, so existing stores keep decoding byte-for-byte.

## Brick index

In fixed-rate mode block *k* spans exactly
`header_bits + k · bits_per_block … + bits_per_block` bits — computed
addressing, no stored table. `BrickIndex` exposes the offsets (bit- and
byte-granular) and `decompress_brick` decodes one `4^d` brick without
touching the rest of the payload; the codec's partial decoder uses the same
arithmetic to fetch only the byte ranges the requested
[`ArraySubset`](https://docs.rs/zarrs/latest/zarrs/array/struct.ArraySubset.html)
touches — `RangeIndex`-style random access (see
[byte-range access](./range-access.md)). The variable-size modes decode the
whole chunk and slice; an explicit byte-range table for them is future
work.

## Configuration

```json
{ "name": "zfp", "configuration": { "mode": "fixed_rate", "rate": 8.0 } }
```

`mode` is one of `reversible`, `fixed_rate`, `fixed_accuracy`,
`fixed_precision`, with the corresponding parameter
(`rate`/`tolerance`/`precision`); exactly the mode's own parameter may be
present, and the schema is the registered one (a vendored copy sits at
`spec/vendor/zfp.schema.json` and CI validates every builder-emitted
configuration against it). The same configuration object is parsed by the
Rust codec, the Python `zarr_codec.NdZfpCodec`, and the TypeScript `NdZfp`
class.

## Testing

- **Checksum matrix** (`fixtures/zfp/checksums.json`): the encoded stream's
  checksum across dims × dtype × mode, reproduced bit-exactly, plus a
  pinned chunk fixture re-encoded byte-for-byte from Rust and Python.
- **`imagecodecs` differential**: same input/params ⇒ byte-identical
  streams to the C library; cross-decode both directions. (Upstream
  C-checksum parity is additionally carried by `zfp-rs`'s own test suite.)
- Round-trip identity in reversible mode (proptest, all supported dtypes,
  1D–4D, Rust + zarrs + zarr-python + WASM).
- Fixed-rate random access: decode brick *k* directly and compare to the
  full decode, including clipped edge bricks (proptest); brick-selective
  sub-chunk reads through the zarrs partial decoder equal the full decode.
- Malformed streams (empty, garbage, truncated, padded, mismatched
  header) error cleanly, never panic.
