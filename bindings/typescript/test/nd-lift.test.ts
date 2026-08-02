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
    expect(
      () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms } }),
    ).toThrow(/levels >= 1/);
  });

  // The three validators (Rust serde + `chunk::validate`, `_lift.validate_config`,
  // and this class) have to agree, or a hand-written configuration accepted
  // here fails only once it reaches a codec that can actually transform.
  it("refuses unknown transform fields", () => {
    const transforms = [
      { axis: "z", dimension: 0, kind: "haar", levels: 1, group: 0, quantize: true },
    ] as unknown as NdLiftTransform[];
    expect(
      () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms } }),
    ).toThrow(/unknown fields \["quantize"\]/);
  });

  it("refuses a negative group", () => {
    const transforms: NdLiftTransform[] = [
      { axis: "z", dimension: 0, kind: "haar", levels: 1, group: -1 },
    ];
    // The message keeps the value inline so the diagnostic names the offender.
    expect(
      () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms } }),
    ).toThrow(/group -1 must be >= 0/);
  });

  // Configurations arrive from JSON, so the static types are a claim about
  // this boundary rather than a guarantee at it.
  it("refuses transforms that are not an array of objects", () => {
    const bad: unknown[] = [{ transforms: {} }, { transforms: "delta" }];
    for (const configuration of bad) {
      expect(
        () =>
          new NdLift({
            name: "nd_lift",
            configuration: { version: "0.1", ...(configuration as object) },
          }),
      ).toThrow(/transforms must be an array/);
    }
    for (const t of [null, "delta", 3, []]) {
      expect(
        () =>
          new NdLift({
            name: "nd_lift",
            configuration: { version: "0.1", transforms: [t] as unknown as NdLiftTransform[] },
          }),
      ).toThrow(/transform must be an object/);
    }
  });

  it("refuses a non-integer group", () => {
    // `("8" ?? 0) < 0` is false, so a string group used to serialize straight
    // through to a codec that types it as `u32`. `null`/`undefined` are not
    // in this list: `?? 0` normalizes both to the default, matching how the
    // adjacent `levels ?? 0` check already treats an absent value.
    for (const group of ["8", 1.5, Number.NaN, -1]) {
      const transforms = [
        { axis: "z", dimension: 0, kind: "haar", levels: 1, group },
      ] as unknown as NdLiftTransform[];
      expect(
        () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms } }),
      ).toThrow(/must be >= 0/);
    }
  });

  it("refuses a non-string version", () => {
    // Rust types `version` as `String`; the JSON number 0.1 is a serde error
    // there, so stringifying it here would accept a refused configuration.
    for (const version of [0.1, 1, ["0", "1"], {}]) {
      const configuration = { version, transforms: [] } as unknown as NdLiftConfig["configuration"];
      expect(() => new NdLift({ name: "nd_lift", configuration })).toThrow(
        /version must be a string/,
      );
    }
  });

  it("refuses a non-string axis", () => {
    // Rust types `axis` as `String`, so a numeric axis is a serde error
    // there. Python's `validate_config` pins the same rule.
    for (const axis of [5, null, 0.5, ["z"], { name: "z" }]) {
      const transforms = [
        { axis, dimension: 0, kind: "haar", levels: 1, group: 0 },
      ] as unknown as NdLiftTransform[];
      expect(
        () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms } }),
      ).toThrow(/axis .* must be a string/);
    }
  });

  it("refuses transforms missing kind or dimension", () => {
    // `dimension` and `kind` have no serde default in AxisTransform; `axis`,
    // `levels`, and `group` do, so they stay optional.
    for (const t of [{ dimension: 0, levels: 1 }, { kind: "haar", levels: 1 }]) {
      expect(
        () =>
          new NdLift({
            name: "nd_lift",
            configuration: { version: "0.1", transforms: [t] as unknown as NdLiftTransform[] },
          }),
      ).toThrow(/missing required field/);
    }
    // The defaulted fields may be omitted outright — no cast needed, because
    // the public type now matches the serde attributes.
    const ok: NdLiftTransform[] = [{ dimension: 0, kind: "haar", levels: 1 }];
    expect(
      () => new NdLift({ name: "nd_lift", configuration: { version: "0.1", transforms: ok } }),
    ).not.toThrow();
    expect(
      () =>
        new NdLift({
          name: "nd_lift",
          configuration: { version: "0.1", transforms: [{ dimension: 0, kind: "delta" }] },
        }),
    ).not.toThrow();
  });

  it("enforces the Rust integer widths on levels and group", () => {
    const build = (over: Record<string, unknown>) =>
      new NdLift({
        name: "nd_lift",
        configuration: {
          version: "0.1",
          transforms: [
            { axis: "z", dimension: 0, kind: "haar", levels: 1, group: 0, ...over },
          ] as unknown as NdLiftTransform[],
        },
      });
    // levels is u8, group is u32.
    expect(() => build({ levels: 256 })).toThrow(/levels 256 must be >= 0 and <= 255 \(u8\)/);
    expect(() => build({ group: 0x1_0000_0000 })).toThrow(/<= 4294967295 \(u32\)/);
    expect(() => build({ levels: "2" })).toThrow(/levels "2" must be >= 0/);
    expect(() => build({ levels: 1.5 })).toThrow(/levels 1.5 must be >= 0/);
    // Boundary values are accepted.
    expect(() => build({ levels: 255, group: 0xffff_ffff })).not.toThrow();
  });

  it("refuses a configuration object without a version", () => {
    const configuration = { transforms: [] } as unknown as NdLiftConfig["configuration"];
    expect(() => new NdLift({ name: "nd_lift", configuration })).toThrow(/explicit "version"/);
    // An absent configuration is the all-defaults case, not a missing version.
    expect(new NdLift({ name: "nd_lift" }).toDict().configuration.version).toBe("0.1");
    // A JSON `"configuration": null` is absent too — it must not throw a bare
    // TypeError on property access.
    const nulled = { name: "nd_lift", configuration: null } as unknown as NdLiftConfig;
    expect(new NdLift(nulled).toDict().configuration.version).toBe("0.1");
  });
});
