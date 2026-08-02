/**
 * `NdLift` config-class tests: every `nd_lift` configuration the
 * cross-language `codecSeries` builder emits (the committed fixture matrix)
 * must construct and re-serialize byte-identically — the same configs the
 * Rust codec accepts — and the version gate must refuse unknown versions.
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { NdLift, type NdLiftConfig, type NdLiftTransform } from "../src/index.ts";

const matrixPath = fileURLToPath(
  new URL("../../../fixtures/codec-series/matrix.json", import.meta.url),
);
const cases: { name: string; expected?: { name: string; configuration?: unknown }[] }[] =
  JSON.parse(readFileSync(matrixPath, "utf8")).cases;

const liftConfigs = cases.flatMap(
  (c) => (c.expected ?? []).filter((codec) => codec.name === "nd_lift") as NdLiftConfig[],
);

describe("NdLift config class", () => {
  it("the fixture matrix exercises nd_lift configs", () => {
    expect(liftConfigs.length).toBeGreaterThan(0);
  });

  it("accepts and re-serializes every builder-emitted configuration", () => {
    for (const config of liftConfigs) {
      const codec = NdLift.fromConfig(config);
      expect(codec.toDict()).toEqual(config);
    }
  });

  it("refuses unknown configuration versions", () => {
    for (const version of ["0.2", "1.0", "nonsense"]) {
      expect(
        () => new NdLift({ name: "nd_lift", configuration: { version, transforms: [] } }),
      ).toThrow(/not supported/);
    }
  });

  it("refuses lifting kinds without levels", () => {
    const transforms: NdLiftTransform[] = [
      { axis: "z", dimension: 0, kind: "lift53", levels: 0, group: 0 },
    ];
    expect(() => new NdLift({ name: "nd_lift", configuration: { transforms } })).toThrow(
      /levels >= 1/,
    );
  });
});
