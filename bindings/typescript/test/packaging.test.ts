// The packaging contract: the WASM core has to reach the published tarball.
//
// This is regression cover for a bug that shipped twice. `npm pack` honours a
// `.gitignore` found inside the package, and wasm-pack writes one containing
// `*` into its out-dir; copying that directory into `dist/` verbatim excluded
// every WASM artifact from the tarball while leaving the working tree — and
// so every `test -f` check — perfectly healthy. 0.0.1 and 0.1.0 both went out
// carrying no WASM at all.
//
// The tarball assertion used to live only in the release workflow, inline in
// YAML, where it ran once per release and could not be tested. These tests
// cover it here so it runs on every PR instead.

import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

import { copyWasm, shouldCopy } from "../scripts/copy-wasm.mjs";
import { WASM_CORE, assertWasmCorePacked, packedPaths } from "../scripts/tarball.mjs";

const pkgRoot = fileURLToPath(new URL("..", import.meta.url));
const distBuilt = existsSync(join(pkgRoot, "dist", "wasm", "ndic_zarr_bg.wasm"));

/** A packed-file entry as npm reports it; only `path` is read. */
const entry = (path: string) => ({ path, size: 1, mode: 420 });

const scratch: string[] = [];
function tempDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "ndic-pack-"));
  scratch.push(dir);
  return dir;
}
afterEach(() => {
  while (scratch.length) rmSync(scratch.pop()!, { recursive: true, force: true });
});

describe("npm pack output shapes", () => {
  // npm 11 emitted an array; npm 12 emits an object keyed by package name.
  // Reading `payload[0].files` crashed the release job on the version bump
  // alone, so both shapes are pinned here.
  it("reads the npm 11 array shape", () => {
    const payload = [{ name: "@fideus-labs/nd-image-codecs", files: [entry(WASM_CORE)] }];
    expect(packedPaths(payload)).toEqual([WASM_CORE]);
  });

  it("reads the npm 12 name-keyed object shape", () => {
    const payload = {
      "@fideus-labs/nd-image-codecs": {
        name: "@fideus-labs/nd-image-codecs",
        files: [entry("package.json"), entry(WASM_CORE)],
      },
    };
    expect(packedPaths(payload)).toEqual(["package.json", WASM_CORE]);
  });

  it("preserves the packed order and reports every path", () => {
    const paths = ["README.md", "dist/index.js", WASM_CORE];
    expect(packedPaths([{ files: paths.map(entry) }])).toEqual(paths);
  });

  // Refusing rather than guessing: reporting on the wrong package, or on no
  // package, would let the check pass while the tarball is broken.
  it("refuses a payload describing no packages", () => {
    expect(() => packedPaths([])).toThrow(/exactly 1 package, got 0/);
    expect(() => packedPaths({})).toThrow(/exactly 1 package, got 0/);
  });

  it("refuses a payload describing more than one package", () => {
    expect(() => packedPaths([{ files: [] }, { files: [] }])).toThrow(/exactly 1 package, got 2/);
  });

  it("refuses a shape with no file list at all", () => {
    expect(() => packedPaths([{ name: "x" }])).toThrow(/JSON shape has changed/);
    expect(() => packedPaths({ x: { name: "x" } })).toThrow(/JSON shape has changed/);
  });
});

describe("staging the wasm-pack output", () => {
  it("drops wasm-pack's generated .gitignore", () => {
    // The entire bug: this one file, copied into the package, makes npm
    // exclude everything beside it.
    expect(shouldCopy("/build/src/wasm/.gitignore")).toBe(false);
    expect(shouldCopy(".gitignore")).toBe(false);
  });

  it("keeps every other wasm-pack artifact", () => {
    for (const name of [
      "ndic_zarr_bg.wasm",
      "ndic_zarr_bg.wasm.d.ts",
      "ndic_zarr.js",
      "ndic_zarr.d.ts",
      "package.json",
      "snippets/inline0.js",
    ]) {
      expect(shouldCopy(`/build/src/wasm/${name}`)).toBe(true);
    }
  });

  it("does not mistake a file merely ending in .gitignore for one", () => {
    expect(shouldCopy("/build/src/wasm/not-a.gitignore")).toBe(true);
  });

  it("copies the core across while leaving the .gitignore behind", () => {
    const root = tempDir();
    const from = join(root, "src", "wasm");
    mkdirSync(from, { recursive: true });
    writeFileSync(join(from, ".gitignore"), "*\n");
    writeFileSync(join(from, "ndic_zarr_bg.wasm"), "\0asm");
    writeFileSync(join(from, "ndic_zarr.js"), "export default 1;\n");

    const to = join(root, "dist", "wasm");
    expect(copyWasm(from, to)).toBe(to);
    expect(existsSync(join(to, "ndic_zarr_bg.wasm"))).toBe(true);
    expect(existsSync(join(to, "ndic_zarr.js"))).toBe(true);
    expect(existsSync(join(to, ".gitignore"))).toBe(false);
  });

  it("clears a .gitignore left in the destination by an earlier build", () => {
    // `cpSync` merges into the destination rather than replacing it, so a
    // pre-fix `dist/wasm/.gitignore` would outlive the filter and restore the
    // bug on any incremental `npm run build`.
    const root = tempDir();
    const from = join(root, "src", "wasm");
    mkdirSync(from, { recursive: true });
    writeFileSync(join(from, "ndic_zarr_bg.wasm"), "\0asm");

    const to = join(root, "dist", "wasm");
    mkdirSync(to, { recursive: true });
    writeFileSync(join(to, ".gitignore"), "*\n");

    copyWasm(from, to);
    expect(existsSync(join(to, ".gitignore"))).toBe(false);
    expect(existsSync(join(to, "ndic_zarr_bg.wasm"))).toBe(true);
  });

  it("drops artifacts an earlier build staged and the current one does not", () => {
    // The same replace-don't-merge property, for a renamed core: a stale file
    // shipping alongside the real one is how a tarball goes subtly wrong.
    const root = tempDir();
    const from = join(root, "src", "wasm");
    mkdirSync(from, { recursive: true });
    writeFileSync(join(from, "ndic_zarr_bg.wasm"), "\0asm");

    const to = join(root, "dist", "wasm");
    mkdirSync(to, { recursive: true });
    writeFileSync(join(to, "ndic_zarr_old_bg.wasm"), "\0asm");

    copyWasm(from, to);
    expect(existsSync(join(to, "ndic_zarr_old_bg.wasm"))).toBe(false);
    expect(existsSync(join(to, "ndic_zarr_bg.wasm"))).toBe(true);
  });

  it("reports a missing source tree instead of throwing", () => {
    // `npm run build` runs before the WASM core exists in some flows; that is
    // a skip, not a failure.
    expect(copyWasm(join(tempDir(), "absent"), join(tempDir(), "out"))).toBeNull();
  });
});

// The end-to-end contract, against the real package. Needs `npm run build`,
// which the CI TypeScript job and the release job both run first.
describe.skipIf(!distBuilt)("the packed tarball", () => {
  it("carries the WASM core", () => {
    const paths = assertWasmCorePacked(pkgRoot);
    expect(paths).toContain(WASM_CORE);
  });

  it("carries no .gitignore that would empty it out", () => {
    const paths = assertWasmCorePacked(pkgRoot);
    expect(paths.filter((p) => p.endsWith(".gitignore"))).toEqual([]);
  });

  it("carries the loader beside the core, so the module resolves", () => {
    // ERR_MODULE_NOT_FOUND on this file is how the shipped bug surfaced.
    expect(assertWasmCorePacked(pkgRoot)).toContain("dist/wasm/ndic_zarr.js");
  });
}, 60_000);
