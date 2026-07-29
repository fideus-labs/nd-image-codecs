## The `nd_lift` Cross-Axis Transform

> Crate: [`ndic-lift`](../../crates/ndic-lift/) · Roadmap:
> [Phase 2](../development/roadmap/phase-2-nd-lift.md) · Codec version: `0.1`

`nd_lift` is the codec that lets nd-image-codecs capture correlation along the
non-spatial axes (z, time, channel) of a scientific volume **without JPEG 2000
Part 2**. It is a registered Zarr v3 **array-to-array** codec: it runs first in
a series, decorrelating chosen axes in place, and hands the transformed array to
an ordinary array-to-bytes codec (`htj2k`) that compresses the trailing 2D
`(y, x)` planes.

### Why an explicit codec instead of MCT

A Part 2 Multiple Component Transformation can express a wavelet-across-slices,
but it does so inside JPEG 2000 codestream syntax (`MCT`/`MCC`/`MCO` markers)
that carries patent and tooling baggage and is unevenly supported. `nd_lift`
achieves the same *effect* — decorrelate z/t/c before 2D coding — as a
standalone, fully specified Zarr codec built from long-published lifting and
differencing primitives. The 2D codec downstream then only ever sees Part 1 /
Part 15 syntax. This is the project's central IP posture (see
[goals.md](./goals.md)).

### Configuration

```json
{
  "name": "nd_lift",
  "configuration": {
    "version": "0.1",
    "transforms": [
      { "axis": "z", "dimension": 2, "kind": "lift53", "levels": 2, "group": 0 },
      { "axis": "t", "dimension": 1, "kind": "lift53", "levels": 2, "group": 0 }
    ]
  }
}
```

Each entry in `transforms` is one 1D transform applied along one axis:

| Field | Meaning |
| --- | --- |
| `axis` | Human-readable axis name (`"z"`, `"t"`, `"c"`, …); informational. |
| `dimension` | The axis's index into the **post-transpose** chunk shape — this is what the decoder uses. |
| `kind` | `delta`, `haar`, or `lift53` (9/7 float lifting is the Phase 2 lossy extension). |
| `levels` | Dyadic decomposition levels for lifting kinds; ignored for `delta`. |
| `group` | Group length along the axis; `0` = the whole chunk extent. Bounds decode amplification and working memory. |

Transforms are applied in listed order on encode and in reverse on decode.

### Transform kinds

| Kind | Rule | Reversible | Notes |
| --- | --- | --- | --- |
| `delta` | `r[i] = x[i] − x[i−1]`, `r[0] = x[0]` | Yes (integers) | Fastest; single lifting step; longest dependency chain, bounded by `group`. |
| `haar` | Reversible integer Haar lifting: `d = x₁ − x₀`, `s = x₀ + ⌊d/2⌋` | Yes (integers) | Compact support (2 samples); good for short axes. |
| `lift53` | Le Gall 5/3 integer lifting (predict + update), T.800 rounding | Yes (integers) | Better smooth-signal decorrelation; needs symmetric boundary handling. |
| `lift97` *(Phase 2)* | CDF 9/7 float lifting + per-band quantization | No | Lossy; higher ratio; pairs with lossy `htj2k`. |

### Lifting math (5/3)

For an axis signal `x[0…n−1]`, one 5/3 level computes odd (detail) then even
(approx) samples with integer rounding:

```text
d[i] = x[2i+1] − ⌊(x[2i] + x[2i+2] + 1) / 2⌋        (predict)
s[i] = x[2i]   + ⌊(d[i−1] + d[i] + 2) / 4⌋           (update)
```

Multiple `levels` recurse on the approximation band `s`. The `haar` kernel is
the degenerate 2-tap case. All integer kinds are exactly invertible for every
supported integer dtype.

### Boundary handling

- **Symmetric (mirror) extension** at group and chunk boundaries, matching
  T.800 Annex F, so the transform is well-defined at the edges without storing
  extra samples.
- **Odd axis lengths** are supported: the final sample of an odd-length group is
  handled by the standard whole-sample-symmetric extension; no padding is
  written.
- **Singleton axes** (extent 1) are a no-op — the builder never places a
  transform on a size-1 axis, but the codec tolerates one defensively.

### Overflow and precision

- Integer coefficients live in `i32` planes (`ndic_core::CoeffPlane`). For
  16-bit input, 5/3 growth over a handful of levels stays well within `i32`; the
  codec asserts the per-axis bit-growth budget on encode.
- 64-bit integer input is transformed in `i64` and range-checked.
- The lossy `lift97` path documents its Q-format per lifting step; overflow
  behavior is checked by proptest with extreme-value inputs.

### Grouping and chunk independence

A transform's `group` bounds how many samples along the axis are coupled. The
codec-series builder sets grouping so coupling never crosses a Zarr chunk
boundary — chunk independence is what keeps Zarr's parallel read/write model
intact. A `group` of 0 means "the whole chunk extent along this axis", which is
the common case for a chunk that holds a bounded z/t block.

### Versioning

The `version` field pins the transform semantics. `0.1` is the initial integer
lifting spec (`delta`/`haar`/`lift53`). Any change to predictor, update,
rounding, boundary rule, or coefficient ordering bumps the version; decoders
refuse unknown major versions rather than silently mis-decoding.

### Testing

- Round-trip identity on `delta`/`haar`/`lift53` for random volumes and every
  integer dtype (proptest).
- Analytic vectors: impulse, ramp, DC against closed-form band values.
- Boundary/odd-length/singleton edge cases enumerated explicitly.
- Cross-validation: an `nd_lift`-then-`htj2k` volume decoded through the Python
  binding and checked against a NumPy reference implementation of the same
  lifting math (Phase 6).
