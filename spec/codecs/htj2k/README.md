# `htj2k` codec

Defines an `array -> bytes` codec that codes each trailing 2D plane of a chunk
as an independent High Throughput JPEG 2000 (HTJ2K) codestream — ITU-T T.800
Part 1 with the Part 15 (T.814) FBCOT block coder — and binds the planes
together with a byte index that makes low-resolution and single-plane reads
answerable from a byte range.

The point is not merely a good ratio. An HTJ2K codestream in `RPCL`
progression stores its lowest resolution first, so a *prefix* of the bytes
decodes a thumbnail; the index in this container records where those prefixes
end. A viewer over a store with byte-range reads (HTTP `Range`, S3, a sharded
Zarr store) gets progressive resolution and single-slice access without a tile
server and without decoding the chunk.

## Codec name

The value of the `name` member in the codec object MUST be `htj2k`.

## Configuration parameters

| Member | Type | Default | Meaning |
| --- | --- | --- | --- |
| `xy_levels` | unsigned integer | `5` | In-plane wavelet decomposition levels. Resolutions available = `xy_levels + 1`. MUST be ≤ 32 (the JPEG 2000 Part 1 `SPcod` bound). |
| `reversible` | boolean | `true` | `true` selects the reversible 5/3 wavelet (lossless). `false` selects the irreversible 9/7 wavelet (lossy). |
| `progression` | string | `"RPCL"` | Progression order: one of `"LRCP"`, `"RLCP"`, `"RPCL"`, `"PCRL"`, `"CPRL"`. `"RPCL"` is what makes a byte prefix a resolution prefix; other orders are conformant but forfeit that property. |
| `index` | boolean | `true` | Write the coefficient-plane index (and per-plane `TLM`/`PLT` marker segments). With `index: false` the chunk is a bare concatenation and only whole-chunk decode is possible. |

All members are optional. Unknown members MUST be rejected.

## Example

```json
{
  "codecs": [
    {
      "name": "htj2k",
      "configuration": {
        "xy_levels": 5,
        "reversible": true,
        "progression": "RPCL",
        "index": true
      }
    }
  ]
}
```

## Supported chunk shapes

Two or more dimensions, up to 32. The **trailing two** dimensions are the 2D
plane `(y, x)`; every leading dimension indexes planes. A chunk of shape
`[c, z, y, x]` therefore holds `c × z` planes, each coded independently and
each individually addressable through the index.

Because the plane axes are the trailing two, a `transpose` codec upstream is
what puts the spatial axes there — see the reference implementation's
codec-series builder.

These rules apply to the inner chunk shape when this codec is used as the
array-to-bytes codec within `sharding_indexed`.

## Supported data types

`uint8`, `int8`, `uint16`, `int16`, `uint32`, `int32`.

Each plane declares its **actual** dynamic range in the codestream's `Ssiz`
field, not the storage type's nominal width. That is what lets the `int32`
coefficient planes produced by an upstream `nd_lift` codec fit the 32-bit HT
datapath: JPEG 2000 permits any per-component precision from 1 to 38 bits, and
the HT datapath admits declared depths up to `30 − X` bits where `X` is the
5/3 ranging gain (2–4 bits). A full 32-bit declaration is rejected; values that
*fit* a narrower declaration are not.

Floating-point types are not supported by the reversible path. (A lossy
floating-point path would quantize before coding; it is not specified here.)

## Format and algorithm

A chunk is `[header | coefficient-plane index | codestreams…]`. All integers
are little-endian.

### Header

```text
offset  size    field
0       4       magic, the ASCII bytes "ndht"
4       1       version = 1
5       1       flags   (bit 0: coefficient-plane index present)
6       1       xy_levels
7       1       ndim    (2..=32)
8       4*ndim  dims, u32 each — chunk shape as this codec sees it,
                trailing two = (y, x)
```

### Coefficient-plane index

Present when flag bit 0 is set. One entry per plane, in plane order
(C order over the leading dimensions):

```text
size            field
8               offset — the plane codestream's byte offset from the chunk's
                first byte
4               len — the plane codestream's byte length
4*(xy_levels+1) prefix[r] — bytes from the plane's first byte that suffice to
                decode resolutions 0..=r
```

`prefix` is non-decreasing and `prefix[xy_levels] <= len`. A reader that wants
resolution `r` of plane `p` fetches `[offset_p, offset_p + prefix_p[r])` and
decodes that prefix; it never reads the rest of the chunk. The header parses
from a chunk *prefix*, so a remote reader bootstraps from one small ranged
fetch.

An implementation MUST bound `ndim` (≤ 32) and the plane count before sizing
the entry table, so a malformed header cannot induce a large allocation.

### Codestreams

Each plane is a complete, conforming single-component JPEG 2000 codestream:
`SOC`, `SIZ`, `COD`/`COC`, `QCD`/`QCC`, `CAP`, optional `COM`, optional `TLM`,
then tile-parts (`SOT`, optional `PLT`, `SOD`, packets), then `EOC`. HT block
coding is signalled through `CAP` — `Pcap` bit 15 set, and `Ccap15` carrying
the HT mode (`HTONLY` or `MIXED`) and `MAGB`.

Encoding one plane:

1. Level-shift and, when the array's samples are unsigned, offset to signed.
2. Apply `xy_levels` levels of the 2D wavelet (5/3 when `reversible`, 9/7
   otherwise).
3. Partition each subband into code-blocks and code each with the Part 15
   FBCOT block coder.
4. Emit packets in `progression` order, writing `TLM`/`PLT` when `index` is
   set.

Decoding reverses these steps. Decoding a *prefix* stops at the last complete
packet the bytes contain, which — in `RPCL` — is a complete lower resolution.

No JPEG 2000 Part 2 syntax is emitted or parsed. In particular, cross-component
or cross-axis decorrelation is never expressed as an MCT: it belongs to a
separate array-to-array codec upstream (see [`nd_lift`](../nd_lift/README.md)).

## Interoperability and compatibility

Each plane codestream is conforming Part 1 / Part 15, so any JPEG 2000
implementation that reads HT codestreams can decode it. What the reference
implementation's CI actually pins is bit-exact plane decode through
`imagecodecs.jpeg2k_decode` (OpenJPEG); OpenJPH interop is exercised by
opt-in corpus and differential suites that fetch and build OpenJPH locally.
Extracting a plane needs only its `offset` and `len` from the index above.

What is specific to this codec is the *container*: the header and index that
bind a chunk's planes together. A third-party reader that wants whole planes
parses that fixed-size header; a reader that only wants pixels can rely on
this codec's implementations, which are byte-identical across ecosystems by
construction (one core, compiled natively and to WebAssembly).

Not currently implemented by the reference implementation: tiles beyond one
tile per plane, subsampled components, custom precincts, and multiple quality
layers. These are permitted by the format and are additive.

## Changelog

- **1** (container version) — initial version.

## Maintainers

- [Fideus Labs LLC](https://github.com/fideus-labs) —
  [nd-image-codecs](https://github.com/fideus-labs/nd-image-codecs)
