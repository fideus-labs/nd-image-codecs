/**
 * @fideus-labs/nd-image-codecs — composable Zarr v3 codecs for ND scientific
 * images, backed by the nd-image-codecs Rust core compiled to WebAssembly
 * (wasm32, SIMD128).
 *
 * Three codec families are assembled by {@link codecSeries} from an array's
 * axis metadata:
 *
 * - **nd-delta**  — `transpose → numcodecs.delta → bitshuffle → zstd/lz4`
 * - **nd-lift-ht** — `transpose → nd_lift → htj2k`
 * - **nd-zfp**    — `transpose → reshape → zfp`
 *
 * The individual codec classes follow the numcodecs.js convention (one JS
 * wrapper + one .wasm artifact, https://github.com/manzt/numcodecs.js) with a
 * static `fromConfig` so they register with zarrita.js / zarr.js registries.
 * The `nd_lift`, `htj2k`, and `nd_zfp` WASM cores are live; `codecSeries` is
 * pure TypeScript and is cross-checked against the Rust and Python builders
 * in CI.
 *
 * No component uses JPEG 2000 Part 2 (MCT) syntax; cross-axis decorrelation is
 * the explicit `nd_lift` array-to-array codec.
 */

export type ZarrCodec = { name: string; configuration?: Record<string, unknown> };

export {
  DenseDeltaZarrita,
  Htj2kZarrita,
  NdLiftZarrita,
  NdZfpZarrita,
  ReshapeZarrita,
  registerZarritaCodecs,
  type ZarritaChunk,
  type ZarritaChunkMeta,
} from "./zarrita.js";

// ---------------------------------------------------------------------------
// Registered codec classes
// ---------------------------------------------------------------------------
/**
 * One `nd_lift` decorrelation step (the schema the Rust codec accepts).
 *
 * Optionality mirrors `AxisTransform`'s serde attributes exactly: `dimension`
 * and `kind` are required, while `axis`, `levels`, and `group` are
 * `#[serde(default)]` and may be omitted.
 */
export interface NdLiftTransform {
  /** Axis name (e.g. `"z"`, `"t"`); informational. Defaults to `""`. */
  axis?: string;
  /** Axis index into the post-transpose chunk shape. */
  dimension: number;
  /** Transform kind. */
  kind: "delta" | "haar" | "lift53";
  /** Dyadic decomposition levels (ignored for `delta`; >= 1 for lifting kinds). Defaults to 0. */
  levels?: number;
  /** Group length along the axis (0 = the whole chunk extent). Defaults to 0. */
  group?: number;
}

/** The `nd_lift` configuration version this package implements. */
export const ND_LIFT_VERSION = "0.1";

export interface NdLiftConfig {
  name: "nd_lift";
  /** Omit entirely for the defaults; when present, `version` is required. */
  configuration?: { version: string; transforms?: NdLiftTransform[] };
}

/** The transform fields the Rust and Python validators accept. */
const ND_LIFT_TRANSFORM_FIELDS: ReadonlySet<string> = new Set([
  "axis",
  "dimension",
  "kind",
  "levels",
  "group",
]);

/**
 * The integer transform fields, with the bounds their Rust types impose:
 * `dimension: usize` (required), `levels: u8`, `group: u32` — the latter two
 * `#[serde(default)]`, so absent means 0. `axis` is `#[serde(default)]` too
 * and is informational, so it is optional here as well.
 */
const ND_LIFT_INT_FIELDS = [
  { name: "dimension", max: Number.MAX_SAFE_INTEGER, rust: "usize", required: true },
  { name: "levels", max: 0xff, rust: "u8", required: false },
  { name: "group", max: 0xffff_ffff, rust: "u32", required: false },
] as const;

/**
 * One integer transform field, validated against its Rust type.
 *
 * JSON supplies these, so a numeric string or a fractional value reaches here
 * intact; `("8" ?? 0) < 0` is false, and serde would reject both. Returns the
 * value (0 when absent and optional) so callers can range-check further.
 */
function ndLiftUintField(
  t: Record<string, unknown>,
  field: (typeof ND_LIFT_INT_FIELDS)[number],
): number {
  const raw = t[field.name];
  if (raw === undefined || raw === null) {
    if (field.required) {
      throw new Error(`nd_lift transform is missing required field ${JSON.stringify(field.name)}`);
    }
    return 0;
  }
  if (typeof raw !== "number" || !Number.isInteger(raw) || raw < 0 || raw > field.max) {
    throw new Error(
      `nd_lift transform ${field.name} ${JSON.stringify(raw)} must be >= 0 and ` +
        `<= ${field.max} (${field.rust})`,
    );
  }
  return raw;
}

/** Chunk geometry the nd_lift codec needs beyond its configuration. */
export interface NdLiftChunkMeta {
  /** Chunk shape in stored (post-transpose) order. */
  shape: number[];
  /** Zarr dtype name of the *array* elements, e.g. `"uint16"` (integer dtypes). */
  dtype: string;
}

/**
 * The `nd_lift` Zarr v3 array-to-array codec: explicit cross-axis integer
 * lifting. Serializes exactly the configurations the Rust codec accepts and
 * applies the same validation: the version gate, unknown transform fields,
 * `levels >= 1` for lifting kinds, and a non-negative `group`.
 *
 * `encode` widens little-endian chunk bytes into the decorrelated `int32`
 * (or, for 64-bit dtypes, `int64`) coefficient plane; `decode` inverts it.
 * Backed by the WASM build of the same Rust core as the `zarrs` codec, so
 * every ecosystem produces byte-identical planes. The chunk `shape`/`dtype`
 * come from the second `fromConfig` argument.
 */
export class NdLift {
  static codecName = "nd_lift" as const;

  constructor(
    public readonly config: NdLiftConfig,
    public readonly meta?: NdLiftChunkMeta,
  ) {
    const configuration = config.configuration;
    // Rust's `NdLiftConfig` has no serde default for `version`, so a
    // configuration object read off storage without one is refused there.
    // Refuse it here too instead of assuming this build's semantics. An
    // absent `configuration` is the "all defaults" case, matching
    // `NdLiftConfig::new`.
    // `!= null` on purpose: it catches both `undefined` and `null`, so a JSON
    // `"configuration": null` falls through to the defaults below instead of
    // throwing a TypeError on property access.
    if (configuration != null && configuration.version === undefined) {
      throw new Error(
        `nd_lift configuration must carry an explicit "version" (this build implements ` +
          `${ND_LIFT_VERSION}); refusing rather than mis-decoding`,
      );
    }
    const { version = ND_LIFT_VERSION, transforms = [] } = configuration ?? {};
    // Rust types `version` as `String`, so the JSON number 0.1 is a serde
    // error there; stringifying it here would accept a configuration the
    // codec refuses. The Python `check_version` gate does the same.
    if (typeof version !== "string") {
      throw new Error(
        `nd_lift configuration version must be a string, got ${JSON.stringify(version)}`,
      );
    }
    const [major, minor] = version.split(".");
    if (major !== "0" || minor !== "1") {
      throw new Error(
        `nd_lift configuration version ${JSON.stringify(version)} is not supported by this ` +
          `build (implements ${ND_LIFT_VERSION}); refusing rather than mis-decoding`,
      );
    }
    // This is a deserialization boundary — the values arrive from JSON, so the
    // static types are a claim, not a guarantee. Check the shape before
    // reaching into it, or `for...of`/`Object.keys` throw a bare TypeError
    // instead of a diagnostic the caller can act on.
    if (!Array.isArray(transforms)) {
      throw new Error(`nd_lift transforms must be an array, got ${JSON.stringify(transforms)}`);
    }
    for (const t of transforms) {
      if (t === null || typeof t !== "object" || Array.isArray(t)) {
        throw new Error(`nd_lift transform must be an object, got ${JSON.stringify(t)}`);
      }
      const fields = t as unknown as Record<string, unknown>;
      const unknown = Object.keys(fields)
        .filter((k) => !ND_LIFT_TRANSFORM_FIELDS.has(k))
        .sort();
      if (unknown.length > 0) {
        throw new Error(`nd_lift transform has unknown fields ${JSON.stringify(unknown)}`);
      }
      if (!("kind" in fields)) {
        throw new Error('nd_lift transform is missing required field "kind"');
      }
      if (fields.axis !== undefined && typeof fields.axis !== "string") {
        throw new Error(`nd_lift transform axis ${JSON.stringify(fields.axis)} must be a string`);
      }
      let levels = 0;
      for (const field of ND_LIFT_INT_FIELDS) {
        const value = ndLiftUintField(fields, field);
        if (field.name === "levels") levels = value;
      }
      if (!["delta", "haar", "lift53"].includes(t.kind)) {
        throw new Error(`nd_lift transform kind ${JSON.stringify(t.kind)} is unknown`);
      }
      if (t.kind !== "delta" && levels < 1) {
        throw new Error(`nd_lift transform kind ${JSON.stringify(t.kind)} needs levels >= 1`);
      }
    }
  }

  static fromConfig(config: NdLiftConfig, meta?: NdLiftChunkMeta): NdLift {
    return new NdLift(config, meta);
  }

  /** The Zarr v3 codec metadata object (the schema the Rust codec parses). */
  toDict(): { name: "nd_lift"; configuration: { version: string; transforms: NdLiftTransform[] } } {
    const { version = ND_LIFT_VERSION, transforms = [] } = this.config.configuration ?? {};
    return { name: "nd_lift", configuration: { version, transforms } };
  }

  private chunkMeta(): NdLiftChunkMeta {
    if (this.meta === undefined) {
      throw new Error(
        "nd_lift encode/decode needs the chunk shape and dtype: " +
          "construct with NdLift.fromConfig(config, { shape, dtype })",
      );
    }
    return this.meta;
  }

  async encode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.nd_lift_encode(
      data,
      Uint32Array.from(shape),
      dtype,
      JSON.stringify(this.toDict().configuration),
    );
  }

  async decode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.nd_lift_decode(
      data,
      Uint32Array.from(shape),
      dtype,
      JSON.stringify(this.toDict().configuration),
    );
  }
}

export interface Htj2kConfig {
  name: "htj2k";
  configuration?: {
    xy_levels?: number;
    reversible?: boolean;
    progression?: string;
    index?: boolean;
  };
}

/** Chunk geometry an array-to-bytes codec needs beyond its configuration. */
export interface Htj2kChunkMeta {
  /** Chunk shape in stored (post-transpose) order; trailing dims are (y, x). */
  shape: number[];
  /** Zarr dtype name, e.g. `"uint16"` (integer dtypes up to 32-bit). */
  dtype: string;
}

const HTJ2K_PROGRESSIONS = new Set(["LRCP", "RLCP", "RPCL", "PCRL", "CPRL"]);

// Minimal ambient view of Node's `process`, so the Node-vs-browser probe
// typechecks without @types/node (the package targets browsers first).
declare const process: { versions?: { node?: string } } | undefined;

/** The lazily-initialized WASM core (`npm run build:wasm`). */
let wasmCore: Promise<{
  nd_lift_encode(chunk: Uint8Array, shape: Uint32Array, dtype: string, config: string): Uint8Array;
  nd_lift_decode(chunk: Uint8Array, shape: Uint32Array, dtype: string, config: string): Uint8Array;
  htj2k_encode(chunk: Uint8Array, shape: Uint32Array, dtype: string, config: string): Uint8Array;
  htj2k_decode(chunk: Uint8Array, shape: Uint32Array, dtype: string): Uint8Array;
  nd_zfp_encode(chunk: Uint8Array, shape: Uint32Array, dtype: string, config: string): Uint8Array;
  nd_zfp_decode(chunk: Uint8Array, shape: Uint32Array, dtype: string, config: string): Uint8Array;
}> | null = null;

function loadWasmCore() {
  if (wasmCore === null) {
    // A computed specifier keeps bundlers and tsc from resolving the module
    // statically: the artifact exists only after `npm run build:wasm`.
    const specifier = "./wasm/ndic_zarr.js";
    wasmCore = import(/* @vite-ignore */ /* webpackIgnore: true */ specifier)
      .then(async (mod) => {
        // The wasm-pack `web` target initializes via fetch, which Node
        // cannot do for file: URLs — hand it the bytes there instead.
        if (typeof process !== "undefined" && process?.versions?.node !== undefined) {
          const fsSpecifier = "node:fs/promises";
          const { readFile } = await import(/* @vite-ignore */ fsSpecifier);
          const wasmUrl = new URL("./wasm/ndic_zarr_bg.wasm", import.meta.url);
          await mod.default({ module_or_path: await readFile(wasmUrl) });
        } else {
          await mod.default();
        }
        return mod;
      })
      .catch((cause) => {
        wasmCore = null;
        throw new Error(
          "the nd_lift/htj2k/nd_zfp codecs need the nd-image-codecs WASM core; " +
            "run `npm run build:wasm` (wasm-pack) first",
          { cause },
        );
      });
  }
  return wasmCore;
}

/**
 * The `htj2k` Zarr v3 array-to-bytes codec: one HTJ2K codestream per
 * trailing 2D plane plus the coefficient-plane byte index. Backed by the
 * WASM build of the same Rust core as the `zarrs` codec and the Python
 * extension, so every ecosystem produces byte-identical chunks.
 *
 * `encode`/`decode` operate on little-endian chunk bytes in C order; the
 * chunk `shape`/`dtype` come from the second `fromConfig` argument (an
 * array-to-bytes codec cannot infer them from bytes alone).
 */
export class Htj2k {
  static codecName = "htj2k" as const;

  constructor(
    public readonly config: Htj2kConfig,
    public readonly meta?: Htj2kChunkMeta,
  ) {
    const { xy_levels = 5, progression = "RPCL" } = config.configuration ?? {};
    if (!Number.isInteger(xy_levels) || xy_levels < 0 || xy_levels > 32) {
      throw new Error(`htj2k: xy_levels must be an integer in 0..=32, got ${xy_levels}`);
    }
    if (!HTJ2K_PROGRESSIONS.has(progression)) {
      throw new Error(`htj2k: unknown progression order ${JSON.stringify(progression)}`);
    }
    if (meta !== undefined && meta.shape.length < 2) {
      throw new Error("htj2k needs a chunk with trailing 2D (y, x) planes");
    }
  }

  static fromConfig(config: Htj2kConfig, meta?: Htj2kChunkMeta): Htj2k {
    return new Htj2k(config, meta);
  }

  /** The Zarr v3 codec metadata object (the schema the Rust codec parses). */
  toDict(): Required<Htj2kConfig> {
    const {
      xy_levels = 5,
      reversible = true,
      progression = "RPCL",
      index = true,
    } = this.config.configuration ?? {};
    return { name: "htj2k", configuration: { xy_levels, reversible, progression, index } };
  }

  private chunkMeta(): Htj2kChunkMeta {
    if (this.meta === undefined) {
      throw new Error(
        "htj2k encode/decode needs the chunk shape and dtype: " +
          "construct with Htj2k.fromConfig(config, { shape, dtype })",
      );
    }
    return this.meta;
  }

  async encode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.htj2k_encode(
      data,
      Uint32Array.from(shape),
      dtype,
      JSON.stringify(this.toDict().configuration),
    );
  }

  async decode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.htj2k_decode(data, Uint32Array.from(shape), dtype);
  }
}

export interface NdZfpConfig {
  name: "zfp" | "nd_zfp";
  configuration?: {
    mode?: string;
    rate?: number;
    tolerance?: number;
    precision?: number;
    dims?: number;
  };
}

/** Chunk geometry the nd_zfp codec needs beyond its configuration. */
export interface NdZfpChunkMeta {
  /** Chunk shape in stored (post-transpose) order. */
  shape: number[];
  /** Zarr dtype name, e.g. `"float32"` (ZFP's native types + narrow ints). */
  dtype: string;
}

/** nd_zfp compression modes and the parameter each one takes. */
const ND_ZFP_MODES = new Map<string, "rate" | "tolerance" | "precision" | null>([
  ["reversible", null],
  ["fixed_rate", "rate"],
  ["fixed_accuracy", "tolerance"],
  ["fixed_precision", "precision"],
]);

/**
 * The `zfp` Zarr v3 array-to-bytes codec (the zarr-extensions registered
 * name): chunks compressed as 1-4D ZFP fields with the full ZFP header,
 * byte-identical to the C library's streams. A configuration carrying the
 * legacy `dims` member (data written under the deprecated `nd_zfp` name)
 * selects the old squeeze-and-pad chunk mapping instead. Backed by the
 * WASM build of the same Rust core as the `zarrs` codec and the Python
 * extension, so every ecosystem produces byte-identical chunks.
 *
 * `encode`/`decode` operate on little-endian chunk bytes in C order; the
 * chunk `shape`/`dtype` come from the second `fromConfig` argument (an
 * array-to-bytes codec cannot infer them from bytes alone).
 */
export class NdZfp {
  static codecName = "zfp" as const;

  constructor(
    public readonly config: NdZfpConfig,
    public readonly meta?: NdZfpChunkMeta,
  ) {
    const { mode = "reversible", rate, tolerance, precision, dims } = config.configuration ?? {};
    if (!ND_ZFP_MODES.has(mode)) {
      throw new Error(`zfp: unknown mode ${JSON.stringify(mode)}`);
    }
    const needed = ND_ZFP_MODES.get(mode);
    const params = { rate, tolerance, precision } as const;
    for (const [name, value] of Object.entries(params)) {
      if (name === needed && value === undefined) {
        throw new Error(`zfp: ${mode} mode needs a "${name}"`);
      }
      if (name !== needed && value !== undefined) {
        throw new Error(`zfp: ${JSON.stringify(mode)} mode does not take "${name}"`);
      }
    }
    // Mirror the Rust core's numeric bounds (`ZfpMode::validate`), so a bad
    // configuration fails here rather than inside the WASM call.
    if (rate !== undefined && !(Number.isFinite(rate) && rate > 0)) {
      throw new Error(`zfp: rate must be a positive finite number, got ${rate}`);
    }
    if (tolerance !== undefined && !(Number.isFinite(tolerance) && tolerance >= 0)) {
      throw new Error(`zfp: tolerance must be a non-negative finite number, got ${tolerance}`);
    }
    if (precision !== undefined && !(Number.isInteger(precision) && precision >= 1 && precision <= 64)) {
      throw new Error(`zfp: precision must be an integer in 1..=64, got ${precision}`);
    }
    // Legacy nd_zfp configurations only; the registered `zfp` codec never
    // writes `dims`.
    if (dims !== undefined && (!Number.isInteger(dims) || dims < 1 || dims > 4)) {
      throw new Error(`zfp: dims must be an integer in 1..=4, got ${dims}`);
    }
  }

  static fromConfig(config: NdZfpConfig, meta?: NdZfpChunkMeta): NdZfp {
    return new NdZfp(config, meta);
  }

  /** The Zarr v3 codec metadata object (the schema the Rust codec parses). */
  toDict(): { name: "zfp"; configuration: NonNullable<NdZfpConfig["configuration"]> } {
    const { mode = "reversible", rate, tolerance, precision, dims } = this.config.configuration ?? {};
    const configuration: NonNullable<NdZfpConfig["configuration"]> = { mode };
    if (rate !== undefined) configuration.rate = rate;
    if (tolerance !== undefined) configuration.tolerance = tolerance;
    if (precision !== undefined) configuration.precision = precision;
    if (dims !== undefined) configuration.dims = dims;
    return { name: "zfp", configuration };
  }

  private chunkMeta(): NdZfpChunkMeta {
    if (this.meta === undefined) {
      throw new Error(
        "zfp encode/decode needs the chunk shape and dtype: " +
          "construct with NdZfp.fromConfig(config, { shape, dtype })",
      );
    }
    return this.meta;
  }

  async encode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.nd_zfp_encode(
      data,
      Uint32Array.from(shape),
      dtype,
      JSON.stringify(this.toDict().configuration),
    );
  }

  async decode(data: Uint8Array): Promise<Uint8Array> {
    const { shape, dtype } = this.chunkMeta();
    const core = await loadWasmCore();
    return core.nd_zfp_decode(
      data,
      Uint32Array.from(shape),
      dtype,
      JSON.stringify(this.toDict().configuration),
    );
  }
}

// ---------------------------------------------------------------------------
// codecSeries — mirror of ndic_zarr::series::codec_series
// ---------------------------------------------------------------------------
export type Family = "nd-delta" | "nd-lift-ht" | "nd-zfp";

export interface CodecSeriesOptions {
  decorrelate?: number[];
  addDecorrelate?: number[];
  removeDecorrelate?: number[];
  lift?: "delta" | "haar" | "lift53";
  xyLevels?: number;
  reversible?: boolean;
  deltaBackend?: "zstd" | "lz4";
  zfpRate?: number;
}

const DTYPES: Record<string, [string, number]> = {
  uint8: ["|u1", 1],
  int8: ["|i1", 1],
  uint16: ["<u2", 2],
  int16: ["<i2", 2],
  uint32: ["<u4", 4],
  int32: ["<i4", 4],
  uint64: ["<u8", 8],
  int64: ["<i8", 8],
  float32: ["<f4", 4],
  float64: ["<f8", 8],
};

/**
 * Contiguous input-dimension groups collapsing singletons for `reshape`:
 * one group per non-singleton dimension, each absorbing the singleton
 * dimensions before it; trailing singletons merge into the final group, and
 * an all-singleton shape becomes one group (a 1-element 1D field). Mirrored
 * exactly by the Rust and Python builders.
 */
function reshapeGroups(shape: number[]): number[][] {
  const groups: number[][] = [];
  let current: number[] = [];
  shape.forEach((extent, i) => {
    current.push(i);
    if (extent > 1) {
      groups.push(current);
      current = [];
    }
  });
  if (current.length > 0) {
    if (groups.length > 0) {
      groups[groups.length - 1].push(...current);
    } else {
      groups.push(current);
    }
  }
  return groups;
}

/**
 * Build a Zarr v3 codec pipeline for one nd-image-codecs family. A faithful
 * port of the Rust `ndic_zarr::series::codec_series`; CI asserts the three
 * implementations agree.
 *
 * @param axes Axis identifier per dimension in order, e.g. `["t","c","z","y","x"]`.
 * @param chunkShape Chunk size per dimension (same order as `axes`).
 * @param dtype Zarr v3 data-type name, e.g. `"uint16"`.
 * @param family One of `"nd-delta"`, `"nd-lift-ht"`, `"nd-zfp"`.
 */
export function codecSeries(
  axes: string[],
  chunkShape: number[],
  dtype: string,
  family: Family = "nd-lift-ht",
  opts: CodecSeriesOptions = {},
): ZarrCodec[] {
  const {
    decorrelate,
    addDecorrelate = [],
    removeDecorrelate = [],
    lift = "lift53",
    xyLevels = 5,
    reversible = true,
    deltaBackend = "zstd",
    zfpRate,
  } = opts;

  const ndim = axes.length;
  if (chunkShape.length !== ndim) {
    throw new Error(`${ndim} axes but chunk shape has ${chunkShape.length} entries`);
  }
  const idx = new Map<string, number>();
  axes.forEach((n, i) => idx.set(n, i));
  if (idx.size !== ndim) throw new Error("axis names must be unique");
  const x = idx.get("x");
  const y = idx.get("y");
  if (x === undefined || y === undefined) throw new Error("an 'x' and a 'y' axis are required");
  const z = idx.get("z");
  const t = idx.get("t");
  const dt = DTYPES[dtype];
  if (!dt) throw new Error(`unsupported dtype ${JSON.stringify(dtype)}`);
  const [npDtype, itemsize] = dt;
  if (family === "nd-lift-ht" && npDtype.includes("f") && reversible) {
    throw new Error(`nd-lift-ht reversible coding needs an integer dtype, got ${dtype}`);
  }

  const defaults = [z, t].filter((d): d is number => d !== undefined && chunkShape[d] > 1);
  let decorr: number[];
  if (decorrelate !== undefined) {
    decorr = [...decorrelate];
  } else {
    decorr = [...defaults];
    for (const d of addDecorrelate) if (!decorr.includes(d)) decorr.push(d);
    decorr = decorr.filter((d) => !removeDecorrelate.includes(d));
  }
  for (const d of decorr) {
    if (d >= ndim) throw new Error(`invalid decorrelation dimension ${d}`);
    if (d === x || d === y) {
      throw new Error("the primary spatial axes (x, y) are decorrelated by the 2D codec itself");
    }
  }
  decorr = [...new Set(decorr)].sort((a, b) => a - b);

  const tGrouped = t !== undefined && chunkShape[t] > 1 && decorr.includes(t);
  const trailing: number[] = [];
  if (t !== undefined && tGrouped) trailing.push(t);
  if (z !== undefined) trailing.push(z);
  trailing.push(y, x);
  const extra = decorr.filter((d) => !trailing.includes(d));
  let order = [...Array(ndim).keys()].filter((d) => !trailing.includes(d) && !extra.includes(d));
  order = order.concat(extra, trailing);

  if (family === "nd-delta") {
    const a = [z, t].find((d): d is number => d !== undefined && decorr.includes(d));
    if (a !== undefined) order = order.filter((d) => d !== a).concat(a);
  }

  const codecs: ZarrCodec[] = [];
  const isIdentity = order.every((v, i) => v === i);
  if (!isIdentity) codecs.push({ name: "transpose", configuration: { order } });

  const posOf = (d: number): number => order.indexOf(d);

  if (family === "nd-delta") {
    codecs.push({ name: "numcodecs.delta", configuration: { dtype: npDtype } });
    codecs.push({ name: "bytes", configuration: { endian: "little" } });
    codecs.push({
      name: "blosc",
      configuration: {
        cname: deltaBackend,
        clevel: 5,
        shuffle: "bitshuffle",
        typesize: itemsize,
        blocksize: 0,
      },
    });
  } else if (family === "nd-lift-ht") {
    const transforms = decorr.map((d) => ({
      axis: axes[d],
      dimension: posOf(d),
      kind: lift,
      levels: lift === "delta" ? 0 : 2,
      group: 0,
    }));
    if (transforms.length > 0) {
      codecs.push({ name: "nd_lift", configuration: { version: "0.1", transforms } });
    }
    codecs.push({
      name: "htj2k",
      configuration: { xy_levels: xyLevels, reversible, progression: "RPCL", index: true },
    });
  } else {
    const nonsingleton = chunkShape.filter((c) => c > 1).length;
    if (nonsingleton > 4) {
      throw new Error(`nd-zfp needs <=4 non-singleton chunk dimensions, got ${nonsingleton}`);
    }
    // Collapse singleton dimensions with a `reshape` codec so the chunk
    // reaching `zfp` is a direct 1-4D field, as the registered codec
    // specifies. Groups are contiguous post-transpose dimension indices,
    // one per output dimension; trailing singletons merge into the final
    // group.
    const transposed = order.map((d) => chunkShape[d]);
    if (transposed.some((c) => c === 1)) {
      codecs.push({
        name: "reshape",
        configuration: { shape: reshapeGroups(transposed) },
      });
    }
    const cfg: Record<string, unknown> =
      zfpRate === undefined ? { mode: "reversible" } : { mode: "fixed_rate", rate: zfpRate };
    codecs.push({ name: "zfp", configuration: cfg });
  }

  return codecs;
}
