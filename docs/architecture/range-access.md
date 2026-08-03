---
title: 'Byte-Range Access: Thumbnails Without a Smart Server'
short_title: Byte-Range Access
description: Thumbnails, single planes, and low-resolution 3D previews decode from plain HTTP Range requests against any static file server or object store — no JPIP, tile server, or sidecar index.
---

**Crate:** [`ndic-codestream`](https://github.com/fideus-labs/nd-image-codecs/tree/main/crates/ndic-codestream) · **Roadmap:**
[Phase 4](../development/roadmap/phase-4-nd-lift-ht.md)

A core requirement of the nd-lift-ht family: **thumbnails, single planes, and
low-resolution 3D previews must decode from plain HTTP `Range:` requests**
against any static file server or object store (S3, GCS, nginx). No JPIP, no
tile server, no proprietary index sidecar required.

## Why RPCL makes prefixes meaningful

With **RPCL** (Resolution → Position → Component → Layer) progression, every
packet of resolution level 0 appears before any packet of resolution 1, and so
on. The first bytes of a plane's codestream body therefore *are* the thumbnail:

```text
byte 0                                                            EOF
│ main hdr │ R0 packets │ R1 packets │ R2 packets │ … │ R5 packets │
            ◄─ fetch this prefix ⇒ decode a 1/32-scale thumbnail
```

RPCL front-loads whole low-resolution levels so the prefix thumbnail is small
and useful; the same access pattern underlies HTJ2K's adoption for medical and
remote-sensing streaming
([JPEG HTJ2K white paper](https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf)).

## The index: TLM + PLT + the coefficient-plane index

Prefix reads alone only give whole leading resolutions. For *arbitrary* subsets
the reader needs byte offsets, provided by two layers without decoding:

- **Within a plane** — `TLM` (main header, tile-part lengths) + `PLT`
  (tile-part header, packet lengths) give the byte offset of every packet.
- **Across planes** — the `htj2k` codec's **coefficient-plane index** records
  the byte range of each trailing-2D plane's `.jph` codestream within the chunk,
  so a reader can locate plane *z* (and its low-resolution prefix) directly.

`ndic-codestream` exposes this as a `RangeIndex`:

```text
fetch(main header) ─► parse TLM ─► fetch(tile-part headers w/ PLT) ─► RangeIndex
RangeIndex::thumbnail(max_px)        → Vec<ByteRange>   (2D thumbnail)
RangeIndex::thumbnail_3d(max_px, …)  → Vec<ByteRange>   (low-res, low-pass planes)
RangeIndex::plane(z)                 → Vec<ByteRange>   (one plane)
RangeIndex::region(rect, level)      → Vec<ByteRange>   (precinct-aligned sub-region)
```

Each plan is a small list of contiguous ranges, deliberately coalesced (adjacent
packets merge into one range) so a whole thumbnail is typically **1–3 Range
requests**: one for the header, one for the R0…Rk prefix or the packet runs.

The `ndic index` CLI subcommand prints these plans so any HTTP client (or a
viewer's service worker) can execute them without linking nd-image-codecs's
decoder; the `ndic thumbnail` subcommand executes one directly.

## 3D thumbnails and the nd_lift low-pass

With `nd_lift` decorrelating z (and grouped t), each group's **low-pass band**
concentrates the coarse structure into a few planes whose low-resolution packets
sit early in each RPCL sequence. A 3D thumbnail plan selects, per group:
resolution levels 0…k of the low-pass plane(s) only. The result decodes into a
volume downsampled in x, y, *and* z — fetched with the same handful of Range
requests (see [the `nd_lift` transform](./nd-transform.md)).

## Precincts: measured guidance for `region` plans

Default precincts are **maximal** — one whole-plane precinct per resolution.
Every packet then spans the full plane, so a `RangeIndex::region` plan
degrades to the resolution prefix: it fetches *all* of resolutions
`0..=level`, however small the viewport.

Measured on a 2048×2048 8-bit plane (5 levels, 527 KiB codestream), a
256×256 viewport — 1.6 % of the area — plans:

| `--level` | decoded size | bytes fetched | share of stream |
| --- | --- | --- | --- |
| 0 | 8×8 | 4.4 KiB | 0.8 % |
| 1 | 16×16 | 9.5 KiB | 1.8 % |
| 2 | 32×32 | 21.8 KiB | 4.1 % |
| 3 | 64×64 | 51.6 KiB | 9.8 % |

Each plan is a **single** contiguous range (the RPCL prefix), so for
thumbnails and low-`level` navigation maximal precincts are exactly right:
minimal header overhead, one request, and the low resolutions are cheap
regardless of viewport size. The cost only appears at *high* levels of
*very large* planes, where the whole-plane packets dwarf the viewport — at
`--level 3` above, a precinct-partitioned stream (256×256 precincts ⇒ four
precincts at that resolution) would fetch roughly the viewport's share
(~25 % of that resolution's packets, ~3 % of the stream) at the price of
per-precinct packet headers (~1–2 % size overhead) and more, smaller
ranges per plan.

**Guidance:** stay with maximal precincts for chunked Zarr data — chunk
planes (≤ 1024²) never accumulate enough high-resolution bytes for
precincts to pay for their overhead. Reach for `EncodeParams::precincts`
(256×256 at the finest two resolutions) only for monolithic planes ≳ 4k²
served for deep-zoom viewports; the writer's precinct support lands with a
later phase, and `RangeIndex::region` is already precinct-aware on the
read side (the reader decodes explicit-precinct streams today).

## nd-zfp random access

The nd-zfp family provides a different, complementary random-access story: in
fixed-rate mode every `4^d` brick has a constant byte size, so a renderer
computes any brick's offset arithmetically — no index fetch at all. The variable
modes carry an explicit brick index. See [the `nd_zfp` codec](./zfp.md).
