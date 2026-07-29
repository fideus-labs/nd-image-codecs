# Usage

> **Status:** Skeleton — guides fill in as their features land (see the
> [roadmap](../development/roadmap/index.md); usage docs are completed and
> example-verified in
> [Phase 6](../development/roadmap/phase-6-validation-and-docs.md)).

Task-oriented guides. Every code block in these pages is executed by CI against the
current API — if it's written here, it runs.

| Guide | Audience | You'll learn |
| --- | --- | --- |
| [zarr.md](./zarr.md) | Data engineers / imaging scientists | The three codec families, the `codec_series` builder, chunking guidance, validation with imagecodecs |
| [cli.md](./cli.md) | Anyone with a terminal | `ndic compress / expand / series / inspect / index / thumbnail` |
| [rust.md](./rust.md) | Rust developers | Library encode/decode, `EncodeParams`, the series builder, partial decode |
| [python.md](./python.md) | Python developers | NumPy round-trips, zarr-python entry points, OME-Zarr |
| [typescript.md](./typescript.md) | Web developers | WASM codecs, zarrita.js, in-browser decode |
| [thumbnails-and-streaming.md](./thumbnails-and-streaming.md) | Viewer builders | Byte-range plans, HTTP thumbnails, 3D previews |

New to the project? Start with the [architecture overview](../architecture/overview.md)
for the mental model, then the guide matching your ecosystem.
