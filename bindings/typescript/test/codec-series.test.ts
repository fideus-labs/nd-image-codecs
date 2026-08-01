/**
 * Shared fixture-matrix test for the pure-TypeScript `codecSeries` builder.
 *
 * Runs every case in `fixtures/codec-series/matrix.json` — the cross-language
 * fixture matrix shared with the Rust and Python builders — and asserts the
 * produced pipeline JSON matches the committed expected output (or that the
 * case throws when marked `error`).
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { codecSeries, type CodecSeriesOptions, type Family } from "../src/index.ts";

interface MatrixCase {
  name: string;
  axes: string[];
  chunk_shape: number[];
  dtype: string;
  family: Family;
  options?: Record<string, unknown>;
  expected?: unknown[];
  error?: boolean;
}

const matrixPath = fileURLToPath(
  new URL("../../../fixtures/codec-series/matrix.json", import.meta.url),
);
const cases: MatrixCase[] = JSON.parse(readFileSync(matrixPath, "utf8")).cases;

/** Map the fixture's snake_case option names onto {@link CodecSeriesOptions}. */
function toOptions(raw: Record<string, unknown> = {}): CodecSeriesOptions {
  const opts: CodecSeriesOptions = {};
  if (raw.decorrelate !== undefined) opts.decorrelate = raw.decorrelate as number[];
  if (raw.add_decorrelate !== undefined) opts.addDecorrelate = raw.add_decorrelate as number[];
  if (raw.remove_decorrelate !== undefined) opts.removeDecorrelate = raw.remove_decorrelate as number[];
  if (raw.lift !== undefined) opts.lift = raw.lift as CodecSeriesOptions["lift"];
  if (raw.xy_levels !== undefined) opts.xyLevels = raw.xy_levels as number;
  if (raw.reversible !== undefined) opts.reversible = raw.reversible as boolean;
  if (raw.delta_backend !== undefined) opts.deltaBackend = raw.delta_backend as "zstd" | "lz4";
  if (raw.zfp_rate !== undefined) opts.zfpRate = raw.zfp_rate as number;
  return opts;
}

describe("codecSeries fixture matrix", () => {
  it("is not truncated", () => {
    expect(cases.length).toBeGreaterThan(100);
  });

  for (const c of cases) {
    it(c.name, () => {
      const build = () => codecSeries(c.axes, c.chunk_shape, c.dtype, c.family, toOptions(c.options));
      if (c.error) {
        expect(build).toThrow();
        return;
      }
      expect(build()).toEqual(c.expected);
    });
  }
});
