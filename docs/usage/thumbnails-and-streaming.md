---
title: Thumbnails & Streaming over HTTP Range
short_title: Thumbnails & Streaming
description: Decoding thumbnails, single planes, and low-resolution previews from any static host with plain HTTP Range requests — no tile server required.
---

# Thumbnails & Streaming over HTTP Range

:::{note} Status
Implemented ([Phase 4](../development/roadmap/phase-4-nd-lift-ht.md)):
`RangeIndex` plans, `ndic index`/`ndic thumbnail` (local files and HTTP
Range), `ndic expand --partial`, and 3D low-pass targets.
:::

Any nd-lift-ht plane (`.jph`) or chunk on any static host (S3, GCS, nginx…)
supports partial decode with plain `Range:` requests — no tile server. Why this
works: [range-access architecture](../architecture/range-access.md).

## The plan format

```shell
$ ndic index volume.jph --target thumbnail --max 64
{
  "target": "thumbnail",
  "decoded_size": [
    64,
    64
  ],
  "max_res": 3,
  "planes": [
    0
  ],
  "ranges": [
    {
      "start": 85,
      "end": 604
    }
  ],
  "total_bytes": 520
}
```

That is the real plan for the 256×256 plane built in the next section: 520 of
the codestream's 1049 bytes for a 64×64 preview — a modest saving at this
size, and the same mechanism that fetches a few kilobytes out of a gigabyte
plane. `decoded_size` is
the shape those bytes decode to and `max_res` the highest resolution they
cover — what `ndic expand --partial` and `decode_to_resolution` need.
`planes` lists the chunk plane indices a plan fetches.

Plans are coalesced — a thumbnail is typically 1–3 ranges.

## Executing a plan

The examples below run against a local static server so they can be executed;
any host that honors `Range:` — S3, GCS, nginx — behaves identically.

```bash
# A plane to plan against, and a Range-capable server for it — the
# repository ships one because `python3 -m http.server` ignores `Range:`.
# CI passes its own port.
python3 -c "
import sys
w, h = 256, 256
sys.stdout.buffer.write(b''.join(
    ((3 * x + 5 * y) % 4096).to_bytes(2, 'little')
    for y in range(h) for x in range(w)))
" > volume.raw
ndic compress -i volume.raw --raw-size 256x256 --raw-dtype u16 -o volume.jph

port="${DOCS_HTTP_PORT:-8000}"
repo="$(git rev-parse --show-toplevel)"
python3 "$repo/scripts/range-server.py" "$port" &
trap 'kill %1' EXIT
url="http://127.0.0.1:$port/volume.jph"
sleep 1

# Plan, fetch the planned span, decode what arrived — and check the fetch
# really was partial.
ranges=$(ndic index "$url" --target thumbnail --max 64 --format curl)
curl -s -H "Range: bytes=$ranges" "$url" -o thumb.part
test "$(stat -c%s thumb.part)" -lt "$(stat -c%s volume.jph)"
ndic expand -i thumb.part --partial -o thumb.raw
```

Or in one step:

```bash
ndic thumbnail "$url" --max 64 -o thumb.png
```

In the browser, fetch the same ranges and hand the bytes to the WASM codec
([TypeScript](./typescript.md)):

<!-- docs-check: skip — the browser fetch path; the bash blocks above execute the same plan -->
```typescript
const res = await fetch(url, { headers: { Range: `bytes=${start}-${end}` } });
const thumb = await codec.decode(new Uint8Array(await res.arrayBuffer()));
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
([the `nd_lift` transform](../architecture/nd-transform.md)).

## Patterns

- **Service worker:** intercept image requests, execute plans, cache by
  `(url, target)` — a static volume/WSI viewer with no backend.
- **Zarr:** stores with byte-range reads get low-res chunk decode for free; the
  coefficient-plane index locates planes inside chunks ([Zarr & OME-Zarr](./zarr.md)).
- **nd-zfp:** fixed-rate bricks need no plan at all — brick offsets are
  computable ([zfp architecture](../architecture/zfp.md)).
