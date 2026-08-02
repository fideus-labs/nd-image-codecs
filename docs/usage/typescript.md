---
title: TypeScript / Browser
short_title: TypeScript
description: 'Using nd-image-codecs from TypeScript and the browser: the codecSeries builder and the WASM codec cores for zarrita.js.'
---

# TypeScript / Browser

:::{caution} Status: Skeleton
The package scaffolding and the `codecSeries` builder
exist; the WASM cores land per the [roadmap](../development/roadmap/index.md).
:::

```bash
npm install @fideus-labs/nd-image-codecs
```

## Build a codec series

Works today — the builder is pure TypeScript (mirrors the Rust implementation
byte-for-byte):

```typescript
import { codecSeries } from "@fideus-labs/nd-image-codecs";

const codecs = codecSeries(
  [..."tczyx"],              // one name per dimension
  [8, 1, 32, 256, 256],
  "uint16",
  "nd-lift-ht",              // "nd-delta" | "nd-lift-ht" | "nd-zfp"
);
// → Zarr v3 codec JSON, identical to the Rust/Python builders
```

## Decode a chunk

```typescript
import { Htj2k, NdLift, NdZfp } from "@fideus-labs/nd-image-codecs";

const codec = Htj2k.fromConfig({
  id: "htj2k", xy_levels: 5, reversible: true, progression: "RPCL", index: true,
});

const bytes = new Uint8Array(await (await fetch(chunkUrl)).arrayBuffer());
const decoded = await codec.decode(bytes); // Uint8Array of raw samples
```

The codecs follow the [numcodecs.js](https://github.com/manzt/numcodecs.js)
convention: one small JS wrapper per codec plus a lazily-instantiated `.wasm`
module (SIMD128).

## zarrita.js registration

```typescript
import * as zarrita from "zarrita";
import { Htj2k, NdLift, NdZfp } from "@fideus-labs/nd-image-codecs";

zarrita.registry.set("htj2k", () => Htj2k);
zarrita.registry.set("nd_lift", () => NdLift);
zarrita.registry.set("nd_zfp", () => NdZfp);
const arr = await zarrita.open(store, { kind: "array" });
const view = await zarrita.get(arr, [null, null, null]);
```

nd-delta pipelines need no registration at all — they use codecs zarrita.js
already ships (`transpose`, `blosc`, …).

## Streaming thumbnails in the browser

Combine the `htj2k` codec with Range requests against `.jph` files or Zarr
chunks — the byte plans from `ndic index` are plain JSON your app (or a service
worker) can execute with `fetch(url, { headers: { Range: "bytes=…" } })`. See
[thumbnails & streaming](./thumbnails-and-streaming.md).

## Bundler notes

- The `.wasm` asset must be served with `application/wasm` for streaming
  compilation.
- No threads are used (no COOP/COEP requirements); SIMD128 is required — all
  evergreen browsers ship it ([WebAssembly roadmap](https://webassembly.org/roadmap/)).
- Size budget is tracked in CI: < 500 KB gzipped WASM per codec.
