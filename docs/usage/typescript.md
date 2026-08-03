---
title: TypeScript / Browser
short_title: TypeScript
description: 'Using nd-image-codecs from TypeScript and the browser: the codecSeries builder and the WASM codec cores for zarrita.js.'
---

# TypeScript / Browser

:::{caution} Status: Partial
The `codecSeries` builder and the `htj2k` WASM core work
(`npm run build:wasm`); the `nd_lift` and `nd_zfp` WASM paths land per the
[roadmap](../development/roadmap/index.md).
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
import { Htj2k } from "@fideus-labs/nd-image-codecs";

// An array-to-bytes codec needs the chunk geometry alongside its config
// (both come from the array's zarr.json metadata).
const codec = Htj2k.fromConfig(
  {
    name: "htj2k",
    configuration: { xy_levels: 5, reversible: true, progression: "RPCL", index: true },
  },
  { shape: [32, 256, 256], dtype: "uint16" }, // post-transpose chunk shape, (…, y, x)
);

const bytes = new Uint8Array(await (await fetch(chunkUrl)).arrayBuffer());
const decoded = await codec.decode(bytes); // Uint8Array of little-endian samples
```

The codecs follow the [numcodecs.js](https://github.com/manzt/numcodecs.js)
convention: one small JS wrapper per codec plus a lazily-instantiated `.wasm`
module built from the same Rust core as the native codecs (run
`npm run build:wasm` when working from the source tree).

## zarrita.js registration

```typescript
import * as zarrita from "zarrita";
import { Htj2k } from "@fideus-labs/nd-image-codecs";

zarrita.registry.set("htj2k", () => Htj2k);
const arr = await zarrita.open(store, { kind: "array" });
const view = await zarrita.get(arr, [null, null, null]);
```

`NdLift` and `NdZfp` register the same way once their WASM encode/decode
paths land (see the status note above) — today they are config/validation
classes only, so reading an `nd_lift`-decorrelated array from the browser
still needs those phases.

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
