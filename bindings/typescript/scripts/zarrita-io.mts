/**
 * zarrita.js writer/reader for the cross-ecosystem validation matrix.
 *
 * One corner of the matrix: `write` builds the codec pipeline with
 * the TypeScript `codecSeries` builder, creates a Zarr v3 array through
 * zarrita.js with our WASM codec classes registered, and stores the raw
 * input bytes; `read` decodes any store written by this script,
 * `ndic zarr write` (zarrs), or the zarr-python writer back to raw
 * little-endian C-order element bytes.
 *
 * Run from `bindings/typescript` (so zarrita resolves) with the WASM core
 * built: `npx tsx scripts/zarrita-io.mts write|read ...`. The orchestrator
 * is `scripts/ci/check-cross-validation.py` at the repository root.
 */

import { readFileSync, writeFileSync } from "node:fs";
import * as zarrita from "zarrita";
import { FileSystemStore } from "@zarrita/storage";

import { codecSeries, registerZarritaCodecs, type CodecSeriesOptions } from "../src/index.ts";

registerZarritaCodecs(zarrita.registry);

interface CaseSpec {
  name?: string;
  shape: number[];
  axes: string[];
  chunk_shape: number[];
  dtype: string;
  family: "nd-delta" | "nd-lift-ht" | "nd-zfp";
  options?: Record<string, unknown>;
}

/** snake_case fixture-matrix options → the builder's camelCase options. */
function builderOptions(raw: Record<string, unknown> = {}): CodecSeriesOptions {
  const o: CodecSeriesOptions = {};
  if (raw.decorrelate !== undefined) o.decorrelate = raw.decorrelate as number[];
  if (raw.add_decorrelate !== undefined) o.addDecorrelate = raw.add_decorrelate as number[];
  if (raw.remove_decorrelate !== undefined) o.removeDecorrelate = raw.remove_decorrelate as number[];
  if (raw.lift !== undefined) o.lift = raw.lift as CodecSeriesOptions["lift"];
  if (raw.xy_levels !== undefined) o.xyLevels = raw.xy_levels as number;
  if (raw.reversible !== undefined) o.reversible = raw.reversible as boolean;
  if (raw.delta_backend !== undefined) o.deltaBackend = raw.delta_backend as "zstd" | "lz4";
  if (raw.zfp_rate !== undefined) o.zfpRate = raw.zfp_rate as number;
  return o;
}

const TYPED_ARRAYS: Record<string, any> = {
  uint8: Uint8Array,
  int8: Int8Array,
  uint16: Uint16Array,
  int16: Int16Array,
  uint32: Uint32Array,
  int32: Int32Array,
  uint64: BigUint64Array,
  int64: BigInt64Array,
  float32: Float32Array,
  float64: Float64Array,
};

function parseArgs(argv: string[]): Map<string, string> {
  const args = new Map<string, string>();
  for (let i = 0; i < argv.length; i += 2) {
    if (!argv[i].startsWith("--") || argv[i + 1] === undefined) {
      throw new Error(`expected --flag value pairs, got ${JSON.stringify(argv)}`);
    }
    args.set(argv[i].slice(2), argv[i + 1]);
  }
  return args;
}

function need(args: Map<string, string>, flag: string): string {
  const value = args.get(flag);
  if (value === undefined) throw new Error(`missing --${flag}`);
  return value;
}

async function write(args: Map<string, string>): Promise<void> {
  const spec: CaseSpec = JSON.parse(readFileSync(need(args, "spec"), "utf8"));
  const codecs = codecSeries(
    spec.axes,
    spec.chunk_shape,
    spec.dtype,
    spec.family,
    builderOptions(spec.options),
  );
  const store = new FileSystemStore(need(args, "store"));
  const arr = await zarrita.create(store, {
    shape: spec.shape,
    chunkShape: spec.chunk_shape,
    dtype: spec.dtype as never,
    codecs,
    fillValue: 0,
    dimensionNames: spec.axes,
  });
  const Elem = TYPED_ARRAYS[spec.dtype];
  if (!Elem) throw new Error(`unsupported dtype ${spec.dtype}`);
  const raw = readFileSync(need(args, "input"));
  const n = spec.shape.reduce((a, b) => a * b, 1);
  const buffer = raw.buffer.slice(raw.byteOffset, raw.byteOffset + raw.byteLength);
  const data = new Elem(buffer, 0, n);
  const stride: number[] = new Array(spec.shape.length);
  let step = 1;
  for (let i = spec.shape.length - 1; i >= 0; i--) {
    stride[i] = step;
    step *= spec.shape[i];
  }
  await zarrita.set(arr as never, null, { data, shape: spec.shape, stride } as never);
}

async function read(args: Map<string, string>): Promise<void> {
  const store = new FileSystemStore(need(args, "store"));
  const arr = await zarrita.open(store, { kind: "array" });
  const { data, shape, stride } = (await zarrita.get(arr as never)) as {
    data: any;
    shape: number[];
    stride: number[];
  };
  // Serialize to C order via the strides (zarrita returns chunks in the
  // pipeline's native — possibly transposed — memory order).
  const n = shape.reduce((a: number, b: number) => a * b, 1);
  const out = new (data.constructor as any)(n);
  const index = new Array(shape.length).fill(0);
  for (let c = 0; c < n; c++) {
    let offset = 0;
    for (let d = 0; d < shape.length; d++) offset += index[d] * stride[d];
    out[c] = data[offset];
    for (let d = shape.length - 1; d >= 0; d--) {
      if (++index[d] < shape[d]) break;
      index[d] = 0;
    }
  }
  writeFileSync(need(args, "output"), new Uint8Array(out.buffer, 0, out.byteLength));
}

const [command, ...rest] = process.argv.slice(2);
const args = parseArgs(rest);
if (command === "write") {
  await write(args);
} else if (command === "read") {
  await read(args);
} else {
  throw new Error(`usage: zarrita-io.mts write|read --flag value ... (got ${command})`);
}
