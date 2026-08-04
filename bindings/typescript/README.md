# @fideus-labs/nd-image-codecs

**Composable Zarr v3 codecs for ND scientific images** — TypeScript / WebAssembly binding.

A family of Zarr v3 codecs that capture correlation along z, time, and channel
axes *explicitly* — as ordinary, independently specified array-to-array and
array-to-bytes codecs — then store the result with a fast entropy backend,
High-Throughput JPEG 2000 (ISO/IEC 15444-15) coefficient planes, or ZFP blocks.
Built for OME-Zarr / OME-NGFF and [zarrita.js](https://zarrita.dev).

> No JPEG 2000 Part 2 (MCT) syntax anywhere — cross-axis decorrelation is an
> explicit Zarr codec, sidestepping Part 2 IP entirely.

## The three codec families

`codecSeries` assembles a *series* (pipeline) of Zarr v3 codecs from an array's
axis metadata:

| Family | Series (pipeline) | Built for |
| --- | --- | --- |
| **nd-delta** | `transpose → numcodecs.delta → bitshuffle → zstd/lz4` | Fast lossless storage from **existing** Zarr codecs only |
| **nd-lift-ht** | `transpose → nd_lift → htj2k` | Scalable microscopy & volume visualization |
| **nd-zfp** | `transpose → reshape → zfp` | GPU volume rendering, random access, fixed-rate memory |

## Install

```sh
npm install @fideus-labs/nd-image-codecs
```

## Usage

```ts
import { codecSeries } from "@fideus-labs/nd-image-codecs";

const codecs = codecSeries(["t", "c", "z", "y", "x"], [8, 1, 32, 256, 256],
                           "uint16", "nd-lift-ht");
```

The codec classes (`NdLift`, `Htj2k`, `NdZfp`) follow the
[numcodecs.js](https://github.com/manzt/numcodecs.js) convention with a static
`fromConfig`, so they register with zarrita.js:

```ts
import * as zarrita from "zarrita";
import { registerZarritaCodecs } from "@fideus-labs/nd-image-codecs";

registerZarritaCodecs(zarrita.registry);
```

One call registers zarrita-native adapters for `nd_lift`, `htj2k`, `zfp`,
and `reshape` (plus the deprecated `nd_zfp` read alias), and replaces
zarrita's `transpose`/`numcodecs.delta`/`blosc` entries, whose stock
implementations cannot *write* transposed pipelines correctly.

## Status

**Pre-alpha.** `codecSeries` is fully implemented in pure TypeScript and is
cross-checked against the Rust and Python implementations in CI. The
`nd_lift`, `htj2k`, and `zfp` encode/decode paths are backed by the same
Rust core as the `zarrs` codecs and the Python extension, compiled to WASM,
so every ecosystem produces byte-identical chunks — CI pins the committed
micro-fixtures and the `nd_lift` conformance vectors in all three languages.

## Links

- [Repository](https://github.com/fideus-labs/nd-image-codecs)
- [TypeScript usage guide](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/usage/typescript.md)
- [Architecture](https://github.com/fideus-labs/nd-image-codecs/blob/main/docs/architecture/index.md)

## License

MIT — Copyright (c) Fideus Labs LLC.
