// The htj2k codec: config validation always; WASM round-trips when the
// core has been built (`npm run build:wasm`), skipped cleanly otherwise.

import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { Htj2k } from "../src/index.js";

const wasmBuilt = existsSync(
  fileURLToPath(new URL("../src/wasm/ndic_zarr_bg.wasm", import.meta.url)),
);

describe("Htj2k configuration", () => {
  it("serializes the full configuration with defaults", () => {
    expect(Htj2k.fromConfig({ name: "htj2k" }).toDict()).toEqual({
      name: "htj2k",
      configuration: { xy_levels: 5, reversible: true, progression: "RPCL", index: true },
    });
  });

  it("refuses malformed configurations, matching the Rust codec", () => {
    expect(() =>
      Htj2k.fromConfig({ name: "htj2k", configuration: { progression: "SNAKE" } }),
    ).toThrow(/progression/);
    expect(() => Htj2k.fromConfig({ name: "htj2k", configuration: { xy_levels: 40 } })).toThrow(
      /xy_levels/,
    );
    expect(() => Htj2k.fromConfig({ name: "htj2k" }, { shape: [8], dtype: "uint8" })).toThrow(
      /2D/,
    );
  });

  it("demands chunk meta before coding", async () => {
    await expect(Htj2k.fromConfig({ name: "htj2k" }).encode(new Uint8Array(4))).rejects.toThrow(
      /shape and dtype/,
    );
  });
});

describe.skipIf(!wasmBuilt)("Htj2k WASM core", () => {
  it("round-trips a uint16 chunk and writes the ndht container magic", async () => {
    const shape = [3, 16, 16];
    const n = shape.reduce((a, b) => a * b, 1);
    const samples = Uint16Array.from({ length: n }, (_, i) => (i * 7) % 4096);
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = Htj2k.fromConfig(
      { name: "htj2k", configuration: { xy_levels: 2 } },
      { shape, dtype: "uint16" },
    );
    const chunk = await codec.encode(bytes);
    expect(Array.from(chunk.slice(0, 4))).toEqual([0x6e, 0x64, 0x68, 0x74]); // "ndht"
    expect(await codec.decode(chunk)).toEqual(bytes);
  });

  it("round-trips int32 coefficient planes (the post-nd_lift shape)", async () => {
    const shape = [2, 8, 8];
    const samples = Int32Array.from({ length: 128 }, (_, i) => ((i * 11) % 61) - 30);
    const bytes = new Uint8Array(samples.buffer.slice(0));
    const codec = Htj2k.fromConfig({ name: "htj2k" }, { shape, dtype: "int32" });
    expect(await codec.decode(await codec.encode(bytes))).toEqual(bytes);
  });

  it("reports dtypes without an integer path", async () => {
    const codec = Htj2k.fromConfig({ name: "htj2k" }, { shape: [4, 4], dtype: "float32" });
    await expect(codec.encode(new Uint8Array(64))).rejects.toThrow(/no integer path/);
  });
});
