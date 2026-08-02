---
title: CLI — ndic
short_title: CLI
description: 'The ndic command-line tool: build a codec series from an array''s axis metadata, then encode, decode, and inspect nd-image-codecs data.'
---

# CLI — `ndic`

:::{caution} Status: Skeleton
`ndic series` works today; encode/decode behavior lands
with [Phase 3](../development/roadmap/phase-3-htj2k-core.md) and
[Phase 4](../development/roadmap/phase-4-nd-lift-ht.md).
:::

```bash
cargo install --path crates/ndic-cli   # or a released binary, post-1.0
```

## Series — build Zarr codec pipelines

Working now. Describe the array; get the Zarr v3 codec JSON for a family:

```bash
ndic series --axes t,c,z,y,x --chunks 8,1,32,256,256 --dtype uint16 --family nd-lift-ht
ndic series --axes x,y,z   --chunks 256,256,32     --dtype float32 --family nd-zfp
ndic series --axes t,c,z,y,x --chunks 8,1,32,256,256 --dtype uint16 --family nd-delta
```

Output is the `codecs` JSON array to paste into (or pipe through) your Zarr
tooling — identical to what the [Python](./python.md) and
[TypeScript](./typescript.md) builders produce. See
[Zarr & OME-Zarr](./zarr.md) for the builder's transpose/decorrelation rules.

## Compress

```bash
# Defaults: RPCL, 5 xy levels, 64×64 HT code-blocks, reversible 5/3, TLM/PLT on.
ndic compress input.raw --size 2048x2048 --dtype u16 -o out.jph

# Lossy 9/7:
ndic compress input.raw --size 2048x2048 --dtype u16 --irreversible -o out.jph
```

## Expand

```bash
ndic expand out.jph -o out.raw                    # full decode
ndic expand out.jph --resolution 2 -o small.raw   # 1/4-scale (2 levels down)
```

## Inspect

```bash
ndic inspect out.jph            # markers, resolutions, layers, tile-parts
ndic inspect out.jph --stats    # per-pass byte shares, packet histograms
```

## Index — byte-range plans

```bash
ndic index out.jph --target thumbnail          # ranges for a 2D thumbnail
ndic index volume.jph --target thumbnail-3d    # low-res, low-pass-z preview
ndic index out.jph --target region --rect 1024,1024,512,512 --level 1
```

Output is a JSON list of `{start, end}` ranges; execute with any HTTP client:

```bash
ranges=$(ndic index https://example.com/out.jph --target thumbnail --format curl)
curl -H "Range: bytes=$ranges" https://example.com/out.jph -o thumb.part
ndic expand thumb.part --partial -o thumb.raw
```

## Thumbnail

```bash
ndic thumbnail out.jph --max 256 -o thumb.png            # 2D, local file
ndic thumbnail https://example.com/vol.jph --max 128 \
    --three-d -o preview.raw                             # 3D preview over HTTP Range
```

`thumbnail` plans (like `index`) and then executes the fetch + partial decode in
one step. See [thumbnails & streaming](./thumbnails-and-streaming.md) for
the full workflow and [byte-range access](../architecture/range-access.md) for why
this works.
