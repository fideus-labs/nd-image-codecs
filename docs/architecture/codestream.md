## Codestream Syntax

> Crate: [`ndic-codestream`](../../crates/ndic-codestream/) · Roadmap:
> [Phase 3](../development/roadmap/phase-3-htj2k-core.md) (core + indexing)

The `htj2k` codec reads and writes raw JPEG 2000 codestreams (`.j2c`) and the
JPH box format (`.jph`, the Part 15 analogue of `.jp2`) for each trailing 2D
plane. The syntax layer follows [ITU-T T.800](https://www.itu.int/rec/T-REC-T.800)
Annex A with Part 15 additions from [T.814](https://www.itu.int/rec/T-REC-T.814).

**No JPEG 2000 Part 2 (MCT) markers are ever emitted or parsed.** Cross-axis
decorrelation lives entirely in the `nd_lift` codec upstream (see
[nd-transform.md](./nd-transform.md)), so every codestream this crate produces
is pure Part 1 + Part 15.

### Anatomy

```text
SOC ─ SIZ ─ COD ─ [COC…] ─ QCD ─ [QCC…] ─ CAP ─ [COM] ─ TLM
 └ main header                                          │
SOT ─ [PLT…] ─ SOD ─ packet packet packet …            ◄┘
EOC
```

A **packet** is the atomic unit of the body: the code-block contributions for
one (resolution, precinct, layer, component). The progression order dictates
packet interleaving; the codec defaults to **RPCL** so all packets of resolution
0 precede resolution 1, and so on (see [range-access.md](./range-access.md)).

### Markers emitted

| Marker | Code | Purpose | Notes |
| --- | --- | --- | --- |
| `SOC` | FF4F | Start of codestream | |
| `SIZ` | FF51 | Image/tile grid, components, `Ssiz` dtypes | 2D plane geometry |
| `COD` | FF52 | Coding style: progression, layers, code-block size/style, wavelet | `SPcod` block-coder field selects HT |
| `COC` | FF53 | Per-component coding overrides | |
| `QCD`/`QCC` | FF5C/5D | Quantization (9/7) / ranging (5/3) | |
| `CAP` | FF50 | Extended capabilities: `Pcap` bit 15 for Part 15; `Ccap15` carries `MAGB`, HTONLY/HTDECLARED, MIXED | **Mandatory** for HT codestreams ([T.814](https://www.itu.int/rec/T-REC-T.814)) |
| `COM` | FF64 | Comment: writer name/version | |
| `TLM` | FF55 | Tile-part lengths in the **main header** | Always emitted (`emit_tlm_plt`) |
| `SOT`/`SOD` | FF90/93 | Tile-part header bounds | |
| `PLT` | FF58 | Packet lengths in the **tile-part header** | Always emitted |
| `EOC` | FFD9 | End of codestream | |

`TLM` + `PLT` are what turn a codestream into a *random-access file*: together
they give the byte offset of every packet without decoding anything (see
[range-access.md](./range-access.md)). We make them the default rather than an
option.

### HT signaling specifics

- `CAP`'s `Pcap` sets bit 15 (Part 15 capability); `Ccap15` declares whether
  *all* blocks are HT (`HTONLY`) or mixed, whether HT Sets can carry multiple
  refinements, and `MAGB` — the magnitude-bit bound the decoder uses to size its
  datapaths.
- `COD`/`COC` `SPcod` selects the block-coder per tile-component: J2K-1, HT, or
  MIXED — the mechanism that keeps HTJ2K a strict syntactic superset of Part 1
  ([HTJ2K white paper](https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf)).

### Coefficient-plane index

Because a chunk holds many trailing 2D planes (one per z, and per grouped t),
the `htj2k` codec writes an outer **coefficient-plane index**: a small table of
the byte range of each plane's codestream within the chunk. This is what lets
`RangeIndex::plane(z)` locate a single plane, and what a 3D-thumbnail plan walks
to gather each group's low-pass plane (see
[range-access.md](./range-access.md)).

### Reader design

The reader is a pull parser over any `Read + Seek` (or async range-fetch)
source:

1. Parse main header markers into typed structs (`Siz`, `Cod`, `Cap`…).
2. Build the packet index from `TLM`/`PLT` when present; otherwise fall back to
   sequential packet-header walking.
3. Decode only requested (resolution, component, precinct) subsets — partial
   decode is the *primary* path, full decode the special case.

Malformed input never panics: every parse error surfaces as
`Error::Codestream { offset, message }`, and fuzzing (OSS-Fuzz style, mirroring
[OpenJPH's harness](https://github.com/aous72/OpenJPH/tree/master/fuzzing)) runs
the reader against corpus mutations in CI.
