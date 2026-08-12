// Stage the wasm-pack output into `dist/`, where `files` in package.json
// picks it up for the published tarball.
//
// The filter is the whole point. wasm-pack writes a `.gitignore` containing
// `*` into its out-dir, on the assumption that the directory is generated
// and should never be committed — true for the source tree, and fatal once
// the directory is copied into `dist/`: npm honours a `.gitignore` found
// inside the package, so that single line silently excludes every WASM
// artifact from the tarball even though `dist` is listed in `files`. The
// result installs cleanly and then fails to initialize any codec at run
// time. See scripts/tarball.mjs for the assertion that keeps it honest.

import fs from "node:fs";
import path from "node:path";

const FROM = "src/wasm";
const TO = "dist/wasm";

/**
 * Whether a wasm-pack artifact belongs in the published package.
 *
 * Only wasm-pack's generated `.gitignore` is dropped. Everything else it
 * emits is either loaded at run time or consumed by TypeScript, so this
 * excludes by name rather than allow-listing extensions — a new artifact
 * should ship by default, not vanish silently.
 */
export function shouldCopy(src) {
  return path.basename(src) !== ".gitignore";
}

export function copyWasm(from = FROM, to = TO) {
  if (!fs.existsSync(from)) return null;
  // Replace rather than merge. `cpSync` copies over the destination without
  // clearing it, so a `.gitignore` staged by an earlier build survives the
  // filter above and silently restores the bug — which is what an
  // incremental `npm run build` across this change would otherwise do. The
  // directory is reproduced from `from` in full, so removing it is safe;
  // `tsc` writes to `dist/` but never `dist/wasm/`.
  fs.rmSync(to, { recursive: true, force: true });
  fs.cpSync(from, to, { recursive: true, filter: shouldCopy });
  return to;
}

if (process.argv[1] && import.meta.url === `file://${process.argv[1]}`) {
  const staged = copyWasm();
  if (staged === null) {
    console.log(`${FROM} does not exist — skipping (run \`npm run build:wasm\` first).`);
    process.exit(0);
  }
  const core = path.join(staged, "ndic_zarr_bg.wasm");
  if (!fs.existsSync(core)) {
    console.error(`${core} is missing after the copy — the WASM build did not produce a core.`);
    process.exit(1);
  }
  console.log(`staged ${FROM} -> ${staged}`);
}
