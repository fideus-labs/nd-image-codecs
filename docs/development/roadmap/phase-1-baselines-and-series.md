---
title: Phase 1 — Baselines & the Codec-Series Builder
short_title: Phase 1 — Baselines & Series
description: 'Phase 1 delivers something useful on day one without implementing any new codec: the nd-delta family, the codec_series builder in all three languages, and a working benchmark harness.'
---

# Phase 1 — Baselines & the Codec-Series Builder

**Depends on:** nothing (first phase) · **Gates:** all later phases · **Architecture:** [](../../architecture/codec-series.md),
[](../../architecture/zarr-codec.md)

Phase 1 delivers something useful on day one **without implementing any new
codec**: the **nd-delta** family (built entirely from existing Zarr codecs), the
`codec_series` builder in all three languages, and a working benchmark harness.

## What to build

1. **The `codec_series` builder** (`ndic-zarr` `series` module) — *done in the
   scaffold*: axis parsing, transpose-order selection, decorrelation-axis
   defaults + overrides, and the three family tails, returning Zarr v3 codec
   JSON. Unit-tested.
2. **Pure-Python mirror** (`nd_image_codecs.codec_series`) — *done*: identical
   behavior, no native module required.
3. **Pure-TypeScript mirror** (`codecSeries`) — *done*: identical behavior.
4. **Cross-language equality test** — a shared fixture matrix (axis layouts ×
   chunk shapes × dtypes × families) run through all three implementations,
   asserting byte-identical JSON. Wire this into CI (`scripts/`).
5. **nd-delta end-to-end** — the builder already emits
   `transpose → numcodecs.delta → bytes → blosc(bitshuffle, zstd/lz4)`. Validate
   that a `zarr-python` array using this pipeline round-trips real data, since
   every codec in it already exists.
6. **Benchmark harness** (`bench/`, `ndic-bench-*`) — record ratio and
   throughput for nd-delta vs plain blosc-zstd on OME-Zarr fixtures; commit
   baselines.

## Order of work

1. Land the builder + mirrors + equality test (foundation for everything).
2. Add the nd-delta round-trip test against `zarr-python`.
3. Stand up the benchmark harness and commit the first baselines.

## Spec / reference anchors

- Zarr v3 codec pipeline & `transpose`: <https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html>
- `numcodecs` delta + blosc + bitshuffle: <https://numcodecs.readthedocs.io>
- Delta+shuffle+Zstd as the OME-Zarr microscopy workhorse: <https://pmc.ncbi.nlm.nih.gov/articles/PMC9900847/>

## Tests & benchmarks

- Rust `series::tests` (grouping, overrides, ZFP dim cap) — green.
- Cross-language JSON equality across the fixture matrix.
- nd-delta round-trip on `zarr-python` fixtures.
- Bench lanes: `nd-delta-zstd`, `nd-delta-lz4` vs `blosc-zstd` baseline.

## Acceptance criteria

- [x] `codec_series` produces identical JSON in Rust, Python, and TypeScript for
      the whole fixture matrix, enforced in CI
      (`fixtures/codec-series/matrix.json` + `scripts/ci/check-series-equality.py`,
      the `series-equality` job, and per-language matrix tests).
- [x] An nd-delta pipeline authored by the builder round-trips real OME-Zarr data
      via `zarr-python`
      (`bindings/python/nd-image-codecs/tests/test_nd_delta_roundtrip.py`).
- [x] `ndic series` CLI emits valid pipelines for all three families
      (full option set: decorrelation overrides, lift kind, xy levels, lossy,
      delta backend, ZFP rate; unit-tested).
- [x] Benchmark harness records committed baselines for the nd-delta lanes
      (`bench/py/run_nd_delta.py` → `bench/baselines/main/`, gated by
      `ndic-bench compare` in `bench-pr-gate.yml`).
