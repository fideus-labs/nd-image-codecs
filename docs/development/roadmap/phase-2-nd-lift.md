---
title: Phase 2 — The nd_lift Cross-Axis Transform
short_title: Phase 2 — nd_lift
description: Phase 2 implements nd_lift, the explicit array-to-array codec that captures z/t/c correlation without JPEG 2000 Part 2, validated behind a stock Blosc-Zstd backend.
---

# Phase 2 — The `nd_lift` Cross-Axis Transform

**Depends on:** Phase 1 · **Gates:** Phase 4 · **Architecture:** [nd_lift Transform](../../architecture/nd-transform.md)

Phase 2 implements `nd_lift` (`ndic-lift`), the explicit array-to-array codec
that captures z/t/c correlation without JPEG 2000 Part 2. It is validated on its
own — behind a stock Blosc-Zstd backend — before the HTJ2K plane codec exists,
so its decorrelation gains are measurable in isolation.

## What to build

1. **Integer lifting kernels** for `LiftKind::{Delta, Haar, Lift53}`: forward
   and inverse 1D transforms with T.800-style integer rounding.
2. **`forward`/`inverse`** over a chunk: apply each `AxisTransform` along its
   `dimension` within bounded `group`s, in listed order (reverse on decode).
3. **Boundary handling**: symmetric extension; correct odd-length and singleton
   axes with no padding written.
4. **Overflow budget**: `i32` planes for ≤32-bit input, `i64` for 64-bit;
   assert per-axis bit growth on encode.
5. **zarrs registration** (feature-gated): implement `ArrayToArrayCodecTraits` +
   `CodecTraits`; `inventory` plugin submission.
6. **Versioning**: honor `version: "0.1"`; refuse unknown majors.

## Order of work

1. `Delta` (single lifting step) — simplest, establishes the harness.
2. `Haar` (2-tap reversible) — introduces even/odd lanes.
3. `Lift53` (predict+update, multi-level) — the general case.
4. zarrs codec wrapper + registration.
5. Validation series `transpose → nd_lift → bytes → blosc(zstd)` for measurable
   ratio gains vs nd-delta.

## Spec / reference anchors

- Le Gall 5/3 lifting & symmetric extension: ITU-T T.800 Annex F, <https://www.itu.int/rec/T-REC-T.800>
- Reversible integer-to-integer wavelets (Calderbank–Daubechies–Sweldens–Yeo).
- zarrs array-to-array codec guide: <https://book.zarrs.dev/extensions/codec.html>

## Tests & benchmarks

- Round-trip identity for every kind × every integer dtype (proptest).
- Analytic vectors (impulse/ramp/DC) vs closed-form band values.
- Boundary/odd-length/singleton edge cases enumerated.
- Bench lanes: `nd-lift-delta-zstd`, `nd-lift-53-zstd` vs `nd-delta-zstd` (ratio
  on correlated z-stacks).

## Acceptance criteria

- [ ] `delta`/`haar`/`lift53` round-trip exactly for all supported integer dtypes.
- [ ] `nd_lift` registers as a `zarrs` array-to-array codec and composes with
      stock `blosc`.
- [ ] A `transpose → nd_lift → blosc` series beats nd-delta on correlated
      volumetric fixtures in the bench suite.
- [ ] Python/TS `NdLift` config classes serialize configs the Rust codec accepts.
