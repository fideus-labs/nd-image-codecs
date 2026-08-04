---
title: CLI — ndic
short_title: CLI
description: 'The ndic command-line tool: build a codec series from an array''s axis metadata, then encode, decode, and inspect nd-image-codecs data.'
---

# CLI — `ndic`

:::{note} Status
All subcommands work: `series` (Phase 1), `compress`/`expand`/`inspect`
(Phase 3), `index`/`thumbnail` with HTTP Range execution
([Phase 4](../development/roadmap/phase-4-nd-lift-ht.md)), and `zarr`
([Phase 6](../development/roadmap/phase-6-validation-and-docs.md)). Every
command line on this page is executed by CI.
:::

<!-- docs-check: skip — installs into the user's cargo bin; the checker uses the workspace build -->
```bash
cargo install --path crates/ndic-cli   # or a released binary, post-1.0
```

## Sample data

The examples below work on one small raw plane. Make one with any tool that
writes little-endian samples:

```bash
python3 -c "
import sys
w, h = 512, 512
sys.stdout.buffer.write(b''.join(
    ((3 * x + 5 * y) % 4096).to_bytes(2, 'little')
    for y in range(h) for x in range(w)))
" > input.raw
```

## Series — build Zarr codec pipelines

Describe the array; get the Zarr v3 codec JSON for a family:

```bash
ndic series --axes t,c,z,y,x --chunks 8,1,32,256,256 --dtype uint16 --family nd-lift-ht
ndic series --axes x,y,z   --chunks 256,256,32     --dtype float32 --family nd-zfp
ndic series --axes t,c,z,y,x --chunks 8,1,32,256,256 --dtype uint16 --family nd-delta
```

Output is the `codecs` JSON array to paste into (or pipe through) your Zarr
tooling — identical to what the [Python](./python.md) and
[TypeScript](./typescript.md) builders produce. See
[Zarr & OME-Zarr](./zarr.md) for the builder's transpose/decorrelation rules.

A single codec object can be checked against the codec that owns it. Each
configuration type rejects unknown keys, so this catches a typo before it
reaches a store:

```bash
ndic series --validate-codec '{"name": "zfp", "configuration": {"mode": "fixed_rate", "rate": 8.0}}'
```

## Compress

```bash
# Defaults: RPCL, 5 xy levels, 64×64 HT code-blocks, reversible 5/3, TLM/PLT on.
ndic compress -i input.raw --raw-size 512x512 --raw-dtype u16 -o out.jph
```

## Expand

```bash
ndic expand -i out.jph -o out.raw                    # full decode
ndic expand -i out.jph --resolution 2 -o small.raw   # 1/4-scale (2 levels down)
```

## Inspect

```bash
ndic inspect -i out.jph             # markers, resolutions, layers, tile-parts
ndic inspect -i out.jph --packets   # every packet from the TLM/PLT index
```

## Index — byte-range plans

```bash
ndic index out.jph --target thumbnail          # ranges for a 2D thumbnail
ndic index out.jph --target region --rect 128,128,256,256 --level 1
```

`--target thumbnail-3d` plans a low-resolution, low-pass-z preview of an
`ndht` chunk instead; it reads the chunk's `nd_lift` configuration from
`--series`, so see [thumbnails & streaming](./thumbnails-and-streaming.md)
for that flow.

Output is a JSON list of `{start, end}` ranges; execute them with any HTTP
client against a host that honors `Range:` — S3, GCS, and nginx all do. For
a local server use the repository's `scripts/range-server.py`
(`python3 -m http.server` ignores `Range:` and would return whole files):

```bash
# CI passes its own port; pick any free one when following along.
port="${DOCS_HTTP_PORT:-8000}"
repo="$(git rev-parse --show-toplevel)"
python3 "$repo/scripts/range-server.py" "$port" &
trap 'kill %1' EXIT
url="http://127.0.0.1:$port/out.jph"
sleep 1

ranges=$(ndic index "$url" --target thumbnail --format curl)
curl -s -H "Range: bytes=$ranges" "$url" -o thumb.part
ndic expand -i thumb.part --partial -o thumb.raw

# The fetch really was partial: fewer bytes than the whole codestream.
test "$(stat -c%s thumb.part)" -lt "$(stat -c%s out.jph)"
```

## Thumbnail

```bash
ndic thumbnail out.jph --max 256 -o thumb.png       # 2D, local file
ndic thumbnail "$url" --max 128 -o remote.png       # the same, over HTTP Range
```

`thumbnail` plans (like `index`) and then executes the fetch + partial decode in
one step. See [thumbnails & streaming](./thumbnails-and-streaming.md) for
the full workflow and [byte-range access](../architecture/range-access.md) for why
this works.

## Zarr stores

`ndic zarr` writes and reads Zarr v3 stores through the registered codecs —
the Rust corner of the [cross-ecosystem validation
matrix](../development/roadmap/phase-6-validation-and-docs.md), and a quick
way to produce a store that any zarr-python or zarrita.js reader can open.
It is behind a feature flag because it pulls in `zarrs`:

<!-- docs-check: skip — builds the workspace; the checker's ndic already carries the feature -->
```bash
cargo build -p ndic-cli --features zarr
```

`write` takes a JSON spec carrying the array `shape` alongside the same
`axes`, `chunk_shape`, `dtype`, `family`, and builder `options` that `ndic
series` accepts, plus the raw little-endian samples; `read` decodes the whole
array back to raw bytes:

```bash
cat > case.json <<'JSON'
{
  "shape": [8, 512, 512],
  "axes": ["z", "y", "x"],
  "chunk_shape": [4, 256, 256],
  "dtype": "uint16",
  "family": "nd-lift-ht",
  "options": { "xy_levels": 2 }
}
JSON

# One 512×512 plane repeated 8 times, so the input matches `shape`.
for _ in $(seq 8); do cat input.raw; done > volume.raw

ndic zarr write --store volume.zarr --spec case.json --input volume.raw
ndic zarr read  --store volume.zarr --output roundtrip.raw
cmp volume.raw roundtrip.raw && echo "store round-trips exactly"
```
