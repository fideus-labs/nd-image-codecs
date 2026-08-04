# `nd_zfp` codec

Defines an `array -> bytes` codec that compresses chunks with the
[zfp](https://github.com/LLNL/zfp) algorithm, squeezing singleton chunk
dimensions away and declaring the resulting field dimensionality inline.

> [!IMPORTANT]
> **This codec overlaps a codec already registered in zarr-extensions.**
> Read "Relationship to the registered `zfp` codec" below before treating
> this document as a submission. The recommendation there is to align with
> `zfp` rather than register a second name; this document exists so that
> decision is made against a precise statement of the difference, and so the
> stored format `nd_zfp` writes today is specified for anyone holding data
> written by it.

## Codec name

The value of the `name` member in the codec object MUST be `nd_zfp`.

## Configuration parameters

| Member | Type | Default | Meaning |
| --- | --- | --- | --- |
| `mode` | string | `"reversible"` | One of `"reversible"`, `"fixed_rate"`, `"fixed_accuracy"`, `"fixed_precision"`. |
| `rate` | number | — | Bits per value. Required by, and permitted only in, `"fixed_rate"`. MUST be positive and finite. |
| `tolerance` | number | — | Absolute error bound. Required by, and permitted only in, `"fixed_accuracy"`. MUST be non-negative and finite. |
| `precision` | unsigned integer | — | Uncompressed bit planes per value. Required by, and permitted only in, `"fixed_precision"`. MUST be in `1..=64`. |
| `dims` | unsigned integer | `3` | The zfp field dimensionality, `1..=4`. See "Supported chunk shapes". |

Exactly the mode's own parameter may be present: supplying `rate` in
`"reversible"` mode, or `tolerance` in `"fixed_rate"` mode, is an error rather
than an ignored field. Unknown members MUST be rejected.

zfp's `"expert"` mode is not exposed.

## Example

```json
{
  "codecs": [
    {
      "name": "nd_zfp",
      "configuration": { "mode": "fixed_rate", "rate": 8.0, "dims": 3 }
    }
  ]
}
```

## Supported chunk shapes

zfp fields are 1- to 4-dimensional. A chunk of any dimensionality is accepted
provided **at most four of its dimensions are non-singleton**: singleton
dimensions (extent 1) are squeezed away, and the remaining extents map onto the
zfp field in order, left-padded to `dims` when fewer remain.

A chunk of shape `[1, 1, 32, 256, 256]` with `dims: 3` is therefore a
`[32, 256, 256]` field; `[4, 1, 3, 1, 2, 1]` with `dims: 3` is `[4, 3, 2]`.

`dims` states the field dimensionality the writer used. It is redundant with
the chunk shape for a reader that applies the same squeeze rule, and exists so
a reader can validate its interpretation instead of inferring it — a chunk
whose non-singleton count disagrees with `dims` is a mismatch worth reporting
rather than silently reshaping around.

These rules apply to the inner chunk shape when this codec is used as the
array-to-bytes codec within `sharding_indexed`.

## Supported data types

`int32`, `int64`, `float32`, `float64` natively; `uint8`, `int8`, `uint16`,
`int16` through promotion to `int32`, exactly as the zfp C library's
`zfp_promote_*` helpers do. `uint32` and `uint64` have no path: zfp's integer
types are signed, and promotion would not be lossless.

## Format and algorithm

The stored bytes are a **standard zfp stream**: the 32-bit magic, the 52-bit
field metadata (type and dimensions), the compression-mode field (12 or 64
bits), then the compressed blocks, padded to a 64-bit word boundary. Nothing
is wrapped around it and nothing is stripped from it.

### Compression

1. Squeeze singleton chunk dimensions; map the rest onto a `dims`-dimensional
   zfp field.
2. Promote narrow integer types to `int32`.
3. Open a zfp stream with the configured mode and write the header.
4. Compress the field block by block (`4^dims` blocks, partial blocks at the
   edges).

### Decompression

Reverse: parse the header, decompress the blocks, demote to the array's data
type, and reshape to the chunk shape.

### Random access in `fixed_rate` mode

In `"fixed_rate"` mode every `4^dims` block occupies exactly the same number
of bits, so a block's byte offset is *computable* from the field shape, the
rate, and the block's index — no index needs to be stored, and none is. A
reader can decode one block from a byte range. This is a property of the zfp
format itself rather than of this codec, and it is why `"fixed_rate"` is the
mode to choose for volume rendering and random brick access.

## Relationship to the registered `zfp` codec

zarr-extensions already registers [`zfp`](https://github.com/zarr-developers/zarr-extensions/tree/main/codecs/zfp),
which compresses chunks with the same library into the same stream format.
The differences are narrow:

| | registered `zfp` | `nd_zfp` |
| --- | --- | --- |
| Stream bytes | standard zfp stream | **identical** |
| Modes | `reversible`, `fixed_rate`, `fixed_accuracy`, `fixed_precision`, `expert` | the same, minus `expert` |
| Chunks > 4D | handled by a **separate** squeeze array-to-array codec upstream | squeezed by this codec, with `dims` declaring the result |
| Data types | `int32`, `uint32`, `int64`, `uint64`, `float32`, `float64` | `int32`, `int64`, `float32`, `float64` (+ narrow-int promotion); no unsigned path |

So a chunk written by `nd_zfp` is byte-identical to one written by `zfp` for
the same data and mode — the reference implementation asserts exactly that
against `imagecodecs.zfp_encode` in CI, in both directions. Only the *name and
configuration* differ, and only in how the >4D case is expressed: `nd_zfp`
folds the squeeze into the codec, while `zfp` composes with a separate one.

**Recommendation.** Registering a second name for a byte-identical format
fragments the ecosystem for a configuration convenience. The better path is to
emit the registered `zfp` name, express the squeeze as the separate
array-to-array codec `zfp`'s specification anticipates, and keep `nd_zfp` only
as a deprecated alias for reading existing data — if any exists. That is a
breaking format change to the reference implementation (it touches all three
builder implementations, the committed fixture matrix, and the benchmark
baselines), so it is a decision to take deliberately rather than a detail to
settle inside a spec document.

## Changelog

- **0.1** — initial version.

## Maintainers

- [Fideus Labs LLC](https://github.com/fideus-labs) —
  [nd-image-codecs](https://github.com/fideus-labs/nd-image-codecs)
