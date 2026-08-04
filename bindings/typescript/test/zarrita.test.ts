// The zarrita.js adapters: full store round-trips through zarrita's own
// pipeline (create → set → get) for each codec family, exercising the
// registered adapter classes — including the transpose and delta
// replacements — against an in-memory store. WASM-backed families skip
// cleanly when the core has not been built.

import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import * as zarrita from "zarrita";

import { codecSeries, registerZarritaCodecs, type Family } from "../src/index.ts";

const wasmBuilt = existsSync(
  fileURLToPath(new URL("../src/wasm/ndic_zarr_bg.wasm", import.meta.url)),
);

registerZarritaCodecs(zarrita.registry);

/** Round-trip `data` through a zarrita store with a codec-series pipeline. */
async function roundtrip(
  family: Family,
  dtype: string,
  data: { length: number },
  options: Record<string, unknown> = {},
): Promise<{ data: Record<number, unknown>; shape: number[]; stride: number[] }> {
  const axes = ["z", "c", "y", "x"];
  const shape = [4, 2, 8, 8];
  const chunkShape = [2, 1, 8, 8];
  const codecs = codecSeries(axes, chunkShape, dtype, family, options);
  const store = new Map<string, Uint8Array>();
  const arr = await zarrita.create(store, {
    shape,
    chunkShape,
    dtype: dtype as never,
    codecs,
    fillValue: 0,
  });
  const stride = [128, 64, 8, 1];
  await zarrita.set(arr as never, null, { data, shape, stride } as never);
  return (await zarrita.get(await zarrita.open(store, { kind: "array" }))) as never;
}

/** Read a strided zarrita result in C order. */
function cOrder(result: { data: Record<number, unknown>; shape: number[]; stride: number[] }) {
  const n = result.shape.reduce((a, b) => a * b, 1);
  const out: unknown[] = new Array(n);
  const index = new Array(result.shape.length).fill(0);
  for (let c = 0; c < n; c++) {
    let offset = 0;
    for (let d = 0; d < result.shape.length; d++) offset += index[d] * result.stride[d];
    out[c] = result.data[offset];
    for (let d = result.shape.length - 1; d >= 0; d--) {
      if (++index[d] < result.shape[d]) break;
      index[d] = 0;
    }
  }
  return out;
}

describe("zarrita adapters (nd-delta, pure JS)", () => {
  it("round-trips a transposed + delta + blosc store", async () => {
    const data = Uint16Array.from({ length: 512 }, (_, i) => (i * 7) % 4000);
    const result = await roundtrip("nd-delta", "uint16", data);
    expect(cOrder(result)).toEqual([...data]);
  });

  it("round-trips 64-bit deltas through BigInt", async () => {
    const data = BigUint64Array.from({ length: 512 }, (_, i) => BigInt(i) * 3_000_000_000n);
    const result = await roundtrip("nd-delta", "uint64", data);
    expect(cOrder(result)).toEqual([...data]);
  });
});

describe.skipIf(!wasmBuilt)("zarrita adapters (WASM families)", () => {
  it("round-trips an nd-lift-ht store", async () => {
    const data = Uint16Array.from({ length: 512 }, (_, i) => (i * 11) % 4000);
    const result = await roundtrip("nd-lift-ht", "uint16", data, { xyLevels: 2 });
    expect(cOrder(result)).toEqual([...data]);
  });

  it("round-trips an nd-zfp store reversibly", async () => {
    const data = Float32Array.from({ length: 512 }, (_, i) => Math.fround(i / 7 - 30));
    const result = await roundtrip("nd-zfp", "float32", data);
    expect(cOrder(result)).toEqual([...data]);
  });
});
