---
title: Usage
description: Task-oriented guides for using nd-image-codecs from Zarr, the ndic CLI, Rust, Python, and TypeScript, with every code block executed by CI against the current API.
---

# Usage

:::{note} Status
Every code block on these pages is **executed by CI** against the current
API — the Rust snippets are compiled and run, the Python and TypeScript ones
imported and run, the shell ones executed, and the codec configurations
round-tripped through the codecs that own them
([`scripts/ci/check-usage-docs.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/ci/check-usage-docs.py)).
A snippet that cannot run in CI carries a comment saying why, and the check
reports those exemptions; there are a handful, all of them install commands
or the browser `fetch` path.
:::

Task-oriented guides.

| Guide | Audience | You'll learn |
| --- | --- | --- |
| [Zarr & OME-Zarr](./zarr.md) | Data engineers / imaging scientists | The three codec families, the `codec_series` builder, chunking guidance, validation with imagecodecs |
| [CLI — ndic](./cli.md) | Anyone with a terminal | `ndic compress / expand / series / inspect / index / thumbnail` |
| [Rust Library](./rust.md) | Rust developers | Library encode/decode, `EncodeParams`, the series builder, partial decode |
| [Python](./python.md) | Python developers | NumPy round-trips, zarr-python entry points, OME-Zarr |
| [TypeScript / Browser](./typescript.md) | Web developers | WASM codecs, zarrita.js, in-browser decode |
| [Thumbnails & Streaming](./thumbnails-and-streaming.md) | Viewer builders | Byte-range plans, HTTP thumbnails, 3D previews |

New to the project? Start with the [architecture overview](../architecture/overview.md)
for the mental model, then the guide matching your ecosystem.
