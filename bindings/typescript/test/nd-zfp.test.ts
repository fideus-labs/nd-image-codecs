// The nd_zfp codec: config validation always; WASM round-trips when the
// core has been built (`npm run build:wasm`), skipped cleanly otherwise.

import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { NdZfp } from "../src/index.js";

const wasmBuilt = existsSync(
  fileURLToPath(new URL("../src/wasm/ndic_zarr_bg.wasm", import.meta.url)),
);

describe("NdZfp configuration", () => {
  it("serializes the configuration in the builder's field order", () => {
    expect(NdZfp.fromConfig({ name: "nd_zfp" }).toDict()).toEqual({
      name: "nd_zfp",
      configuration: { mode: "reversible", dims: 3 },
    });
    expect(
      NdZfp.fromConfig({
        name: "nd_zfp",
        configuration: { mode: "fixed_rate", rate: 8, dims: 3 },
      }).toDict(),
    ).toEqual({
      name: "nd_zfp",
      configuration: { mode: "fixed_rate", rate: 8, dims: 3 },
    });
  });

  it("refuses malformed configurations, matching the Rust codec", () => {
    expect(() =>
      NdZfp.fromConfig({ name: "nd_zfp", configuration: { mode: "zstd" } }),
    ).toThrow(/unknown mode/);
    expect(() =>
      NdZfp.fromConfig({ name: "nd_zfp", configuration: { mode: "fixed_rate" } }),
    ).toThrow(/rate/);
    expect(() =>
      NdZfp.fromConfig({ name: "nd_zfp", configuration: { mode: "reversible", rate: 8 } }),
    ).toThrow(/rate/);
    expect(() =>
      NdZfp.fromConfig({
        name: "nd_zfp",
        configuration: { mode: "fixed_rate", rate: 8, tolerance: 0.5 },
      }),
    ).toThrow(/tolerance/);
    expect(() => NdZfp.fromConfig({ name: "nd_zfp", configuration: { dims: 5 } })).toThrow(
      /dims/,
    );
    // Numeric bounds mirror the Rust core's ZfpMode::validate.
    expect(() =>
      NdZfp.fromConfig({ name: "nd_zfp", configuration: { mode: "fixed_rate", rate: 0 } }),
    ).toThrow(/positive finite/);
    expect(() =>
      NdZfp.fromConfig({ name: "nd_zfp", configuration: { mode: "fixed_rate", rate: NaN } }),
    ).toThrow(/positive finite/);
    expect(() =>
      NdZfp.fromConfig({
        name: "nd_zfp",
        configuration: { mode: "fixed_accuracy", tolerance: -1 },
      }),
    ).toThrow(/non-negative finite/);
    expect(() =>
      NdZfp.fromConfig({
        name: "nd_zfp",
        configuration: { mode: "fixed_precision", precision: 65 },
      }),
    ).toThrow(/1\.\.=64/);
    expect(() =>
      NdZfp.fromConfig({
        name: "nd_zfp",
        configuration: { mode: "fixed_precision", precision: 0 },
      }),
    ).toThrow(/1\.\.=64/);
  });

  it("demands chunk meta before coding", async () => {
    await expect(NdZfp.fromConfig({ name: "nd_zfp" }).encode(new Uint8Array(4))).rejects.toThrow(
      /shape and dtype/,
    );
  });
});

describe.skipIf(!wasmBuilt)("NdZfp WASM core", () => {
  it("round-trips a float32 chunk reversibly and writes the zfp magic", async () => {
    const shape = [4, 8, 8];
    const n = shape.reduce((a, b) => a * b, 1);
    const samples = Float32Array.from({ length: n }, (_, i) => ((i * 7) % 4096) / 3);
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = NdZfp.fromConfig(
      { name: "nd_zfp", configuration: { mode: "reversible", dims: 3 } },
      { shape, dtype: "float32" },
    );
    const chunk = await codec.encode(bytes);
    expect(Array.from(chunk.slice(0, 4))).toEqual([0x7a, 0x66, 0x70, 0x05]); // "zfp" + codec 5
    expect(await codec.decode(chunk)).toEqual(bytes);
  });

  it("round-trips int16 through the promoted integer path", async () => {
    const shape = [4, 8, 8];
    const samples = Int16Array.from({ length: 256 }, (_, i) => ((i * 11) % 4001) - 2000);
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = NdZfp.fromConfig(
      { name: "nd_zfp", configuration: { mode: "reversible", dims: 3 } },
      { shape, dtype: "int16" },
    );
    expect(await codec.decode(await codec.encode(bytes))).toEqual(bytes);
  });

  it("compresses fixed-rate chunks to the computed size", async () => {
    const shape = [8, 8];
    const samples = Float64Array.from({ length: 64 }, (_, i) => i / 7);
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = NdZfp.fromConfig(
      { name: "nd_zfp", configuration: { mode: "fixed_rate", rate: 8, dims: 2 } },
      { shape, dtype: "float64" },
    );
    const chunk = await codec.encode(bytes);
    // Header (96 bits) + 4 blocks × 128 bits, word-padded.
    expect(chunk.length).toBe(80);
    expect((await codec.decode(chunk)).length).toBe(bytes.length);
  });

  it("reports dtypes without a ZFP path", async () => {
    const codec = NdZfp.fromConfig({ name: "nd_zfp" }, { shape: [4, 4], dtype: "uint32" });
    await expect(codec.encode(new Uint8Array(64))).rejects.toThrow(/no path/);
  });

  it("encodes the committed micro-fixture byte-for-byte", async () => {
    // The Rust core and the Python extension pin the same file
    // (`fixtures/zfp/tiny-chunk-4x8x8-rate8.zfp`), so this is the
    // TypeScript corner of the cross-ecosystem byte-identity gate.
    const shape = [4, 8, 8];
    const n = shape.reduce((a, b) => a * b, 1);
    const samples = Float32Array.from({ length: n }, (_, i) => Math.fround(((i * 7) % 4096) / 3));
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = NdZfp.fromConfig(
      { name: "nd_zfp", configuration: { mode: "fixed_rate", rate: 8, dims: 3 } },
      { shape, dtype: "float32" },
    );
    const committed = readFileSync(
      fileURLToPath(new URL("../../../fixtures/zfp/tiny-chunk-4x8x8-rate8.zfp", import.meta.url)),
    );
    expect(await codec.encode(bytes)).toEqual(new Uint8Array(committed));
    expect((await codec.decode(new Uint8Array(committed))).length).toBe(bytes.length);
  });
});
