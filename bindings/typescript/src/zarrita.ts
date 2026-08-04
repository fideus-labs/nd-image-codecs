/**
 * zarrita.js adapters for the nd-image-codecs codec classes.
 *
 * zarrita's codec pipeline speaks *chunk objects* (`{ data, shape, stride }`)
 * and classifies codecs by a `kind` property, while the classes in
 * `index.ts` follow the numcodecs.js byte-level convention. These adapters
 * bridge the two so a zarrita array whose metadata names `nd_lift`, `htj2k`,
 * or `nd_zfp` decodes (and encodes) through the same WASM core as every
 * other ecosystem:
 *
 * ```ts
 * import * as zarrita from "zarrita";
 * import { registerZarritaCodecs } from "@fideus-labs/nd-image-codecs";
 *
 * registerZarritaCodecs(zarrita.registry);
 * const arr = await zarrita.open(store, { kind: "array" });
 * ```
 *
 * The adapters are structural — nothing here imports zarrita, so the
 * package keeps zero runtime dependencies.
 *
 * A note on shapes: zarrita threads the *pre-transpose* chunk shape through
 * codec construction and converts `transpose` by reordering chunk memory
 * (strides), not by permuting the metadata shape. The WASM cores want the
 * *stored* (post-transpose) shape — `htj2k`'s trailing dims are `(y, x)`
 * and `nd_lift` transform `dimension` indices are post-transpose — so each
 * adapter permutes the metadata shape by the pipeline's `transpose` order,
 * and mirrors zarrita's own `bytes` codec in trusting that chunk memory
 * reaching an array-to-bytes codec is already in stored order.
 */

import { Htj2k, NdLift, NdZfp } from "./index.js";
import type { Htj2kConfig, NdLiftConfig, NdZfpConfig, ZarrCodec } from "./index.js";

/** The chunk object zarrita's codec pipeline passes between codecs. */
export interface ZarritaChunk {
  data: {
    buffer: ArrayBufferLike;
    byteOffset: number;
    byteLength: number;
    length: number;
  };
  shape: number[];
  stride: number[];
}

/** The metadata zarrita hands a codec's `fromConfig` (its `currentMeta`). */
export interface ZarritaChunkMeta {
  /** Zarr data-type name as seen *before* this codec, e.g. `"uint16"`. */
  dataType: string;
  /** Chunk shape in pre-transpose (array) dimension order. */
  shape: number[];
  /** The full codec list of the pipeline, for locating `transpose`. */
  codecs?: ZarrCodec[];
  fillValue?: unknown;
}

/** A typed-array constructor per Zarr data-type name. */
const TYPED_ARRAYS: Record<string, new (buffer: ArrayBufferLike, byteOffset?: number, length?: number) => ZarritaChunk["data"]> = {
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

function typedArrayFor(dataType: string): (typeof TYPED_ARRAYS)[string] {
  const ctor = TYPED_ARRAYS[dataType];
  if (!ctor) throw new Error(`nd-image-codecs: unsupported data type ${JSON.stringify(dataType)}`);
  return ctor;
}

/** The stored (post-transpose) chunk shape: metadata shape permuted by the
 * pipeline's `transpose` order, identity when there is none. */
function storedShape(meta: ZarritaChunkMeta): number[] {
  const transpose = (meta.codecs ?? []).find((c) => c.name === "transpose");
  const order = transpose?.configuration?.order;
  if (order === undefined || order === "C") return [...meta.shape];
  if (order === "F") return [...meta.shape].reverse();
  if (!Array.isArray(order)) {
    throw new Error(`nd-image-codecs: unsupported transpose order ${JSON.stringify(order)}`);
  }
  return (order as number[]).map((d) => meta.shape[d]);
}

/** Chunk memory as bytes — the same trust zarrita's `bytes` codec places in
 * the pipeline having already arranged stored order. */
function chunkBytes(chunk: ZarritaChunk): Uint8Array {
  return new Uint8Array(chunk.data.buffer, chunk.data.byteOffset, chunk.data.byteLength);
}

/** C-order strides for a shape (zarrita's convention for decoded chunks). */
function cStrides(shape: number[]): number[] {
  const stride = new Array<number>(shape.length);
  let step = 1;
  for (let i = shape.length - 1; i >= 0; i--) {
    stride[i] = step;
    step *= shape[i];
  }
  return stride;
}

/** `nd_lift` for zarrita: array-to-array, widening the data type to its
 * coefficient plane (`int32`/`int64`), which `getEncodedMeta` reports to the
 * codecs downstream. */
export class NdLiftZarrita {
  readonly kind = "array_to_array" as const;
  readonly #codec: NdLift;
  readonly #meta: ZarritaChunkMeta;
  readonly #planeType: "int32" | "int64";

  constructor(config: NdLiftConfig["configuration"], meta: ZarritaChunkMeta) {
    this.#meta = meta;
    this.#planeType = meta.dataType === "uint64" || meta.dataType === "int64" ? "int64" : "int32";
    this.#codec = NdLift.fromConfig(
      { name: "nd_lift", configuration: config },
      { shape: storedShape(meta), dtype: meta.dataType },
    );
  }

  static fromConfig(config: NdLiftConfig["configuration"], meta: ZarritaChunkMeta): NdLiftZarrita {
    return new NdLiftZarrita(config, meta);
  }

  /** The plane dtype downstream codecs (e.g. `htj2k`) must be built for. */
  getEncodedMeta(meta: ZarritaChunkMeta): ZarritaChunkMeta {
    return { ...meta, dataType: this.#planeType };
  }

  async encode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    const plane = await this.#codec.encode(chunkBytes(chunk));
    const Plane = typedArrayFor(this.#planeType);
    return { data: new Plane(plane.buffer, plane.byteOffset, plane.byteLength / (this.#planeType === "int64" ? 8 : 4)), shape: chunk.shape, stride: chunk.stride };
  }

  async decode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    const bytes = await this.#codec.decode(chunkBytes(chunk));
    const Elem = typedArrayFor(this.#meta.dataType);
    const itemsize = bytes.byteLength / chunk.data.length;
    return {
      data: new Elem(bytes.buffer, bytes.byteOffset, bytes.byteLength / itemsize),
      shape: chunk.shape,
      stride: chunk.stride,
    };
  }
}

/** Shared shape of the two array-to-bytes adapters. */
abstract class ArrayBytesAdapter {
  readonly kind = "array_to_bytes" as const;
  protected readonly meta: ZarritaChunkMeta;
  protected abstract codec: { encode(data: Uint8Array): Promise<Uint8Array>; decode(data: Uint8Array): Promise<Uint8Array> };

  constructor(meta: ZarritaChunkMeta) {
    this.meta = meta;
  }

  async encode(chunk: ZarritaChunk): Promise<Uint8Array> {
    return this.codec.encode(chunkBytes(chunk));
  }

  /** Mirror zarrita's `bytes` codec: label the decoded buffer with the
   * pre-transpose metadata shape and C strides; `transpose` (when present)
   * reorders it upstream. */
  async decode(bytes: Uint8Array): Promise<ZarritaChunk> {
    const decoded = await this.codec.decode(bytes);
    const Elem = typedArrayFor(this.meta.dataType);
    const n = this.meta.shape.reduce((a, b) => a * b, 1);
    return {
      data: new Elem(decoded.buffer, decoded.byteOffset, n),
      shape: [...this.meta.shape],
      stride: cStrides(this.meta.shape),
    };
  }
}

/** `htj2k` for zarrita: array-to-bytes over the WASM chunk core. */
export class Htj2kZarrita extends ArrayBytesAdapter {
  protected codec: Htj2k;

  constructor(config: Htj2kConfig["configuration"], meta: ZarritaChunkMeta) {
    super(meta);
    this.codec = Htj2k.fromConfig(
      { name: "htj2k", configuration: config },
      { shape: storedShape(meta), dtype: meta.dataType },
    );
  }

  static fromConfig(config: Htj2kConfig["configuration"], meta: ZarritaChunkMeta): Htj2kZarrita {
    return new Htj2kZarrita(config, meta);
  }
}

/** `nd_zfp` for zarrita: array-to-bytes over the WASM chunk core. */
export class NdZfpZarrita extends ArrayBytesAdapter {
  protected codec: NdZfp;

  constructor(config: NdZfpConfig["configuration"], meta: ZarritaChunkMeta) {
    super(meta);
    this.codec = NdZfp.fromConfig(
      { name: "nd_zfp", configuration: config },
      { shape: storedShape(meta), dtype: meta.dataType },
    );
  }

  static fromConfig(config: NdZfpConfig["configuration"], meta: ZarritaChunkMeta): NdZfpZarrita {
    return new NdZfpZarrita(config, meta);
  }
}

/**
 * `numcodecs.delta` over dense chunk memory, replacing zarrita's built-in
 * delta codec, which refuses the permuted strides a non-trivial `transpose`
 * produces — so zarrita cannot *write* the nd-delta family's
 * `transpose → numcodecs.delta` pipelines with its own implementation.
 *
 * numcodecs flattens with `reshape(-1, order="A")` — memory order — so for
 * the dense chunks zarrita's pipeline passes (fresh buffers from `set` or
 * the transpose conversion), a linear walk over the underlying buffer is
 * exactly numcodecs' semantics whatever the stride labels say. Integer
 * wrap-around matches NumPy's modular arithmetic because every partial
 * result is stored into (and re-read from) the typed array.
 */
export class DenseDeltaZarrita {
  readonly kind = "array_to_array" as const;
  readonly #meta: ZarritaChunkMeta;

  constructor(meta: ZarritaChunkMeta) {
    this.#meta = meta;
    typedArrayFor(meta.dataType); // validate early
  }

  static fromConfig(_config: unknown, meta: ZarritaChunkMeta): DenseDeltaZarrita {
    return new DenseDeltaZarrita(meta);
  }

  async encode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    const Elem = typedArrayFor(this.#meta.dataType);
    const src = chunk.data as unknown as Record<number, never> & { length: number };
    const out = new Elem(new ArrayBuffer(chunk.data.byteLength), 0, chunk.data.length) as
      unknown as Record<number, unknown> & { length: number };
    if (src.length > 0) out[0] = src[0];
    for (let i = 1; i < src.length; i++) {
      // BigInt for the 64-bit dtypes, number otherwise — same-typed pairs.
      out[i] = (src[i] as never) - (src[i - 1] as never);
    }
    return { data: out as never, shape: chunk.shape, stride: chunk.stride };
  }

  async decode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    const Elem = typedArrayFor(this.#meta.dataType);
    const src = chunk.data as unknown as Record<number, never> & { length: number };
    const out = new Elem(new ArrayBuffer(chunk.data.byteLength), 0, chunk.data.length) as
      unknown as Record<number, unknown> & { length: number };
    if (src.length > 0) out[0] = src[0];
    for (let i = 1; i < src.length; i++) {
      // Read the previous partial back out of the typed array so the
      // accumulator wraps to the dtype at every step, like NumPy's cumsum.
      out[i] = (out[i - 1] as never) + (src[i] as never);
    }
    return { data: out as never, shape: chunk.shape, stride: chunk.stride };
  }
}

/**
 * Register the codecs into a zarrita registry (`zarrita.registry`, or any
 * map with the same `set` shape): the three nd-image-codecs codecs, plus a
 * `numcodecs.delta` replacement that handles the permuted strides zarrita's
 * built-in delta refuses under `transpose` (see {@link DenseDeltaZarrita}).
 */
/**
 * `transpose` replacement for zarrita, whose built-in codec cannot *encode*
 * through a real permutation: its dense-copy path (`emptyLike`) constructs
 * the output from `chunk.constructor` — the plain chunk object — instead of
 * `chunk.data.constructor`, so every write through `transpose` produces
 * garbage (reads are unaffected: the decode direction only relabels
 * strides). This implementation keeps zarrita's conventions exactly — the
 * chunk `shape` stays in array order and the memory/stride carry the stored
 * order — with a correct dense reorder on encode.
 */
export class TransposeZarrita {
  readonly kind = "array_to_array" as const;
  /** Stored dimension `i` is source dimension `order[i]`. */
  readonly #order: number[];

  constructor(config: { order?: "C" | "F" | number[] } | undefined, meta: ZarritaChunkMeta) {
    const value = config?.order ?? "C";
    const rank = meta.shape.length;
    if (value === "C") {
      this.#order = [...Array(rank).keys()];
    } else if (value === "F") {
      this.#order = [...Array(rank).keys()].reverse();
    } else {
      const seen = new Set(value);
      if (value.length !== rank || seen.size !== rank || value.some((d) => d < 0 || d >= rank)) {
        throw new Error(`transpose: invalid permutation ${JSON.stringify(value)}`);
      }
      this.#order = [...value];
    }
  }

  static fromConfig(
    config: { order?: "C" | "F" | number[] } | undefined,
    meta: ZarritaChunkMeta,
  ): TransposeZarrita {
    return new TransposeZarrita(config, meta);
  }

  /** Stride labels for stored-C-order memory, in source dimension space. */
  #storedStride(shape: number[]): number[] {
    const stride = new Array<number>(shape.length);
    let step = 1;
    for (let i = this.#order.length - 1; i >= 0; i--) {
      stride[this.#order[i]] = step;
      step *= shape[this.#order[i]];
    }
    return stride;
  }

  async encode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    const rank = chunk.shape.length;
    const n = chunk.shape.reduce((a, b) => a * b, 1);
    const src = chunk.data as unknown as Record<number, unknown> & {
      constructor: new (n: number) => ZarritaChunk["data"];
    };
    const out = new src.constructor(n) as unknown as Record<number, unknown>;
    // Walk the stored (transposed C) order; read the source through its own
    // strides, so dense chunks and strided views both reorder correctly.
    const storedShape = this.#order.map((d) => chunk.shape[d]);
    const srcStride = this.#order.map((d) => chunk.stride[d]);
    const index = new Array<number>(rank).fill(0);
    for (let s = 0; s < n; s++) {
      let offset = 0;
      for (let d = 0; d < rank; d++) offset += index[d] * srcStride[d];
      out[s] = src[offset];
      for (let d = rank - 1; d >= 0; d--) {
        if (++index[d] < storedShape[d]) break;
        index[d] = 0;
      }
    }
    return {
      data: out as never,
      shape: chunk.shape,
      stride: this.#storedStride(chunk.shape),
    };
  }

  async decode(chunk: ZarritaChunk): Promise<ZarritaChunk> {
    // Mirror zarrita: the stored memory is already what we want — relabel
    // the strides to say so.
    return {
      data: chunk.data,
      shape: chunk.shape,
      stride: this.#storedStride(chunk.shape),
    };
  }
}

/** Zarr v3 `shuffle` strings → the numeric codes numcodecs.js expects. */
const BLOSC_SHUFFLE: Record<string, number> = {
  noshuffle: 0,
  shuffle: 1,
  bitshuffle: 2,
};

export function registerZarritaCodecs(registry: {
  set(name: string, loader: () => unknown): unknown;
}): void {
  registry.set("nd_lift", () => NdLiftZarrita);
  registry.set("htj2k", () => Htj2kZarrita);
  registry.set("nd_zfp", () => NdZfpZarrita);
  registry.set("numcodecs.delta", () => DenseDeltaZarrita);
  registry.set("transpose", () => TransposeZarrita);
  registry.set("blosc", async () => {
    // zarrita's own loader hands numcodecs.js the Zarr v3 configuration
    // verbatim, but numcodecs.js Blosc expects the v2 numeric `shuffle`; the
    // v3 string sails through its range check (NaN comparisons) into the
    // WASM compressor and corrupts the frame. Same codec, with the
    // configuration translated. The computed specifier keeps bundlers from
    // resolving numcodecs statically — it is zarrita's dependency, present
    // wherever zarrita is.
    const specifier = "numcodecs/blosc";
    const { default: Blosc } = (await import(/* @vite-ignore */ specifier)) as {
      default: { fromConfig(config: Record<string, unknown>): unknown };
    };
    return {
      fromConfig(config: Record<string, unknown>): unknown {
        const shuffle =
          typeof config.shuffle === "string" ? BLOSC_SHUFFLE[config.shuffle] : config.shuffle;
        if (shuffle === undefined) {
          throw new Error(`blosc: unknown shuffle ${JSON.stringify(config.shuffle)}`);
        }
        return Blosc.fromConfig({ ...config, shuffle });
      },
    };
  });
}
