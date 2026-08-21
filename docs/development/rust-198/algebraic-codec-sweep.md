---
type: report
title: Algebraic Float Across the SIMD Path, Quantization, and Codec Glue
short_title: Algebraic Codec Sweep
description: The Phase 04 sweep of every remaining float site outside the 9/7 kernel — why the hand-written SIMD module is kept, why zero further sites were converted, and where the ndic-zfp exactness boundary actually sits.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - simd
  - float
  - conformance
related:
  - '[[Float-Drift-Inventory]]'
  - '[[Rust-198-Adoption-Notes]]'
  - '[[Algebraic-Float-in-the-97-DWT]]'
  - '[[Unsafe-Audit]]'
---

# Algebraic Float Across the SIMD Path, Quantization, and Codec Glue

Phase 04 of the [Rust 1.98 adoption](./adoption-notes.md) (`[[Rust-198-Adoption-Notes]]`)
swept every float site outside the 9/7 kernel that
[Phase 03](./algebraic-97-dwt.md) (`[[Algebraic-Float-in-the-97-DWT]]`) had already
settled, and asked one larger question alongside it: is the hand-written `core::arch`
SIMD module in
[`crates/ndic-htj2k/src/dwt/simd.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-htj2k/src/dwt/simd.rs)
— the only `unsafe` anywhere in the workspace — still earning its keep?

**No shipped line of code changed.** That is the finding, not an absence of one: every
candidate was measured, and each measurement came back at zero. The value of this page is
the evidence, so a later phase does not re-open a settled question or, worse, act on the
premise this one started from.

## Outcome in one table

| | |
| --- | --- |
| Sites converted to `algebraic_*` | **0** |
| Sites examined and ruled out | 8 |
| `simd.rs` decision | **Keep** — 4.5× to 11.8× faster than the scalar oracle |
| `unsafe` blocks removed | 0 (8 blocks + 2 `unsafe fn` remain, all in `simd.rs`) |
| Golden vectors moved | **0** |
| Tolerance assertions changed | **0** |
| Conformance suites green | all, with counts identical to the Phase 02 capture |
| Net code change | Documentation only |

## The SIMD module: keep, decisively

### The premise was wrong in a way worth recording

The phase was framed as a comparison between "scalar-algebraic" and "SIMD-intrinsics",
with the delete branch triggered if the two landed within noise. That comparison does not
exist, because the two are not the same transform:

- `simd.rs` is the **reversible 5/3** transform. All 507 lines operate on `i32` with `+`,
  `-`, and arithmetic `>>`. It contains **no float arithmetic at all**, so `algebraic_add`
  and `algebraic_mul` — which are `f32`/`f64` methods — have no operator to substitute.
- The algebraic question belongs to the **irreversible 9/7** transform, which has no SIMD
  lane at all.

The [Float Drift Inventory](./float-drift-inventory.md) (`[[Float-Drift-Inventory]]`) had
already said this in Phase 02 ("Zero float tokens in 507 lines"); it is repeated here
because a plan naming `simd.rs` as an `algebraic_*` target is working from the file's
reputation rather than its contents, and that plan was written twice.

A second correction: **`dwt/mod.rs` does not choose between scalar and SIMD.** It only
declares `pub mod simd;`. The shipped codec path is hardcoded to the SIMD entry point at
[`writer.rs:151`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/writer.rs)
and
[`reader.rs:605`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-codestream/src/reader.rs);
`config.simd` exists only in the benchmark workload. The runtime detection inside
`simd.rs::kernels()` picks NEON, AVX2, or portable — never scalar. Deleting the module
would therefore not fall back to the scalar path, it would break the build.

### The numbers

Single-process interleaved A/B, nine alternating rounds, median of twelve iterations per
round, both lanes warmed first, and every lane asserted bit-identical to the scalar oracle
before anything was timed. AMD Ryzen Threadripper 9980X, `rustc 1.98.0`, `--release`, no
`target-cpu` override.

| Plane | scalar 5/3 | SIMD, portable lane | SIMD, AVX2 lane | SIMD vs scalar |
| --- | --- | --- | --- | --- |
| 256×256 | 350.1 µs | 79.8 µs | 77.3 µs | **4.53×** |
| 512×512 | 2 200.5 µs | 298.4 µs | 292.9 µs | **7.51×** |
| 1024×1024 | 14 394.9 µs | 1 248.7 µs | 1 218.9 µs | **11.81×** |
| 2048×2048 | 63 313.8 µs | 6 818.5 µs | 6 568.7 µs | **9.64×** |

The delete branch required scalar to land within noise of the intrinsics. It lands an
order of magnitude behind, and no algebraic conversion can narrow that, because the
transform is integer. **The module stays.**

The audit branch asks for the float arithmetic *inside* the intrinsic blocks, so the two
paths cannot diverge in output. There is none: the AVX2 block uses only
`_mm256_add_epi32`, `_mm256_sub_epi32`, and `_mm256_srai_epi32`, and NEON its integer
equivalents. Neither path rounds, so neither can drift.
`matches_scalar_bit_exactly` asserts bit equality over 9 geometries × 6 level counts, and
the measurement harness re-asserted it for both lanes at all four sizes above.

### The intrinsics are worth 1–3 %, and the restructuring is worth the rest

This is the part worth carrying forward. Within the module, the AVX2 intrinsics are only
**1–3 % ahead of the safe portable lane** — tight and reproducible at 512² and 1024²
(+1.1 %, +1.1 %, +1.9 %, +2.4 %), inside ±10 % noise at 2048².

The portable lane is not a scalar fallback: it autovectorizes to 128-bit SSE2 on its own
(475 `paddd`, 249 `psrad`, 260 `psubd`, and **zero** VEX-encoded ops in a build with the
AVX2 lane forced off). Doubling the vector width to AVX2's 256 bits buys almost nothing,
which says the loop is **memory-bound, not ALU-bound**.

So the ~10× win is the *row restructuring* — turning a strided column pass into a
contiguous row pass — and not the ISA-specific code. That is an argument for eventually
retiring the intrinsics, and it was deliberately **not acted on here** for two reasons:
1–3 % is a real, reproducible loss, not noise; and 5 of the 8 `unsafe` blocks are the NEON
lane, which cannot be measured at all on an x86-64 machine. Deleting a first-class
target's code path on the strength of a different ISA's numbers is exactly the kind of
unmeasured change this migration is structured to avoid. It is handed to the unsafe-audit
phase with the numbers attached.

:::{warning} The measurement that was thrown away
The first attempt compared two separately compiled `ndic-bench` binaries and reported the
portable lane **10 % faster** than AVX2. Adding one unrelated `std::env::var` call to the
workload and rebuilding both flipped the sign. Two builds differing in code layout are not
a controlled comparison; the entire result was a layout artifact and none of it appears
above. This is the same lesson Phase 03 recorded in a different costume — interleave the
A/B *inside one process*, or measure instructions instead of wall-clock.
:::

## Every remaining float site, and why none was converted

The sweep was a grep over every line in `crates/ndic-codestream/src/`,
`crates/ndic-zarr/src/`, and `crates/ndic-zfp/src/` carrying a float token together with an
arithmetic operator, then a read of each hit.

| Site | Finding | Action |
| --- | --- | --- |
| `quant.rs:219-222`, `irrev_delta` | Strict and algebraic are **bit-identical over the entire input domain** — all 262 144 (`v` × band) cases, 0 differing, 0 ulps | Not converted |
| `quant.rs`, read-path dequantization | **Does not exist.** `ndic-codestream/src/` has no float token outside `quant.rs`; dequantization is `shift = 31 - k_max`, an integer shift | Nothing to convert |
| `delta_codec.rs:153`, `delta_float!` diff | Bit-identical, no speedup | Not converted |
| `delta_codec.rs:163`, `delta_float!` cumsum | Bit-identical, no speedup, does not vectorize | Not converted |
| `series.rs:109` | `zfp_rate: Option<f64>` is a struct field serialized to JSON — no arithmetic in the file | Nothing to convert |
| `zfp_codec.rs:353` | `rate: f64` is a parameter forwarded to `BrickIndex::fixed_rate`; downstream size math is `u64`/`usize` | Nothing to convert |
| `ndic-zfp/src/chunk.rs:588-589` | `(v as f32) / 3.0` inside `#[cfg(test)]` — a fixture generator feeding the pinned checksums | Not converted |
| `ndic-zfp/src/lib.rs:96,99` | `rate > 0.0`, `tol >= 0.0` — comparisons, not arithmetic | Nothing to convert |

### `irrev_delta` is exact, not merely safe

`Quant::irrev_delta` returns `Δ = gain · (1 + μ/2^11) / 2^ε`. Run over its whole input
domain — every `u16` SPqcd word crossed with every band, 262 144 cases — the strict and
algebraic forms agree on every bit:

```text
domain              : 262144 (v in 0..=65535) x (band in 0..=3)
bit-differing pairs : 0
worst ulp gap       : 0
non-finite strict   : 0
inexact numerators  : 0
```

The reason is structural rather than lucky. `(v & 0x7FF) | 0x800` is an integer in
[2048, 4095]; the band gains are 1, 2, 2, 4; so the numerator is an integer in
[2048, 16380], far inside `f32`'s exact integer range of 2²⁴. Both divisors — the constant
2¹¹ and the derived 2^ε — are exact powers of two. **Every operation is exact, so
`algebraic_*` has nothing to license.** The function also still has zero callers anywhere
in `crates/`, `bench/`, or `bindings/`, exactly as the inventory found.

### `delta_float!` is a scan, not a reduction

This corrects the inventory on a point that matters for any future sweep.

The inventory classified `acc += v` in `delta_float!`'s cumsum as a **carried reduction**
— the shape `algebraic_add` exists to accelerate — and predicted that "the win is real,
and so is the break". Measured against the codec's real loop shape (in place over
`chunks_exact_mut(4)` with `from_le_bytes`/`to_le_bytes`, not a tidier `Vec<f32>` fold),
neither half of that is true:

| Fixture | n | Differing | Max ulps |
| --- | --- | --- | --- |
| The `delta_zarrs.rs` fixture, `i as f32` | 512 | 0 | 0 |
| Same shape, extended | 4 194 304 | 0 | 0 |
| Realistic float32, smooth + noise | 4 194 304 | 0 | 0 |
| Mixed magnitudes, 1e7 / 1e-3 | 4 194 304 | 0 | 0 |

Timing: −0.41 %, +1.87 %, −4.99 % across three interleaved rounds — no consistent sign.
Instruction census of the function both loops inline into: **10 `addss`, 2 `subss`, zero
packed float operations.** Neither form vectorizes.

A cumsum is a **prefix scan**, not a reduction. A reduction may reassociate freely because
only the final value is observable; a scan may not, because *every* partial sum is stored.
LLVM will not synthesize a vectorized scan, so the permission is simply inert here.

The off-limits ruling is unchanged and, if anything, firmer. The codec exists to reproduce
`numcodecs.delta` in NumPy element order so the Rust, Python, and TypeScript readers agree
byte-for-byte, and `algebraic_add` formally surrenders that guarantee even on a compiler
that declines to exercise it. **A license with no measured benefit is a pure increase in
risk.**

:::{note} An incidental finding about `numcodecs.delta` over floats
Rows 3 and 4 above do not round-trip exactly *in either form* — `strict_rt_exact=false`.
That is a property of the format being mirrored, not a defect: `diff` rounds, and `cumsum`
only undoes that rounding where the partial sums are exactly representable. Which is
precisely the regime `delta_zarrs.rs`'s `|i| i as f32` fixture lives in — one more reason
a green run of that test proves nothing about drift.
:::

## The `ndic-zfp` boundary

Confirmed by tracing the whole path rather than by reading the module header.

```text
encode_chunk (chunk.rs:366)
  → effective_shape + checked_elements        usize math
  → typed_vec::<T>                            bytemuck reinterpretation, no arithmetic
    or promoted_i32                           integer widening (u8/i8/u16/i16 → i32)
  → compress (lib.rs:249)
      → zfp_rs::ZfpBitStream::write_header
      → zfp_rs::ZfpBitStream::compress
      → into_vec
```

**Every coded byte is written by `zfp-rs`.** First-party code contributes shape
validation, a capacity query (itself a `zfp-rs` call), a field view over the samples, and
integer dtype promotion. No float arithmetic touches a sample on the way in. The brick
addressing — `BrickIndex`, `fixed_rate_stream_len` — is `u64`/`usize` throughout
(`div_ceil`, `checked_mul`, `next_multiple_of`), which is why the byte offsets are exact.

**[`crates/ndic-zfp/tests/checksums.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-zfp/tests/checksums.rs)
therefore cannot drift from any local `algebraic_*` change, and its checksums were left
untouched.** Both of its tests pin bytes produced entirely outside first-party control.

One caveat found while tracing, worth recording because it cuts against the intuition that
one-shot scalar math is harmless. The checksum *fixture data* is generated with local float
arithmetic — `(v as f32) / 3.0` in the test helpers — so converting **those** could move a
golden checksum even though the codec itself cannot. Over 100 000 fixture values,
`algebraic_div` currently produces **0 differences**; the reciprocal transform (`arcp`) it
licenses would change **32 668 of 100 000**. The permission is hazardous even where today's
LLVM declines to exercise it, which is a good general reason not to grant it without a
measured reason to.

## Conformance results

No shipped code changed, so nothing *could* move — but the suites were run anyway, because
the point of a baseline is to prove that rather than assume it. Every suite matched the
Phase 02 capture exactly, including its skip list, so none silently degraded into an empty
run.

| Suite | Result |
| --- | --- |
| `cargo test --workspace --release` | **207 passed, 0 failed** |
| `ndic-lift --test vectors --features serde` | 1 passed |
| `ndic-zfp --test checksums --features serde` | 2 passed |
| `ndic-htj2k --test openjph_differential` | 1 passed — *verified 2000 OpenJPH differential vectors* |
| `ndic-codestream --test openjph_interop` | 2 passed — bit-exact both directions |
| `ndic-codestream --test corpus_conformance` | 1 passed — 7 files bit-exact, 3 skipped (2× YUV 4:2:0, 1× multi-tile) |
| `ndic-zarr --release --features zarrs` | 53 passed across the lib and all five codec test files |

Cross-language surfaces, invoked the way
[`.github/workflows/ci.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/ci.yml)
does, so a drift cannot hide behind a binding:

| Surface | Result |
| --- | --- |
| Python binding — `maturin build --release`, wheel installed, `pytest -q` | **285 passed**, 0 failed, **0 skipped** |
| `scripts/ci/check-series-equality.py` | **all 148 cases identical across rust/python/typescript** |
| TypeScript binding — `npm ci`, `build:wasm`, `build`, `test` | **203 passed** across 6 files |
| `wasm32-unknown-unknown` and `wasm32-wasip2` release builds | both ok |

`simd.rs` is structurally safe on wasm and this is why: `kernels()` gates NEON on
`target_arch = "aarch64"` and AVX2 on `all(target_arch = "x86_64", feature = "std")`, so
both wasm targets take the `portable::KERNELS` arm and no `core::arch` intrinsic is
compiled in at all.

**No golden vector was regenerated and no tolerance assertion was changed.** Both halves of
the Phase 03 drift rule were exercised in measurement rather than in code — `irrev_delta`
over its whole domain, `delta_float!` over four fixtures of 4.2 M elements — and both
returned zero deviation, so neither reached the tolerance comparison.

:::{warning} `cargo build --workspace` does not target wasm, and never has
It fails in `getrandom v0.2.17`, reached through `ndic-cli` → `ureq` → `rustls` → `ring` —
an HTTP client, which has no meaning on `wasm32-unknown-unknown`. That is why CI's `wasm`
job is scoped to `-p ndic-zarr -p ndic-core`. Reproduced on a clean tree with zero local
changes, so it is structural rather than a regression.
:::

## What a later phase should take from this

1. **Read the file before naming it.** `simd.rs` was named as an `algebraic_*` target in
   two separate phase plans on the strength of its reputation. It contains no floats. The
   inventory's per-file rulings are cheaper to read than to rediscover.
2. **Scan ≠ reduction.** Only a *reduction* — where the intermediate values are discarded
   — has freedom to reassociate. A prefix scan stores every partial result and therefore
   has none, which is why `algebraic_add` is inert on `delta_float!`'s cumsum. Classify by
   *what is observable*, not by the shape of the `+=`.
3. **A license is not free just because it is currently unused.** `algebraic_div` changed
   nothing measurable on the ZFP fixture generator, yet the reciprocal transform it permits
   would change a third of the values. Grant the permission only where a measurement says
   it buys something.
4. **The unsafe worth auditing is narrower than it looks.** Of the SIMD module's ~10× win,
   the ISA-specific intrinsics account for 1–3 %; the rest is the row restructuring, which
   needs no `unsafe` in its kernels. The unsafe-audit phase has real numbers to work from
   — and needs an aarch64 machine before it can act on the NEON half. It did:
   [Unsafe Audit](./unsafe-audit.md) (`[[Unsafe-Audit]]`) kept both lanes on these
   numbers, removed the one `unsafe` block that was *not* an intrinsic, and took the
   workspace lint to `deny`.
5. **Interleave inside one process.** Two separately compiled binaries produced a
   confident, reproducible, and entirely false 10 % result. Code layout is a confounder at
   the same magnitude as the effects being measured.
