---
title: Thumbnails & Streaming over HTTP Range
short_title: Thumbnails & Streaming
description: Decoding thumbnails, single planes, and low-resolution previews from any static host with plain HTTP Range requests — no tile server required.
---

# Thumbnails & Streaming over HTTP Range

:::{caution} Status: Skeleton
`RangeIndex`, thumbnail extraction, and 3D targets land
with [Phase 4](../development/roadmap/phase-4-nd-lift-ht.md).
:::

Any nd-lift-ht plane (`.jph`) or chunk on any static host (S3, GCS, nginx…)
supports partial decode with plain `Range:` requests — no tile server. Why this
works: [range-access architecture](../architecture/range-access.md).

## The plan format

```shell
$ ndic index https://example.com/volume.jph --target thumbnail
{
  "target": "thumbnail",
  "decoded_size": [64, 64],
  "ranges": [
    {"start": 0, "end": 1233},        // main header
    {"start": 1234, "end": 18761}     // R0..R1 packet prefix
  ],
  "total_bytes": 18762
}
```

Plans are coalesced — a thumbnail is typically 1–3 ranges.

## Executing a plan

```bash
curl -s -H "Range: bytes=0-1233,1234-18761" https://example.com/volume.jph -o thumb.part
ndic expand thumb.part --partial -o thumb.raw
```

Or in one step:

```bash
ndic thumbnail https://example.com/volume.jph --max 256 -o thumb.png
```

```typescript
// Browser: fetch ranges, hand bytes to the WASM codec (typescript.md)
const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
```

## Targets

| Target | Fetches | Use |
| --- | --- | --- |
| `thumbnail` | Low-resolution packet prefix | Grid views, previews |
| `thumbnail-3d` | Low-res packets of each group's low-pass planes | Volume preview (x, y **and** z downsampled) |
| `plane --z K` | One plane's codestream via the coefficient-plane index | Single-slice view of an nd-lift-ht chunk |
| `region --rect … --level L` | Precinct-aligned packet subset | Deep-zoom viewport |

## How 3D thumbnails work

With `nd_lift` decorrelating z (and grouped t), the coarse structure of each
group concentrates in its **low-pass plane(s)**. A `thumbnail-3d` plan fetches
only those planes' low-resolution prefixes — a volume downsampled in all three
axes from a handful of ranges
([](../architecture/nd-transform.md)).

## Patterns

- **Service worker:** intercept image requests, execute plans, cache by
  `(url, target)` — a static volume/WSI viewer with no backend.
- **Zarr:** stores with byte-range reads get low-res chunk decode for free; the
  coefficient-plane index locates planes inside chunks ([](./zarr.md)).
- **nd-zfp:** fixed-rate bricks need no plan at all — brick offsets are
  computable ([zfp architecture](../architecture/zfp.md)).
