// The packaging contract for the published tarball: the WASM core has to be
// inside it.
//
// `test -f dist/wasm/ndic_zarr_bg.wasm` only proves the build produced the
// file, not that `files` in package.json ships it — and the two came apart
// in practice. `wasm-pack` writes a `.gitignore` containing `*` into its
// out-dir; once that directory is copied into `dist/`, npm honours the
// ignore file it finds inside the package and silently drops every WASM
// artifact from the tarball. Versions 0.0.1 and 0.1.0 published that way:
// they install cleanly and then fail to initialize any codec at run time
// with ERR_MODULE_NOT_FOUND on dist/wasm/ndic_zarr.js.
//
// So the assertion is against the packed file list, never the working tree.
// This lives in a module rather than inline in the release workflow so the
// test suite can cover it and so the check runs on every PR instead of once,
// at the irreversible moment.

import { execFileSync } from "node:child_process";

/** The file whose absence makes an installed package inert. */
export const WASM_CORE = "dist/wasm/ndic_zarr_bg.wasm";

/**
 * The packed paths from `npm pack --json`, across npm's output shapes.
 *
 * npm 11 emits an array of packed packages; npm 12 changed it to an object
 * keyed by package name. The release workflow pins an npm version, so it can
 * cross that boundary on a version bump alone — accept either, and throw on
 * anything else rather than silently reporting on the wrong package.
 */
export function packedPaths(payload) {
  const entries = Array.isArray(payload) ? payload : Object.values(payload ?? {});
  if (entries.length !== 1) {
    throw new Error(`expected npm pack to describe exactly 1 package, got ${entries.length}`);
  }
  const [entry] = entries;
  if (!Array.isArray(entry?.files)) {
    throw new Error("npm pack output has no file list; its JSON shape has changed again");
  }
  return entry.files.map((f) => f.path);
}

/** Pack without writing a tarball, and return the paths it would contain. */
export function packedPathsFor(cwd) {
  const stdout = execFileSync("npm", ["pack", "--dry-run", "--json"], {
    cwd,
    encoding: "utf8",
    // npm's notices go to stderr; stdout is the JSON payload alone.
    stdio: ["ignore", "pipe", "ignore"],
  });
  return packedPaths(JSON.parse(stdout));
}

/**
 * Throw unless the tarball carries the WASM core. Returns the packed paths so
 * a caller can report on them.
 */
export function assertWasmCorePacked(cwd) {
  const paths = packedPathsFor(cwd);
  if (!paths.includes(WASM_CORE)) {
    throw new Error(`${WASM_CORE} is not in the tarball. Packed:\n${paths.join("\n")}`);
  }
  return paths;
}

// Invoked directly (the release workflow and `npm run check:tarball`) rather
// than imported: report and set the exit status.
if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  try {
    const paths = assertWasmCorePacked(process.cwd());
    const wasm = paths.filter((p) => p.endsWith(".wasm"));
    console.log(`tarball carries ${paths.length} files, including ${wasm.join(", ")}`);
  } catch (err) {
    console.error(err.message);
    process.exit(1);
  }
}
