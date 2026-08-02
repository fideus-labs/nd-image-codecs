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
 * - **nd-zfp**    — `transpose → nd_zfp`
 *
 * The individual codec classes follow the numcodecs.js convention (one JS
 * wrapper + one .wasm artifact, https://github.com/manzt/numcodecs.js) with a
 * static `fromConfig` so they register with zarrita.js / zarr.js registries.
 * The WASM cores land across roadmap Phases 2–5; `codecSeries` is pure
 * TypeScript and is cross-checked against the Rust and Python builders in CI.
 *
 * No component uses JPEG 2000 Part 2 (MCT) syntax; cross-axis decorrelation is
 * the explicit `nd_lift` array-to-array codec.
 */

export type ZarrCodec = { name: string; configuration?: Record<string, unknown> };

// ---------------------------------------------------------------------------
// Registered codec classes (scaffolds; see roadmap)
// ---------------------------------------------------------------------------
/** One `nd_lift` decorrelation step (the schema the Rust codec accepts). */
export interface NdLiftTransform {
  /** Axis name (e.g. `"z"`, `"t"`); informational. */
  axis: string;
  /** Axis index into the post-transpose chunk shape. */
  dimension: number;
  /** Transform kind. */
  kind: "delta" | "haar" | "lift53";
  /** Dyadic decomposition levels (ignored for `delta`; >= 1 for lifting kinds). */
  levels: number;
  /** Group length along the axis (0 = the whole chunk extent). */
  group: number;
}

/** The `nd_lift` configuration version this package implements. */
export const ND_LIFT_VERSION = "0.1";

export interface NdLiftConfig {
  name: "nd_lift";
  configuration?: { version?: string; transforms?: NdLiftTransform[] };
}

/**
 * Config class for the `nd_lift` Zarr v3 array-to-array codec. Serializes
 * exactly the configurations the Rust codec accepts and applies the same
 * validation (version gate, `levels >= 1` for lifting kinds). The WASM
 * encode/decode core lands with the nd-lift-ht integration (roadmap Phase 4).
 */
export class NdLift {
  static codecName = "nd_lift" as const;

  constructor(public readonly config: NdLiftConfig) {
    const { version = ND_LIFT_VERSION, transforms = [] } = config.configuration ?? {};
    const [major, minor] = String(version).split(".");
    if (major !== "0" || minor !== "1") {
      throw new Error(
        `nd_lift configuration version ${JSON.stringify(version)} is not supported by this ` +
          `build (implements ${ND_LIFT_VERSION}); refusing rather than mis-decoding`,
      );
    }
    for (const t of transforms) {
      if (!["delta", "haar", "lift53"].includes(t.kind)) {
        throw new Error(`nd_lift transform kind ${JSON.stringify(t.kind)} is unknown`);
      }
      if (t.kind !== "delta" && (t.levels ?? 0) < 1) {
        throw new Error(`nd_lift transform kind ${JSON.stringify(t.kind)} needs levels >= 1`);
      }
    }
  }

  static fromConfig(config: NdLiftConfig): NdLift {
    return new NdLift(config);
  }

  /** The Zarr v3 codec metadata object (the schema the Rust codec parses). */
  toDict(): { name: "nd_lift"; configuration: { version: string; transforms: NdLiftTransform[] } } {
    const { version = ND_LIFT_VERSION, transforms = [] } = this.config.configuration ?? {};
    return { name: "nd_lift", configuration: { version, transforms } };
  }

  async encode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("nd_lift encode: the WASM core lands with roadmap Phase 4 (nd-lift-ht)");
  }
  async decode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("nd_lift decode: the WASM core lands with roadmap Phase 4 (nd-lift-ht)");
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

export class Htj2k {
  static codecName = "htj2k" as const;
  constructor(public readonly config: Htj2kConfig) {}
  static fromConfig(config: Htj2kConfig): Htj2k {
    return new Htj2k(config);
  }
  async encode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("htj2k encode: implemented in roadmap Phase 3");
  }
  async decode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("htj2k decode: implemented in roadmap Phase 3");
  }
}

export interface NdZfpConfig {
  name: "nd_zfp";
  configuration?: { mode?: string; rate?: number; dims?: number };
}

export class NdZfp {
  static codecName = "nd_zfp" as const;
  constructor(public readonly config: NdZfpConfig) {}
  static fromConfig(config: NdZfpConfig): NdZfp {
    return new NdZfp(config);
  }
  async encode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("nd_zfp encode: implemented in roadmap Phase 5");
  }
  async decode(_data: Uint8Array): Promise<Uint8Array> {
    throw new Error("nd_zfp decode: implemented in roadmap Phase 5");
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
    const cfg: Record<string, unknown> = {
      mode: zfpRate === undefined ? "reversible" : "fixed_rate",
      dims: Math.max(nonsingleton, 2),
    };
    if (zfpRate !== undefined) cfg.rate = zfpRate;
    codecs.push({ name: "nd_zfp", configuration: cfg });
  }

  return codecs;
}
