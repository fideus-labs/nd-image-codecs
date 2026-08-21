---
type: report
title: Algebraic Float in the 9/7 DWT
short_title: Algebraic Float in the 9/7 DWT
description: What happened when the irreversible CDF 9/7 lifting kernel was converted to Rust 1.98 algebraic float operations, measured rather than assumed — and why the conversion was reverted.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - dwt
  - performance
  - conformance
related:
  - '[[Float-Drift-Inventory]]'
  - '[[Rust-198-Adoption-Notes]]'
---

# Algebraic Float in the 9/7 DWT

Phase 03 of the [Rust 1.98 adoption](./adoption-notes.md) (`[[Rust-198-Adoption-Notes]]`)
converted the irreversible CDF 9/7 lifting kernel in
[`crates/ndic-htj2k/src/dwt/mod.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-htj2k/src/dwt/mod.rs)
to `f32::algebraic_add` / `f32::algebraic_mul`, measured the result, and **reverted it**.

The outcome is negative, so the useful content of this page is the evidence — the phase's
own decision rule says a converted site with no measurable improvement and no
vectorization change is a net loss, and this records what was measured before applying
it. The prediction that framed the phase came from the
[Float Drift Inventory](./float-drift-inventory.md) (`[[Float-Drift-Inventory]]`), which
had already established that the 9/7 path has no callers; what it could not predict is
that the conversion is inert on the target this workspace actually builds for.

## Outcome in one table

| | |
| --- | --- |
| Sites converted | 2 (both `lift_97` multiply-accumulate lines) |
| Sites kept | **0** |
| Sites reverted | **2** — no vectorization change, no measurable speedup |
| Sites examined with nothing to convert | 4 (`ana_97_1d`, `syn_97_1d`, `forward_97`, `inverse_97`) |
| Golden vectors moved | 0 |
| Tolerance assertions changed | 0 |
| Net code change | Documentation only, plus one new benchmark lane |

## Every site, and what happened to it

| Site | Expression | Action | Why |
| --- | --- | --- | --- |
| `lift_97`, high-from-low branch | `high[i] += coeff * (l + r)` | Converted, then **reverted** | Identical instructions on the shipped target; no vectorization under any target; no measurable speedup |
| `lift_97`, low-from-high branch | `low[i] += coeff * (l + r)` | Converted, then **reverted** | Same |
| `ana_97_1d` | — | **Nothing to convert** | Pure de-interleave (`low[i] = x[2*i]`) plus four `lift_97` calls. No arithmetic of its own |
| `syn_97_1d` | `-ALPHA`, `-BETA`, `-GAMMA`, `-DELTA` | **Nothing to convert** | Compile-time constant sign flips, exact under IEEE. `algebraic_*` has nothing to license |
| `forward_97` / `inverse_97` | — | **Nothing to convert** | Gather/scatter only; all arithmetic is `usize` index math |

The last row is worth stating plainly because the phase plan assumed otherwise: there is
**no per-coefficient scaling step in this 9/7 implementation**. The `K` normalization
factor is deliberately not applied in the transform — like `OpenJPH`, it is absorbed into
the per-subband quantization step, as the module header has always said. The textbook 9/7
has a fifth scaling stage; this one does not.

Sites ruled out before any code was touched — `nd_lift`'s kernels and the reversible 5/3
path, both integer-only — are recorded in
[Sites ruled out](./adoption-notes.md#sites-ruled-out).

## Why it was reverted

### It never vectorized

This is the clause that decided it. Instruction census of the `lift_97` body:

| Build | Strict `+` / `*` | Algebraic |
| --- | --- | --- |
| default `x86-64` — **what this workspace builds** | 4 × `addss`, 2 × `mulss` | 4 × `addss`, 2 × `mulss` — *identical* |
| `-C target-cpu=native` | 4 × `vaddss`, 2 × `vmulss` | 2 × `vfmadd213ss`, 2 × `vaddss` |

Every instruction is **scalar** (`ss`). There is not one packed (`ps`) float operation in
either form in either build. Confirmed on the shipped artifact and not only on a test
harness: the entire `ndic_htj2k` release assembly contains `16 addss + 8 mulss` and no
other float arithmetic — four inlined copies of `lift_97` — and both compiled `ndic-bench`
binaries contain **zero `vfmadd` instructions**.

Two independent reasons stack up here:

1. **The loops are elementwise, not reductions.** Each iteration reads two neighbours and
   writes one output; there is no accumulator carried across iterations. Reassociation —
   the transformation `algebraic_add` is famous for, and the one the
   [capability probe](./capability-probe.md#algebraic-reassociation-is-an-optimizer-decision)
   demonstrated turning 62 into 0 — has nothing to reassociate. The only thing the
   permission enables is contracting `x + coeff * y` into an FMA.
2. **The build has no FMA.** `.cargo/config.toml` sets no `target-cpu` for native
   targets, so the default `x86_64-unknown-linux-gnu` baseline applies and
   `cfg!(target_feature = "fma")` is `false`. With no FMA instruction available, the
   contraction cannot happen either.

What actually blocks vectorization is neither of those: it is the symmetric-extension
index clamps, `low[(i + 1).min(nl - 1)]` and `high[i.saturating_sub(1).min(nh - 1)]`,
which prevent LLVM proving the loads are stride-1. **No amount of arithmetic permission
addresses that.** Peeling the first and last iterations so the interior is
unconditionally contiguous is the change that would vectorize this loop; it changes the
operation sequence, so it was out of scope for a phase defined as operators-only.

### It was not faster

Interleaved A/B of two `ndic-bench` binaries differing only in those two lines, alternated
to cancel thermal drift, on `transform/dwt97_fwd_2048` (2048×2048, 5 levels):

| Round | Strict | Algebraic | Delta |
| --- | --- | --- | --- |
| 1 | 70.88 ms | 71.43 ms | +0.8 % |
| 2 | 68.95 ms | 69.36 ms | +0.6 % |
| 3 | 69.38 ms | 70.06 ms | +1.0 % |
| 4 | 72.10 ms | 69.38 ms | −3.8 % |

No consistent sign, and every delta sits inside the harness's own spread — a single run
reports min/max of 67.91 / 79.48 ms, about ±8 %. A second experiment agrees: a standalone
harness holding both kernels in one process (best-of-5 medians of 9) measured
−0.52 % / +0.07 % / +0.32 % on the default target and +0.11 % / +0.44 % / −3.23 % under
`target-cpu=native`, where the one apparently-large figure failed to reproduce (−0.38 %,
+0.46 %, +0.06 % on repeat).

**Even where the FMA contraction does fire and removes two of six float operations, it
buys no time.** The loop is bound by the index clamps and bounds checks, not by float
throughput.

:::{warning} An artifact worth recording
An early *uncontrolled* pair of runs suggested the algebraic form was 13 % **slower**
(81.67 ms vs 68.86 ms). It was not. The first run executed four workloads across two
configs and the second one workload across one config, so the two saw different thermal
and cache states. Only the interleaved A/B above is evidence. A benchmark harness with an
±8 % spread cannot resolve a sub-1 % effect in either direction, which is the more general
lesson.
:::

## The deviation, and the tolerance it sits inside

Green tests say nothing about how far values moved, so the deviation was measured directly
rather than inferred from a passing assertion.

**On the target this workspace builds, the two forms are bit-identical.** Over four planes
— the 40×24 unit-test fixture, 1024×1024 pseudo-images at 12-bit and 16-bit amplitude at
5 levels, and a 4096×64 wide plane — the comparison is `differing = 0 / 1048576`,
`max_ulps = 0`, max absolute error `0`, max relative error `0`. There is no location in
the plane to report, because no sample moved.

**Under `-C target-cpu=native` the values do move**, by a consistent fraction of the peak
coefficient magnitude:

| Plane | Max absolute deviation | Peak coefficient | Relative |
| --- | --- | --- | --- |
| 1024×1024, 16-bit amplitude, L5 | 1.504e-1 at (x=55, y=4) | 272 370 | **5.522e-7** |
| 1024×1024, 12-bit amplitude, L5 | 9.399e-3 at (x=55, y=4) | 17 023 | **5.522e-7** |
| 4096×64, 16-bit, L5 | 1.094e-1 at (x=39, y=0) | 239 558 | 4.566e-7 |
| 40×24 unit-test fixture, L3 | 1.030e-4 at (x=5, y=0) | 313.3 | 3.287e-7 |

That is ≈2^−21 of full scale: four to five `f32` ulps accumulated over 5 levels × 4 lifting
steps. Reported relative-error figures of 1.25–1.98 in the raw capture are **not**
meaningful error — they occur at coefficients within a few ulps of zero, where the two
results straddle a cancellation. `max_abs / peak` is the metric that carries information.

### Against the quantization step

The phase's drift rule directs a comparison against
`crates/ndic-codestream/src/quant.rs::irrev_delta`, which returns
`Δ = gain · (1 + μ/2^11) / 2^ε` (T.800 eq. E-3, band gain folded into the `[1, 2, 2, 4]`
multiplier):

| ε | Δ (μ=0, finest) | 5.5e-7 deviation as a fraction of Δ |
| --- | --- | --- |
| 12 | 2.441e-4 | 0.23 % |
| 16 | 1.526e-5 | 3.6 % |
| 18 | 3.815e-6 | 14.4 % |
| 20 | 9.537e-7 | 57.7 % |
| 21 | 4.768e-7 | **115 %** |

The deviation reaches one full quantization step only at **ε ≈ 20.8** — the quantizer
would have to retain about 21 bits of magnitude below the peak coefficient before a
reassociation could move a single quantized index. The 9/7 path exists in order to
quantize *more* coarsely than the lossless 5/3 path, so nothing that would actually select
it sits near ε 21. **The deviation is inside the format's own tolerance by one to three
orders of magnitude** — and on the shipped build it is exactly zero, so this argument
covers hypothetical FMA-enabled builds rather than anything currently produced.

### The drift is an improvement

Worth stating because the decision rule is written to catch degradation: where the values
move, they move *closer* to the true result. Round-trip error against the original input,
strict → algebraic:

| Plane | Strict | Algebraic |
| --- | --- | --- |
| 40×24 fixture | 1.068e-4 | **8.392e-5** |
| 1024×1024, 12-bit | 5.127e-3 | **4.395e-3** |
| 1024×1024, 16-bit | 8.203e-2 | **7.031e-2** |
| 4096×64 wide | 7.227e-2 | **6.055e-2** |

An FMA rounds once where the strict form rounds twice. The revert is therefore *not* a
correctness decision — it is purely a "this buys nothing" decision.

## Conformance results

Every suite passed, with test counts and skip lists matching the Phase 02 capture in the
playbook's `Working/baseline-1.98-pre/` exactly — so none silently degraded into an empty
run, which is the trap the inventory warns about.

| Suite | Result |
| --- | --- |
| `ndic-htj2k --release` (lib) | 18 passed, 0 failed |
| `block_roundtrip` | 3 passed, 0 failed |
| `openjph_differential` | 1 passed — *verified 2000 OpenJPH differential vectors* |
| `openjph_interop` | 2 passed — bit-exact both directions |
| `corpus_conformance` | 1 passed — 7 files bit-exact, 3 skipped (2× YUV 4:2:0, 1× multi-tile) |

**No golden vector was regenerated and no tolerance assertion was changed.** None needed
to be: `dwt97_roundtrips_within_tolerance` asserts `< 1e-2` against a worst observed error
of `1.068e-4`, roughly 94× of headroom. That test would not have caught this change at any
plausible magnitude — more evidence for the inventory's ruling that green is not evidence.

## What was kept: a real 9/7 benchmark

One change from this phase survives, and it is not a float change.

`transform/dwt97_fwd_2048` is now registered in
[`bench/rs/ndic-bench-cli/src/workloads/htj2k.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/rs/ndic-bench-cli/src/workloads/htj2k.rs).
Before it, **no registered workload reached the 9/7 kernel at all** — `plane_encode_1024`
and `plane_decode_1024` return an empty `BenchOutput` under `config.irreversible` because
9/7 encode is not wired up, and the only other `transform` entry is the integer 5/3 lane.
The `simd-97-ht` config was therefore reporting a 5/3 measurement under a 9/7 label. The
inventory called this out and named adding a workload as the only honest way to attribute
a number to this code; `AGENTS.md` requires the benchmark in the same change regardless.

It gives the kernel its first real figure:

| Lane | 2048×2048, 5 levels |
| --- | --- |
| `transform/dwt97_fwd_2048` (scalar float 9/7) | **~70 ms** |
| `transform/dwt53_fwd_2048`, scalar lane | 73.98 ms |
| `transform/dwt53_fwd_2048`, SIMD lane | **8.70 ms** |

The 8× gap between the scalar and SIMD 5/3 lanes is the size of the prize a *real*
vectorization of the 9/7 kernel would be chasing — and the reason the loop-peeling note
above is worth acting on in a later phase. The new lane carries no `bytes_in`/`bytes_out`,
so it cannot affect the ratio gate; the 29 ratio-carrying pairs Phase 02 pinned are
untouched by this phase.

## What a later phase should take from this

1. **`algebraic_*` is not a vectorization tool for elementwise loops.** It licenses
   reassociation and contraction. A loop with no carried reduction has nothing to
   reassociate, and contraction needs FMA in the target feature set. Phase 04 should check
   the loop *shape* first: only carried reductions are candidates, which the inventory's
   "loop shape" column already classifies.
2. **Check `target-cpu` before predicting any float codegen win.** This workspace builds
   for baseline `x86-64`. Every FMA-dependent argument is void unless that changes, which
   is a separate decision with its own portability cost.
3. **The 9/7 kernel's real bottleneck is the boundary clamps.** Peeling the first and last
   iterations so the interior is unconditionally stride-1 is the change worth making, and
   it is an operation-sequence change, not an operator change.
4. **A ±8 % harness cannot adjudicate a 1 % change.** Interleave the A/B in one process,
   or compare instruction counts instead of wall-clock.
