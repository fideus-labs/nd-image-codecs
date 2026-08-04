---
title: TypeScript / Browser
short_title: TypeScript
description: 'Using nd-image-codecs from TypeScript and the browser: the codecSeries builder and the WASM codec cores for zarrita.js.'
---

# TypeScript / Browser

:::{note} Status
The `codecSeries` builder and the `nd_lift`, `htj2k`, and `zfp` WASM cores
all work (`npm run build:wasm` when working from the source tree). Every
snippet on this page is executed by CI.
:::

<!-- docs-check: skip — installs the package the checker resolves from the workspace -->
```bash
npm install @fideus-labs/nd-image-codecs
```

## Build a codec series

The builder is pure TypeScript and mirrors the Rust implementation
byte-for-byte:

```typescript
import assert from "node:assert/strict";   // so this page's checks run in CI
import { codecSeries } from "@fideus-labs/nd-image-codecs";

const codecs = codecSeries(
  [..."tczyx"],              // one name per dimension
  [8, 1, 32, 256, 256],
  "uint16",
  "nd-lift-ht",              // "nd-delta" | "nd-lift-ht" | "nd-zfp"
);
// → Zarr v3 codec JSON, identical to the Rust/Python builders
assert.deepEqual(codecs.map((c) => c.name), ["transpose", "nd_lift", "htj2k"]);
```

## Decode a chunk

```typescript
import { Htj2k } from "@fideus-labs/nd-image-codecs";

// An array-to-bytes codec needs the chunk geometry alongside its config
// (both come from the array's zarr.json metadata).
const codec = Htj2k.fromConfig(
  {
    name: "htj2k",
    configuration: { xy_levels: 2, reversible: true, progression: "RPCL", index: true },
  },
  { shape: [4, 32, 32], dtype: "uint16" }, // post-transpose chunk shape, (…, y, x)
);

// In a browser this is `await (await fetch(chunkUrl)).arrayBuffer()`; here a
// chunk this same codec produced, so the page can check the round-trip.
const samples = Uint16Array.from({ length: 4 * 32 * 32 }, (_, i) => (i * 7) % 4096);
const chunk = await codec.encode(new Uint8Array(samples.buffer));
const decoded = await codec.decode(chunk); // Uint8Array of little-endian samples
assert.deepEqual(new Uint16Array(decoded.buffer), samples);
```

The codecs follow the [numcodecs.js](https://github.com/manzt/numcodecs.js)
convention: one small JS wrapper per codec plus a lazily-instantiated `.wasm`
module built from the same Rust core as the native codecs, so a chunk written
by Rust or Python decodes here byte-for-byte.

## zarrita.js registration

zarrita's codec pipeline speaks chunk objects rather than bytes, so the
package ships adapters and one call registers them all:

```typescript
import * as zarrita from "zarrita";
import { registerZarritaCodecs } from "@fideus-labs/nd-image-codecs";

registerZarritaCodecs(zarrita.registry);

const store = new Map<string, Uint8Array>();
const arr = await zarrita.create(store, {
  shape: [4, 32, 32],
  chunkShape: [4, 32, 32],
  dtype: "uint16",
  codecs: codecSeries(["z", "y", "x"], [4, 32, 32], "uint16", "nd-lift-ht", {
    xyLevels: 2,
  }),
  fillValue: 0,
});
await zarrita.set(arr, null, {
  data: samples,
  shape: [4, 32, 32],
  stride: [1024, 32, 1],
});

const view = await zarrita.get(await zarrita.open(store, { kind: "array" }));
assert.deepEqual(view.data, samples);
```

`registerZarritaCodecs` also replaces zarrita's `transpose`, `numcodecs.delta`,
and `blosc` entries: the built-in `transpose` corrupts chunks when *writing*
through a permutation, its delta codec refuses the strides a permutation
produces, and its blosc loader passes Zarr v3's string `shuffle` to a
numcodecs.js codec that expects the v2 numeric one. Reading a store written
elsewhere works either way; writing one needs the replacements.

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
