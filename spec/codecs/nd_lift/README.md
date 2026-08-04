# `nd_lift` codec

Defines an `array -> array` codec that decorrelates a chunk along one or more
of its dimensions with reversible integer lifting, widening the samples into
an `int32` or `int64` coefficient plane.

Cross-axis correlation in ND scientific images — successive z slices of a
volume, successive timepoints of a series, related channels — is the largest
compressible redundancy that a 2D image codec placed downstream cannot reach.
This codec exposes that decorrelation as an **explicit, self-describing Zarr
codec** rather than burying it in a downstream codec's syntax, so the transform
that was applied is readable from array metadata and reproducible by any
implementation.

## Codec name

The value of the `name` member in the codec object MUST be `nd_lift`.

## Configuration parameters

- `version` (string, **required**, no default). The transform-semantics
  version this configuration is written against. The only defined value is
  `"0.1"`. A decoder that does not implement the stored version MUST refuse
  the array rather than decode it with different semantics.

  There is deliberately no default: a stored configuration states its own
  semantics, so a chunk written by an older writer cannot be silently
  reinterpreted by a newer reader.

- `transforms` (array of transform objects, **required**). Applied in listed
  order on encode; the inverse of each is applied in reverse order on decode.
  An empty array is valid and means the identity (the codec then only widens
  the data type).

Each transform object has:

| Member | Type | Default | Meaning |
| --- | --- | --- | --- |
| `dimension` | unsigned integer | — (required) | Index into the **chunk** shape as this codec sees it. When a `transpose` codec precedes `nd_lift`, that is the post-transpose index. |
| `kind` | string | — (required) | `"delta"`, `"haar"`, or `"lift53"`. |
| `levels` | unsigned integer (0–255) | `0` | Dyadic decomposition levels. MUST be ≥ 1 for `"haar"` and `"lift53"`; ignored for `"delta"`. |
| `axis` | string | `""` | A human-readable axis name (`"z"`, `"t"`, …). Informational only — decoders MUST use `dimension`. |
| `group` | unsigned integer (32-bit) | `0` | Transform length along the axis. `0` means the whole chunk extent. A non-zero `group` partitions the axis into independent runs of that length (the final run may be shorter), bounding how far a corrupt sample propagates and letting a reader reconstruct one group without the others. |

Unknown members MUST be rejected.

## Example

The array metadata below decorrelates dimension 1 (a `z` axis, after a
`transpose`) with two levels of 5/3 lifting, then codes the resulting
coefficient planes with a downstream `array -> bytes` codec:

```json
{
  "codecs": [
    { "name": "transpose", "configuration": { "order": [1, 0, 2, 3] } },
    {
      "name": "nd_lift",
      "configuration": {
        "version": "0.1",
        "transforms": [
          { "axis": "z", "dimension": 1, "kind": "lift53", "levels": 2, "group": 0 }
        ]
      }
    },
    { "name": "htj2k", "configuration": { "xy_levels": 5 } }
  ]
}
```

## Supported data types

`uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`, `uint64`, `int64`.

The codec **changes the data type** of the array representation it passes
downstream: every input type of at most 32 bits encodes to `int32`, and the
64-bit types encode to `int64`. Downstream codecs therefore see a signed
coefficient plane, not the array's declared type. This widening is what gives
the transform room to grow: a reversible lifting step's output needs more bits
than its input.

Floating-point data types are not supported; the transform is integer-exact by
construction.

## Supported chunk shapes

Any chunk shape of one or more dimensions. Each transform's `dimension` MUST
be a valid index into the chunk shape. A transform along an axis of extent 1
is a no-op.

The transform never crosses chunk boundaries: chunks remain independently
decodable, which is what keeps random access and partial reads intact.

## Format and algorithm

All arithmetic below is on the widened coefficient plane (`int32` or `int64`).
Every kernel is reversible over the integers — the decoder recovers the input
exactly, with no floating-point step anywhere.

Let `x[0..n]` be the samples along the transform axis within one group, taken
with the chunk's other indices fixed.

### `delta`

```text
encode:  y[0] = x[0];  y[i] = x[i] - x[i-1]      for i = 1..n-1
decode:  x[0] = y[0];  x[i] = y[i] + x[i-1]      for i = 1..n-1
```

`levels` is ignored.

### `haar`

One level pairs neighbours; `levels` levels recurse on the low-pass half.
With `l[i]` the low-pass and `h[i]` the high-pass output of one level:

```text
encode:  h[i] = x[2i+1] - x[2i]
         l[i] = x[2i] + floor(h[i] / 2)
decode:  x[2i]   = l[i] - floor(h[i] / 2)
         x[2i+1] = h[i] + x[2i]
```

An odd-length run leaves its final sample in the low-pass band untouched.
Output is deinterleaved: the low-pass band occupies the first `ceil(n/2)`
positions, the high-pass band the rest.

### `lift53`

The reversible 5/3 lifting steps of the JPEG 2000 wavelet (ITU-T T.800 Annex
F), applied along the axis. With `x` extended by whole-sample symmetric
(mirror) extension at both ends:

```text
predict: h[i] = x[2i+1] - floor((x[2i] + x[2i+2]) / 2)
update:  l[i] = x[2i]   + floor((h[i-1] + h[i] + 2) / 4)
```

Decode applies the inverse update, then the inverse predict. As with `haar`,
one level's output is deinterleaved into low-pass then high-pass, and `levels`
levels recurse on the low-pass band.

`floor` denotes arithmetic right shift (floor division by a power of two),
which is what makes the kernels exactly invertible on negative values.

### Overflow

An encoder MUST verify, before transforming, that the transform cannot leave
the coefficient plane's range: propagate the input's actual value range
through every step, including the lifting intermediates, and refuse the chunk
if any value could exceed the plane. In practice this refuses only
near-full-width `uint32`/`uint64` input.

A decoder MUST NOT apply the same check: the inverse kernels are total. A
corrupt or hostile chunk can contain any in-range coefficient, and the inverse
must produce *some* value for it rather than fail unpredictably — implementations
SHOULD use wrapping arithmetic so no input can trap. The result of decoding
corrupt input is meaningless, which is all corrupt input can be; a reader that
must *detect* corruption rather than survive it should compose a checksum
codec (`crc32c`) below this one. Decoders MUST reject coefficients that do not
narrow back into the array's data type.

## Interoperability and compatibility

The transform is specified over integers with explicit rounding, so
independent implementations produce **bit-identical** coefficient planes.
Conformance vectors (input, configuration, expected output for both
directions) accompany the reference implementation.

This codec is deliberately independent of any downstream codec. It composes
with `bytes` + a byte compressor, or with an image codec such as `htj2k`, and
carries no assumption about which. In particular it is **not** JPEG 2000 Part 2
multi-component transform (MCT) syntax: the decorrelation is a Zarr codec, in
Zarr metadata, applied before any codestream exists.

## Changelog

- **0.1** — initial version.

## Maintainers

- [Fideus Labs LLC](https://github.com/fideus-labs) —
  [nd-image-codecs](https://github.com/fideus-labs/nd-image-codecs)
