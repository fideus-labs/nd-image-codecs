---
type: analysis
title: Float Drift Inventory
short_title: Float Drift Inventory
description: Which exactness tests can actually observe a float reassociation change, and every float arithmetic site in the workspace classified as a candidate for the Rust 1.98 algebraic operations.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - float
  - conformance
---

# Float Drift Inventory

This page answers two questions that Phases 03–04 of the [Rust 1.98 adoption](./adoption-notes.md)
must not guess at:

1. **If a golden value moves, which test will tell me?** Section
   [Exactness tests](#exactness-tests) classifies every exactness-sensitive suite as
   *drift-sensitive* or *structurally immune*, by reading what the code under it
   actually computes.
2. **Where is there float arithmetic worth converting at all?** Section
   [Float arithmetic sites](#float-arithmetic-sites) inventories every `f32`/`f64`
   arithmetic site in `crates/`, `bench/rs/`, and `bindings/`, with its loop shape and
   whether its result feeds a bit-exact assertion.

The reason this exists before any code changes: `algebraic_add` and friends license the
optimizer to reassociate, and the [capability probe](./capability-probe.md#algebraic-reassociation-is-an-optimizer-decision)
already demonstrated a reassociated sum landing on `0` where the strict sum lands on
`62`. An assertion failure in Phase 03 must be classifiable on sight as *expected drift*
or *real bug*, and — more dangerous — a **silently passing** test must not be mistaken
for evidence that nothing moved.

Everything below was read on `cc0cd12` against `rustc 1.98.0`. Measurements that back it
are in the phase's `Working/baseline-1.98-pre/` capture.

## Headline finding

**There is no reachable float arithmetic in any codec path in this workspace.**

Every byte any codec emits today is produced by integer arithmetic, or by the external
`zfp-rs` crate. The two float paths that exist in first-party code — the irreversible
9/7 DWT and the irreversible quantizer step size — are **not reachable from any encoder
or decoder entry point**; the encoder refuses `WaveletKind::Irreversible97` outright. The
one float loop that *is* reachable, `numcodecs.delta` over `float32`/`float64`, is
exactness-critical in a way that forbids reassociation entirely.

The practical consequence, stated plainly so Phase 03 does not discover it the hard way:
**converting the 9/7 DWT to `algebraic_*` cannot regress a golden vector, and equally
cannot show up in any benchmark, because nothing calls it.** See
[The 9/7 path is not wired up](#the-97-path-is-not-wired-up).

(exactness-tests)=
## Exactness tests

| Suite | Float in the code under test? | Can it observe reassociation? | Assertion kind |
| --- | --- | --- | --- |
| `crates/ndic-lift/tests/vectors.rs` | **No** — zero `f32`/`f64` tokens in `ndic-lift/src/` | **No — structurally immune** | Bit-exact vs `fixtures/nd-lift/vectors.json` |
| `crates/ndic-zfp/tests/checksums.rs` | **No** — `f64` appears only as `rate`/`tolerance` config | **No — structurally immune** | Bit-exact vs `fixtures/zfp/checksums.json` |
| `crates/ndic-htj2k/tests/openjph_differential.rs` | No — block coder is integer | **No** | Bit-exact vs 2000 OpenJPH oracle vectors |
| `crates/ndic-codestream/tests/openjph_interop.rs` | No — reversible 5/3 only | **No** | Bit-exact both directions vs `ojph_compress`/`ojph_expand` |
| `crates/ndic-codestream/tests/corpus_conformance.rs` | No — reversible 5/3 only | **No** | Bit-exact vs corpus references |
| `crates/ndic-zarr/tests/delta_zarrs.rs` | **Yes** — `delta_float!` over `float32` | **Yes, but it will not notice** — see below | Exact `assert_eq!` on `Vec<f32>` |
| `crates/ndic-zarr/tests/lift_zarrs.rs` | No — integer dtypes only | No | Exact `assert_eq!` |
| `crates/ndic-zarr/tests/htj2k_zarrs.rs` | No — reversible 5/3 only | No | Exact `assert_eq!` |
| `crates/ndic-zarr/tests/series_matrix.rs` | No — pure JSON metadata | No | Exact `assert_eq!` on pipeline JSON |
| `crates/ndic-zarr/tests/zfp_zarrs.rs` | Only through `zfp-rs` | No | 4 exact `assert_eq!` + **1 tolerance** (`worst < 0.5`) |
| `crates/ndic-htj2k/src/dwt/mod.rs` unit tests | **Yes** — the only 9/7 caller | **Yes** | **Tolerance** `(want − got).abs() < 1e-2` |

### `ndic-lift` — structurally immune

`crates/ndic-lift/src/` contains **zero** `f32` or `f64` tokens. `kernel.rs` operates
entirely on the `PlaneSample` trait, which `sample.rs` **seals** over exactly `i32` and
`i64`:

```rust
mod sealed {
    pub trait Sealed {}
    impl Sealed for i32 {}
    impl Sealed for i64 {}
}
```

The sealing is not incidental — the doc comment states it carries the invariant that the
forward budget check makes plain arithmetic exact, so an outside implementation for a
narrower type would silently break it. The lifting kernels use `+`, `-`, and arithmetic
`>>` on integers throughout, with `wrapping_add`/`wrapping_sub` on the inverse side. No
`algebraic_*` operation applies to any of it.

The only mention of floats in the crate is a `lib.rs` doc line describing 9/7-style float
lifting as "the lossy extension, **not yet implemented**".

**Verdict:** `vectors.rs` cannot move under any float change. It is a useful canary for
integer regressions and nothing else.

### `ndic-zfp` — the coded stream is entirely external

Every bit of the ZFP stream comes from the `zfp-rs` crate. `crates/ndic-zfp/src/` is
~1600 lines of shape validation, header parsing, brick addressing, and dtype dispatch,
and it does **no float arithmetic on sample data at all**:

- `f32`/`f64` appear as *sample types*, handed to `zfp_rs::ZfpField` and never operated
  on locally (`chunk.rs:376-377`, `chunk.rs:415-416`).
- `f64` appears as the `rate` / `tolerance` *configuration* scalar (`lib.rs:79`,
  `lib.rs:81`, `chunk.rs:106`, `chunk.rs:112`), validated with `is_finite` and
  comparisons and then passed into `ZfpConfig` unmodified.
- `fixed_rate_stream_len` (`lib.rs:380`) and `BrickIndex` do their size math entirely in
  `u64`/`usize` — `div_ceil`, `checked_mul`, `next_multiple_of`. No float appears in the
  byte-offset computation, which is why the brick index is exact.

`zfp-rs` is an ordinary registry dependency. Nothing this workspace does to its own code
can change what `zfp-rs` compiles to, and changing `zfp-rs` is out of scope.

**Verdict:** `checksums.rs` is structurally immune. Both of its tests
(`checksum_matrix_is_reproduced_bit_exactly`, `chunk_fixture_is_byte_stable`) pin bytes
produced entirely outside first-party control.

### The HTJ2K and codestream conformance suites — reversible 5/3 only

All three shell-out / corpus suites exercise the **reversible 5/3 path exclusively**:

- `openjph_differential.rs` drives the **HT block coder**, which is integer throughout
  (`crates/ndic-htj2k/src/block/`); the whole crate's only float file is `dwt/mod.rs`.
  It verified 2000 oracle vectors bit-exactly in the baseline capture.
- `openjph_interop.rs` round-trips through `ojph_compress` / `ojph_expand` and asserts
  bit-exactness in both directions. It cannot reach 9/7 because the writer refuses it —
  `crates/ndic-codestream/src/writer.rs:77` returns
  `Error::Unsupported { "writer is lossless (5/3) only" }` for any
  `WaveletKind` other than `Reversible53`.
- `corpus_conformance.rs` is 5/3 **by construction**: it filters the corpus to
  `simple_dec_rev53_*` files and reports everything else as skipped. In the baseline it
  decoded 7 files bit-exactly and skipped 3 (2× YUV 4:2:0 subsampling, 1× multi-tile).
  The reader also gates on `self.cod.wavelet != 1` (`reader.rs:326`), i.e. it only
  accepts the reversible transform.

**Tolerance: not applicable.** None of these three suites has a tolerance — every
assertion is bit-exact. That is not a weakness for this inventory's purposes; it means
that *if* they ever became reachable from a float path, they would fail loudly rather
than drift silently.

**Verdict:** all three are structurally immune today, and are the right regression
canaries precisely because they are.

### `ndic-zarr` — one real float loop, and a test that will not catch it

Nine of the ten `ndic-zarr` test assertions are exact `assert_eq!`. The single
tolerance-based comparison in the crate is `zfp_zarrs.rs:131-149`
(`fixed_rate_bounds_the_error_on_smooth_data`), which asserts
`worst < 0.5` over `(a − b).abs()` at 16 bits/value — and that error is produced inside
`zfp-rs`, so it is immune anyway.

The one that matters is `delta_zarrs.rs`. `roundtrip_dtype::<f32>("float32", |i| i as f32)`
drives the `delta_float!` codec and asserts
`assert_eq!(back, data, "float32 must round-trip exactly")` — an exact float comparison.

**But it would not catch a reassociation regression**, and this is the trap in the whole
inventory. The fixture is `|i| i as f32` for `i` in `0..512`: every value is a small
exact integer, every partial sum is a small exact integer, and every one of them is
exactly representable in `f32`. Under those inputs `delta` then `cumsum` is exact
*regardless* of summation order, so a reassociated `cumsum` still round-trips and the
test still passes — while the codec's byte-level agreement with `numcodecs.delta` on real
data has silently broken. Green here would mean nothing.

**Verdict:** drift-*capable* but not drift-*sensitive*. Do not treat a green
`delta_zarrs` as evidence. See the "must not change" ruling in the site table below.

(the-97-path-is-not-wired-up)=
### The 9/7 path is not wired up

This is the finding Phase 03 most needs, so it is spelled out rather than implied.
`forward_97` and `inverse_97` are `pub` in `ndic_htj2k::dwt`, but a workspace-wide search
for callers finds **exactly one**: the crate's own unit test at `dwt/mod.rs:482-483`.
Concretely:

| Claim | Evidence |
| --- | --- |
| No encoder can select 9/7 | `crates/ndic-codestream/src/writer.rs:77` rejects any `WaveletKind != Reversible53` with `Error::Unsupported` |
| No decoder can select 9/7 | `crates/ndic-codestream/src/reader.rs:326` bails unless `cod.wavelet == 1` (reversible) |
| The enum variant is inert | `WaveletKind::Irreversible97` (`crates/ndic-core/src/params.rs:30`) is **never matched anywhere** in the workspace — it is declared and never read |
| `irrev_delta` is dead | `crates/ndic-codestream/src/quant.rs:209` has **zero** callers in `crates/`, `bench/`, or `bindings/` |
| No benchmark reaches it | `bench/rs/ndic-bench-cli/src/workloads/htj2k.rs:58,77` return `BenchOutput::default()` when `config.irreversible`, with the comment "9/7 encode is not implemented yet" |
| The `simd-97-ht` config is misleading | It runs only `transform/dwt53_fwd_2048`, and that workload calls `dwt::simd::forward_53` — the **integer 5/3** transform. The 9/7 label describes an intent, not what is measured. Confirmed in the baseline: `simd-97-ht` has exactly one record. |
| Its only coverage is a tolerance test | `dwt97_roundtrips_within_tolerance` asserts `(want − got).abs() < 1e-2` on a 40×24 plane at 3 levels |

So for Phase 03, both halves of the usual argument collapse:

- **Risk side:** converting `lift_97` to `algebraic_*` cannot move any golden vector,
  because no golden vector is downstream of it. The only assertion it can affect has a
  `1e-2` tolerance against values of magnitude ~128 — roughly 1e-4 relative, which
  four `f32` lifting steps' worth of reassociation will not come close to exceeding.
- **Reward side:** it also cannot show a benchmark win, because no registered workload
  calls it. A Phase 03 that reports a speedup has measured something else.

If Phase 03 wants a real number, it must first add a workload that calls `forward_97`
directly — that is a *new benchmark*, and per `AGENTS.md` it belongs in the same change.
Adding one is legitimate; reporting `transform/dwt53_fwd_2048` under the `simd-97-ht`
label as a 9/7 result is not.

(float-arithmetic-sites)=
## Float arithmetic sites

Every `f32`/`f64` arithmetic site in `crates/`, `bench/rs/`, and `bindings/`. "Loop shape"
is what decides whether reassociation can help at all: an **elementwise** loop has no
cross-iteration float dependency and mostly vectorizes already; a **carried reduction**
is where `algebraic_add` actually buys something — and is also exactly where the result
changes.

Classification: **hot** = worth changing, **cold** = config/report/format, not worth it,
**exactness-critical** = a conformance re-check is mandatory (or the change is forbidden).

| File | Function | Line | Expression | Loop shape | Feeds a bit-exact assertion? | Class |
| --- | --- | --- | --- | --- | --- | --- |
| `crates/ndic-zarr/src/delta_codec.rs` | `delta_float!` cumsum | 163 | `acc += v` | **Carried reduction** over the whole chunk | Yes — `delta_zarrs.rs` `assert_eq!`, and `numcodecs.delta` byte parity | **exactness-critical — DO NOT CHANGE** |
| `crates/ndic-zarr/src/delta_codec.rs` | `delta_float!` diff | 153 | `v - prev` | Sequential, one-step carried | Yes — same | **exactness-critical — DO NOT CHANGE** |
| `crates/ndic-htj2k/src/dwt/mod.rs` | `lift_97` | 118 | `high[i] += coeff * (l + r)` | Elementwise over `nh`, no reduction | No — unreachable | hot *shape*, **unreachable** |
| `crates/ndic-htj2k/src/dwt/mod.rs` | `lift_97` | 125 | `low[i] += coeff * (l + r)` | Elementwise over `nl`, no reduction | No — unreachable | hot *shape*, **unreachable** |
| `crates/ndic-htj2k/src/dwt/mod.rs` | `syn_97_1d` | 315–318 | `-ALPHA`, `-BETA`, `-GAMMA`, `-DELTA` | Const negation, 4× per call | No | cold (exact under IEEE; sign flip is lossless) |
| `crates/ndic-codestream/src/quant.rs` | `irrev_delta` | 219–222 | `f32::from(...) * gains[..] / f32::from(1u16 << 11)`, then `mantissa / f32::from_bits(...)` | **Scalar**, once per `(res, band)` | No — zero callers | cold/config, **dead** |
| `bench/rs/ndic-bench-core/src/lib.rs` | `sigma` | 283 | `.sum::<f64>() / n` | **Carried reduction**, n ≈ 10–20 | It *is* the gate's σ envelope | **exactness-critical (gate) — DO NOT CHANGE** |
| `bench/rs/ndic-bench-core/src/lib.rs` | `sigma` | 286–288 | `(s as f64 - mean).powi(2)` summed | **Carried reduction**, n ≈ 10–20 | Same | **exactness-critical (gate) — DO NOT CHANGE** |
| `bench/rs/ndic-bench-core/src/lib.rs` | `sigma` | 289 | `var.sqrt()` | Scalar | Same | cold |
| `bench/rs/ndic-bench-core/src/lib.rs` | `BenchRecord::ratio` | 147 | `o as f64 / i as f64` | Scalar, once per record | It *is* the ratio gate's input | **exactness-critical (gate) — DO NOT CHANGE** |
| `bench/rs/ndic-bench-core/src/lib.rs` | `diff` comparer | 330, 334–335 | `(cur − base) / base`, `base * (1.0 + 0.10)` | Scalar per record pair | Same | cold, gate |
| `bench/rs/ndic-bench-cli/src/workloads/zfp.rs` | `correlated_volume_f32_bytes` | 75 | `(f + 500 + n) as f32 / 3.0` | Elementwise, 131 072 iterations, **outside the timed region** | Indirectly — it determines `bytes_in`/`bytes_out`, hence the ratio gate | cold (fixture), **DO NOT CHANGE** |
| `bench/rs/ndic-bench-cli/src/report.rs` | `fmt_ns`, `fmt_change` | 9–13, 26 | `ns_f / 1e9`, `p * 100.0` | Scalar, display only | No | cold |
| `crates/ndic-cli/src/commands.rs` | ratio print | 183 | `bytes.len() as f64 / in_bytes as f64` | Scalar, display only | No | cold |

### Sites that carry a float type but perform no arithmetic

Listed so a later sweep does not re-investigate them. Each is a `rate` / `tolerance` /
sample-type value that is validated or passed through, never computed on:

| File | Lines | What it is |
| --- | --- | --- |
| `crates/ndic-zfp/src/lib.rs` | 79, 81, 163–172, 403, 525, 595 | `ZfpMode` payloads, `ZfpElement` impls, `rate` parameters — all forwarded to `zfp-rs` |
| `crates/ndic-zfp/src/chunk.rs` | 106, 112, 376–377, 415–416, 432–433, 513, 520 | Config fields, dtype dispatch arms, `bytemuck` casts |
| `crates/ndic-zarr/src/series.rs` | 109 | `zfp_rate: Option<f64>` spec field |
| `crates/ndic-zarr/src/zfp_codec.rs` | 353 | `rate: f64` parameter |
| `crates/ndic-cli/src/main.rs`, `zarr_io.rs` | 93, 113 | `--zfp-rate` CLI/JSON plumbing |
| `bindings/python/nd-image-codecs/src/lib.rs` | 98–99, 127–128, 154–155 | `rate`/`tolerance` PyO3 parameters |
| `crates/ndic-htj2k/src/dwt/simd.rs` | — | **Zero float tokens in 507 lines.** The SIMD lane is the integer 5/3 transform, bit-identical to the scalar oracle by design. Named as a Phase-02 target; it is not one. |

## What Phases 03–04 should conclude

1. **`crates/ndic-htj2k/src/dwt/simd.rs` is not an `algebraic_*` candidate.** It contains
   no floats. Any plan that names it is working from the file's reputation, not its
   contents.
2. **The 9/7 conversion is safe and unmeasurable.** Do it if the goal is to have the
   code ready for when 9/7 is wired up; do not report a benchmark delta for it without
   first adding a workload that calls it.
3. **`delta_float!` is the one reachable float loop, and it is off limits.** Its
   `cumsum` is precisely the shape `algebraic_add` targets, which is what makes it
   dangerous: the win is real, and so is the break. The codec exists to reproduce
   `numcodecs.delta` byte-for-byte so Rust, Python, and TypeScript readers agree; a
   reassociated `cumsum` breaks that on real data while `delta_zarrs.rs` stays green.
4. **Do not touch `ndic-bench-core`.** `sigma`, `ratio`, and the comparer are the
   measuring instrument. Changing the instrument mid-migration invalidates every
   before/after number in the phase.
5. **Green is not evidence.** Two of the five suites named in the phase plan report
   `ok. 0 passed` when run per-crate without `--features serde`
   (`ndic-lift/tests/vectors.rs` and `ndic-zfp/tests/checksums.rs` are both
   `#![cfg(feature = "serde")]`), and `delta_zarrs.rs` passes on exactly-representable
   fixtures regardless of summation order. Re-measure with
   `scripts/rust198-remeasure.sh`, which passes the right features and records the test
   counts.
