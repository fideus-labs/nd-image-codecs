# nd-image-codecs

**Composable Zarr v3 codecs for ND scientific images** — Python binding.

A family of Zarr v3 codecs that capture correlation along z, time, and channel
axes *explicitly* — as ordinary, independently specified array-to-array and
array-to-bytes codecs — then store the result with a fast entropy backend,
High-Throughput JPEG 2000 (ISO/IEC 15444-15) coefficient planes, or ZFP blocks.
Built for OME-Zarr / OME-NGFF.

> No JPEG 2000 Part 2 (MCT) syntax anywhere — cross-axis decorrelation is an
> explicit Zarr codec, sidestepping Part 2 IP entirely.

## The three codec families

`codec_series` assembles a *series* (pipeline) of Zarr v3 codecs from an array's
axis metadata:

| Family | Series (pipeline) | Built for |
| --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bitshuffle → zstd/lz4` | Fast lossless storage from **existing** Zarr codecs only |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | Scalable microscopy & volume visualization |
| **nd-zfp** | `transpose → nd_zfp` | GPU volume rendering, random access, fixed-rate memory |

## Install

```sh
pip install nd-image-codecs
```

## Usage

```python
from nd_image_codecs import codec_series

codecs = codec_series(["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256],
                      "uint16", "nd-lift-ht")
```

The three codec classes (`NdLift`, `Htj2k`, `NdZfp`) register with `zarr-python`
v3 through the `zarr.codecs` entry-point group, so pipelines produced by
`codec_series` resolve by name.

## Status

**Pre-alpha.** The `codec_series` builder is fully implemented and is
cross-checked against the Rust and TypeScript implementations in CI. The codec
encode/decode paths are scaffolds — they land across the six
[roadmap phases](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/development/roadmap/index.md).

## Links

- [Repository](https://github.com/fideus-labs/nd-image-codecs)
- [Python usage guide](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/usage/python.md)
- [Architecture](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/architecture/index.md)

## License

MIT — Copyright (c) Fideus Labs LLC.
