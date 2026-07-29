## Codec Series (the builder)

> Crate: [`ndic-zarr`](../../crates/ndic-zarr/) (`series` module) · mirrored in
> the Python and TypeScript bindings · Roadmap:
> [Phase 1](../development/roadmap/phase-1-baselines-and-series.md)

A **codec series** is a complete Zarr v3 codec pipeline — a `transpose`
followed by decorrelation and a plane/block codec — that nd-image-codecs
assembles from an array's axis metadata. The `codec_series` builder is the one
component that is fully implemented today (pure metadata; it compiles and is
unit-tested), and it is the primary entry point for users.

### Inputs

| Input | Meaning |
| --- | --- |
| **axes** | One `(index, name)` pair per array dimension. `index` is the dimension's position in the array shape; `name` is its axis identifier (`"t"`, `"c"`, `"z"`, `"y"`, `"x"`, or any custom NGFF axis name). `x` and `y` are required. |
| **chunk_shape** | Chunk size per dimension (indexed by dimension index). A chunk size of 1 means the axis is *not grouped* within a chunk. |
| **dtype** | Zarr v3 data-type name, e.g. `"uint16"`, `"float32"`. |
| **family** | `nd-delta`, `nd-lift-ht`, or `nd-zfp`. |
| **decorrelate** | Optional override of which dimensions get a cross-axis transform (see below). |
| family tuning | `lift` kind, `xy_levels`, `reversible`, `delta_backend`, `zfp_rate`. |

### Axis roles

| Axis | Role | Default treatment |
| --- | --- | --- |
| `x` | Fastest spatial axis | Innermost; decorrelated by the 2D plane codec — **never** a cross-axis transform target |
| `y` | Second spatial axis | Second-innermost; likewise off-limits to `nd_lift` |
| `z` | Depth / slices | Grouped just above `yx`; **decorrelated by default** when chunk size > 1 |
| `t` | Time | Grouped just above `z` **only when its chunk size > 1**; decorrelated by default in that case; otherwise kept leading and untouched |
| `c` | Channel | Leading, untransformed by default (channels are often uncorrelated); opt in with a decorrelation override |
| other | Custom NGFF axes | Leading, untransformed by default |

### Transpose rule

The builder targets the order

```text
[ leading dims (original order) … , extra decorrelated dims … , (t), (z), y, x ]
```

- The fastest-moving dimensions are moved into `(z)yx` order.
- `t` is placed immediately before `z` (or before `y` if there is no `z`)
  **only when its chunk size is not 1** *and* it is in the decorrelation set —
  i.e. when it is actually being grouped and transformed. Otherwise `t` stays
  with the leading dimensions.
- If the target order equals the identity, **no `transpose` codec is emitted**.

### Decorrelation-axis selection

By default the decorrelation set is: `z` (when its chunk size > 1), and `t`
(when its chunk size > 1). Overrides:

| Override | Effect |
| --- | --- |
| **Defaults** | `z` and grouped `t`, as above. |
| **Exact([indices])** | Use exactly these dimension indices; replaces the defaults. Use this to, e.g., decorrelate a correlated channel axis only. |
| **Adjust{add, remove}** | Start from the defaults, add the `add` indices, drop the `remove` indices. |

`x` and `y` can never be decorrelation targets — the 2D plane codec already
decorrelates them — and the builder returns an error if they are requested.

### Per-family tails

**nd-delta** — built entirely from existing Zarr codecs:

```text
transpose → numcodecs.delta → bytes(little) → blosc(cname=zstd|lz4, shuffle=bitshuffle, clevel=5)
```

> **numcodecs.delta caveat.** `numcodecs.delta` differences the *flattened*
> chunk in C order, so it only decorrelates along the **fastest** axis. The
> builder therefore places the single chosen delta axis **last** (fastest) in
> the transpose — a different placement than nd-lift-ht/nd-zfp, which keep the
> spatial `yx` innermost. The default delta axis is `z` if grouped, else `t` if
> grouped. This is a deliberate trade-off: nd-delta reuses a stock, portable
> codec at the cost of decorrelating only one axis.

**nd-lift-ht**:

```text
transpose → nd_lift{transforms:[…per decorrelation axis…]} → htj2k{xy_levels, reversible, progression:RPCL, index:true}
```

Each `nd_lift` transform records the axis **name**, its **post-transpose
dimension index**, the lift **kind**, the number of **levels**, and a **group**
size. If the decorrelation set is empty, the `nd_lift` codec is omitted and the
series is just the `htj2k` plane codec.

**nd-zfp**:

```text
transpose → nd_zfp{mode, dims, [rate]}
```

`dims` is the number of non-singleton chunk dimensions (2–4). Requesting more
than 4 non-singleton chunk dimensions is an error — reduce the chunking or split
the array.

### Output

`codec_series` returns a list of Zarr v3 codec metadata objects, in application
order, ready to serialize into array metadata. Example (`t,c,z,y,x` uint16,
chunks `8,1,32,256,256`, nd-lift-ht):

```json
[
  { "name": "transpose", "configuration": { "order": [1, 0, 2, 3, 4] } },
  { "name": "nd_lift", "configuration": { "version": "0.1", "transforms": [
      { "axis": "t", "dimension": 1, "kind": "lift53", "levels": 2, "group": 0 },
      { "axis": "z", "dimension": 2, "kind": "lift53", "levels": 2, "group": 0 }
  ] } },
  { "name": "htj2k", "configuration": { "xy_levels": 5, "reversible": true, "progression": "RPCL", "index": true } }
]
```

### Three implementations, one behavior

The builder exists in Rust (`ndic_zarr::series::codec_series`), pure Python
(`nd_image_codecs.codec_series`), and pure TypeScript (`codecSeries`). The
Python and TypeScript versions work with **no native module built**, so metadata
authoring and pipeline validation are available everywhere. CI runs the same
matrix of inputs through all three and asserts byte-identical JSON — any change
to one must be mirrored in the others.

### Testing

- Rust unit tests in `series.rs` cover: t/z grouping, t-chunk-of-1 staying
  leading, XYZ→ZYX transpose, delta-axis-moves-fastest, exact/remove overrides,
  rejection of `x`/`y` decorrelation, and the ZFP dimension cap.
- Cross-language equality tests (Phase 1) run the shared fixture matrix through
  Rust, Python, and TypeScript.
- Round-trip validation against `zarr-python` + `imagecodecs` (Phase 6).
