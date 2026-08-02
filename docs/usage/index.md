---
title: Usage
description: Task-oriented guides for using nd-image-codecs from Zarr, the ndic CLI, Rust, Python, and TypeScript, with code blocks that CI will execute against the current API once Phase 6 lands.
---

# Usage

:::{caution} Status: Skeleton
Guides fill in as their features land (see the
[roadmap](../development/roadmap/index.md); usage docs are completed and
example-verified in
[Phase 6](../development/roadmap/phase-6-validation-and-docs.md)).
:::

Task-oriented guides. Code blocks on these pages are **static today** — nothing
executes them or checks them against the current API. Putting every snippet under
a docs CI job is
[Phase 6](../development/roadmap/phase-6-validation-and-docs.md) work; until it
lands, read a snippet as the intended API rather than as a tested example.

| Guide | Audience | You'll learn |
| --- | --- | --- |
| [](./zarr.md) | Data engineers / imaging scientists | The three codec families, the `codec_series` builder, chunking guidance, validation with imagecodecs |
| [](./cli.md) | Anyone with a terminal | `ndic compress / expand / series / inspect / index / thumbnail` |
| [](./rust.md) | Rust developers | Library encode/decode, `EncodeParams`, the series builder, partial decode |
| [](./python.md) | Python developers | NumPy round-trips, zarr-python entry points, OME-Zarr |
| [](./typescript.md) | Web developers | WASM codecs, zarrita.js, in-browser decode |
| [](./thumbnails-and-streaming.md) | Viewer builders | Byte-range plans, HTTP thumbnails, 3D previews |

New to the project? Start with the [architecture overview](../architecture/overview.md)
for the mental model, then the guide matching your ecosystem.
