---
type: report
title: Rust 1.98 Adoption
short_title: 1.98 Adoption
description: The authoritative summary of the Rust 1.98 migration — the MSRV decision and what it costs on crates.io, PyPI, and npm; the measured performance delta with the toolchain effect separated from the code effect; and the evidence that no encoded byte moved.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - migration
  - performance
  - conformance
related:
  - '[[Capability-Probe]]'
  - '[[Float-Drift-Inventory]]'
  - '[[Algebraic-97-DWT]]'
  - '[[Algebraic-Codec-Sweep]]'
  - '[[Unsafe-Audit]]'
  - '[[Ergonomic-Sweep]]'
---

# Rust 1.98 Adoption

This is the one page to read to understand the whole migration. It pulls the seven phase
records into one narrative: why the MSRV moved to 1.98 in a single step and what that
costs consumers on three registries, what moved in the benchmarks and how much of it is
attributable to code rather than to the compiler, which golden values moved (**none**, and
that is established rather than assumed), where `algebraic_*` was tried and reverted, and
what the `unsafe` surface and the lint configuration look like on the other side.

Every number here is backed by a captured artifact. The final measurement and the final
conformance sweep were both taken at `9ae6dc4` on a clean tree under
`rustc 1.98.0 (88d9e12ae 2026-08-18)`, on the same AMD Ryzen Threadripper 9980X box as
the Phase 02 baseline at `cc0cd12`; the raw output lives in the playbook's
`Working/final-1.98/` folder and is reproducible with
[`scripts/rust198-remeasure.sh`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/rust198-remeasure.sh).

## The migration in one table

| | |
| --- | --- |
| MSRV | 1.91 → **1.98**, one step, set by this workspace rather than by a dependency |
| Encoded bytes changed | **0** — 41 ratio-carrying benchmark pairs bit-identical to the committed baseline |
| Golden values moved | **0** — no vector regenerated, no tolerance assertion edited |
| `algebraic_*` sites shipped | **0** — 2 converted and reverted; every other candidate measured and ruled out |
| `unsafe` keywords in first-party source | 10 → **9**; hand-written aliasing arguments 1 → **0** |
| Lines under an `allow(unsafe_code)` | 507 (a whole file) → **81** (one `#[cfg]`-selected module) |
| Workspace `unsafe_code` | `warn` → **`deny`** |
| 1.98 APIs applied | 4 sites, all `subslice_range` / `format_into`; **5 of the 7** APIs had no site at all |
| Largest code effect | `transform/dwt53_fwd_2048`, SIMD lanes, **−15 % to −18 %** |
| Workspace tests | 207 → **211**, 0 failed, 0 ignored |

## The pages

| Page | What it settles |
| --- | --- |
| [Adoption Notes](./adoption-notes.md) (`[[Rust-198-Adoption-Notes]]`) | The running phase log and the re-measure recipe — the record this page summarizes |
| [Capability Probe](./capability-probe.md) (`[[Capability-Probe]]`) | What 1.98 actually offers, measured on the pinned compiler, with exact signatures |
| [Float Drift Inventory](./float-drift-inventory.md) (`[[Float-Drift-Inventory]]`) | Which exactness suites can observe a float reassociation, and every float site in the tree |
| [Algebraic Float in the 9/7 DWT](./algebraic-97-dwt.md) (`[[Algebraic-97-DWT]]`) | The two conversions that were made, measured, and reverted |
| [Algebraic Codec Sweep](./algebraic-codec-sweep.md) (`[[Algebraic-Codec-Sweep]]`) | Every remaining float site, and why the hand-written SIMD module is kept |
| [Unsafe Audit](./unsafe-audit.md) (`[[Unsafe-Audit]]`) | Every `unsafe` block, what was removed, and the final lint configuration |
| [Ergonomic Sweep](./ergonomic-sweep.md) (`[[Ergonomic-Sweep]]`) | The four small-API conversions, and the three APIs with no site here |

## The MSRV decision

`[workspace.package] rust-version` moved from `"1.91"` to `"1.98"` on 2026-08-21, one day
after 1.98.0 shipped. All ten member crates inherit it through
`rust-version.workspace = true`; there are no per-crate overrides.
`rust-toolchain.toml` pins `1.98.0`, and CI's `dtolnay/rust-toolchain@stable` jobs honour
that pin, so contributors are moved automatically and nobody has to know the number.

Three things make the single step the right call rather than a reckless one, and they are
argued in full in the [Adoption Notes](./adoption-notes.md):

- **The adoption is aggressive on purpose.** 1.98's additions are the things this project
  was working around by hand — algebraic float arithmetic for the transform loops, and
  `AtomicU32::from_mut_slice` for the parallel-fill patterns a codec library eventually
  grows. Adopting late means carrying the workarounds *and* the eventual migration.
- **The floor is now a decision, not a report.** Until this change the MSRV tracked
  whatever `zarrs` required. It is now set by this workspace's own use of 1.98 standard
  library APIs, and the [Rust style page](../style/rust.md) was corrected to match.
- **The counterweight is that 1.98 is stable and pinned**, so the cost falls entirely on
  downstream consumers rather than on contributors.

:::{note} The irony worth recording
The MSRV was raised to use `algebraic_*`, and **no `algebraic_*` call ships**. What the
1.98 floor actually buys in the shipped artifact is four `subslice_range` /
`format_into` conversions and the confidence that the alternatives were measured rather
than assumed. That is a smaller return than the plan predicted, and it is the honest one
— see [Every `algebraic_*` site](#every-algebraic-site) below.
:::

### What downstream sees, per registry

The workspace publishes to three registries, and the new floor is visible in all three.
There is no way to soften it.

| Registry | Artifact | Effect of the 1.98 floor |
| --- | --- | --- |
| crates.io | `ndic-core`, `ndic-htj2k`, `ndic-codestream`, `ndic-lift`, `ndic-zfp`, `ndic-zarr`, `ndic-cli` (7 crates) | **Direct.** A pre-1.98 toolchain gets a resolver error, and `cargo add` refuses the new version rather than silently selecting an older one. |
| PyPI | `nd-image-codecs` wheel (`abi3-py311`, built by maturin from `ndic-py`) | **None for end users** — the wheel ships compiled. Building the binding *from source* needs 1.98. |
| npm | the TypeScript binding's prebuilt WASM bundle | **None for end users** — same reasoning. `npm run build:wasm` from a checkout needs 1.98. |

`ndic-bench-core`, `ndic-bench-cli`, and `ndic-py` are `publish = false` and never reach
crates.io; that is what keeps the Phase 01 capability probe, which lives in
`ndic-bench-core`'s `examples/`, out of any published crate.

The bump is recorded in
[`CHANGELOG.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/CHANGELOG.md)
as a breaking change for exactly this reason. The release procedure itself is unchanged —
see [Publishing](../publishing.md).

## Performance

Two effects are in play and they must not be quoted as one: the **toolchain effect**
(1.91 → 1.98, no code change) and the **code effect** (`cc0cd12` → `9ae6dc4`, same
compiler). They were measured separately.

### The toolchain effect

**On output: exactly zero.** Phase 02 established it for the compiler bump alone across 29
ratio-carrying record pairs; the final run extends it to **41** pairs — 29 Rust plus the 12
Python lanes — every one of which reproduces the committed `bench/baselines/main/` to full
`f64` **bit** equality, compared as a bit pattern rather than to a printed precision.

**On throughput: not measured, and not measurable from the artifacts this project has.**
`bench/baselines/main/manifest.json` was captured on `wsl2-aarch64-12core-dev` under
`rustc 1.91.0`. Comparing it against an x86-64 run confounds compiler, CPU, machine class,
and SIMD backend — the `simd-*` lanes are NEON against AVX2. The median −24 % that a naive
`compare main` reports is **not** a 1.98 number and is not quoted as one anywhere in this
migration. Getting a real one needs 1.91 and 1.98 builds alternated on one box, which no
phase did because no decision depended on it.

### The code effect, per workload

Measured the only way that survives scrutiny: a `git worktree` at `cc0cd12` built with the
same toolchain and the same tracked `.cargo/config.toml` (verified identical — no
`target-cpu`, so both are baseline `x86-64`), the two `ndic-bench` binaries run
**alternately in one sitting**, five rounds each, reported as min-of-five, with a
`--filter` pass so both execute an identical workload set.

| Workload | Lanes | Code effect | Attributed to |
| --- | --- | --- | --- |
| `transform/dwt53_fwd_2048` | `simd-53-ht`, `simd-53-lift-z2`, `simd-97-ht`, `zfp-rate8`, `zfp-reversible` | **−15 % to −18 %** | `split_three` (below) |
| `transform/dwt53_fwd_2048` | the six scalar lanes | −1 % to −6 % — **noise floor**, see below | — |
| `htj2k/plane_encode_1024` | all | −1.5 % to −3.8 % | unattributed; the SIMD DWT is a component of it |
| `htj2k/plane_decode_1024` | all | −0.9 % to 0.0 % | — |
| `lift/*`, `lift_codec/*`, `lift_ht/*`, `zfp/*` | all | within ±3 %, no consistent sign | — |
| `transform/dwt97_fwd_2048` | all | new lane, no baseline | added by Phase 03 |

The scalar `dwt53` row is called noise **on evidence, not on principle**: the six scalar
configs run byte-identical code through `dwt::forward_53` — the workload selects the lane
on `BenchConfig::simd`, and nothing on that path changed — yet their deltas scatter
−1.9 / −3.3 / −4.4 / −5.4 / −5.4 / −6.2 %. A spread that wide across configs doing
identical work *is* the measurement's noise floor at a 62 ms workload, so no claim
narrower than it can be made about that lane.

:::{warning} The whole-suite comparison is not the code effect
`compare-vs-pre.txt` reads −11 % to −23 % on several workloads and flags one
`TIME-REGRESSED` (+11.0 % on `lift/inverse_zyx_32x64x64`). Neither figure describes this
branch. Phase 03 registered `transform/dwt97_fwd_2048`, which runs ~68 ms × 11 configs
*immediately before* the three workloads whose apparent delta is largest — so the two
binaries are not executing the same sequence (59 records against 48 is the same fact).
And the flagged regression sits on a 25 µs workload behind `ndic-lift`, which has not
changed a line since `cc0cd12`; it is machine state hours apart on a shared box. This is
why the interleaved A/B exists.
:::

(the-split-three-gain)=

### The one change that moved the needle

The SIMD DWT gain is Phase 05's `split_three` rewrite — replacing three slices conjured
from one `*mut i32` with two `split_at_mut` calls — and nothing else. A three-arm
interleaved run attributes it, where arm B is `cc0cd12` with **only**
`crates/ndic-htj2k/src/dwt/simd.rs` patched forward:

```text
config                A cc0cd12  B +splitter    C HEAD      B-A      C-A
simd-53-ht                6.170        5.230     5.130  -15.24%  -16.86%
simd-53-lift-z2           6.140        5.200     5.110  -15.31%  -16.78%
simd-97-ht                6.130        5.230     5.130  -14.68%  -16.31%
zfp-rate8                 6.090        5.050     5.050  -17.08%  -17.08%
zfp-reversible            6.090        5.050     5.010  -17.08%  -17.73%
```

B reproduces essentially all of C. It is also the only candidate the diff leaves standing:
`dwt/mod.rs` differs from `cc0cd12` in comments only, and the `Cargo.toml` change is lint
levels with no `[profile]` edit. An independent single-function A/B — two binaries from
one tree differing only in that function's body — reads **−10.3 % to −12.6 %**. Both are
the same sign and the same order; **the range to quote is 10–17 %**.

The mechanism is **not** established. The plausible story is that the raw-pointer form
gave LLVM three slices whose provenance it could not relate, while `split_at_mut` yields
provably disjoint borrows; confirming that needs the assembly rather than another timing,
and it is recorded as a hypothesis in the [Unsafe Audit](./unsafe-audit.md#splitter-cost).

(the-measurement-lesson)=

### The measurement lesson

Phase 05 originally recorded that same rewrite as a **+1 % cost** — opposite sign, an
order of magnitude smaller. Phase 07 settled it and **Phase 05 was wrong**, but not
because it measured carelessly. Its scratch harness was pinned, interleaved,
order-alternating, bit-exactness-checked, and consistent across four plane sizes; re-run,
it still reads +0.13 / +0.84 / +1.20 / +1.36 %. Three different call shapes were then
tried in one process — including the shipped one — and **none of them contains the
effect**, so the harness under-models the transform rather than mis-calling into it: it
drives the vertical forward pass with two of the four kernels, no horizontal pass, no
`interleave_rows`, and no `forward_53` driver.

**A scratch harness that isolates a function is measuring the harness until something ties
it to the shipped path.** The tie-back that would have caught this costs one extra build:
compile the real binary twice, changing only the function under test. Two earlier
measurement artifacts in this migration point the same way — a "13 % slower" reading from
two uncontrolled runs (Phase 03) and a "portable lane is 10 % faster than AVX2" reading
that flipped sign when an unrelated `std::env::var` call was added (Phase 04).

(golden-values)=

## Golden values: none moved

**No golden value moved during this migration, so there is no delta to report and no
tolerance rationale to justify one.** No vector was regenerated, no checksum was
refreshed, and no tolerance assertion was edited in any phase. That is a strong claim, so
here is what establishes it rather than assumes it:

| Evidence | Result |
| --- | --- |
| 41 ratio-carrying benchmark pairs vs `bench/baselines/main/` | bit-identical as `f64` bit patterns; `compare-vs-pre` shows `ratio == base ratio` on every row |
| `ndic-lift --test vectors` | 1 passed, bit-exact vs `fixtures/nd-lift/vectors.json` |
| `ndic-zfp --test checksums` | 2 passed, bit-exact vs `fixtures/zfp/checksums.json` |
| `ndic-htj2k --test openjph_differential` | 1 passed — **2000 OpenJPH oracle vectors** verified |
| `ndic-codestream --test openjph_interop` | 2 passed — bit-exact in both directions against `ojph_compress` / `ojph_expand` |
| `ndic-codestream --test corpus_conformance` | 1 passed — **7 corpus streams bit-exact**, the same 3 documented YUV 4:2:0 / multi-tile skips as Phase 02 |
| `simd::tests::matches_scalar_bit_exactly` | 9 plane geometries × 6 level counts, forward and inverse, including the degenerate and odd shapes where the mirror branches fire |
| Phase 06 byte capture | **65 artifacts** — packet dumps, every `index` target at every level and eight pixel budgets in both output formats, SHA-256 of every encoded stream and decoded image — all identical across the change |
| `check-series-equality.py` | **148 cases identical** across the Rust, Python, and TypeScript builders |

Counts matched the Phase 02 capture in every phase, which matters as much as the passes:
the [Float Drift Inventory](./float-drift-inventory.md) established that six test files
report `ok. 0 passed` when run per-crate without their feature, so a suite that quietly
became empty would otherwise read as a suite that passed.

(what-could-have-moved)=

### The ledger of what could have moved

Every deviation that *was* measured, and the argument that kept it out of the shipped
tree. This is where the tolerance reasoning lives, even though nothing reached the point
of needing it.

| Candidate | Measured deviation | Why nothing moved |
| --- | --- | --- |
| `lift_97` algebraic, shipped target (baseline `x86-64`) | `differing = 0 / 1 048 576`, `max_ulps = 0` over four planes | Strict and algebraic compile to **identical instructions** — no FMA in the baseline target, and no reduction to reassociate |
| `lift_97` algebraic, `-C target-cpu=native` | max abs 1.504e-1 on a peak coefficient of 272 370 = **5.522e-7 relative** (≈2⁻²¹) | Reaches one irreversible quantization step (`Δ = gain·(1 + μ/2¹¹)/2^ε`) only at **ε ≈ 20.8**; the 9/7 path exists to quantize *more* coarsely, so nothing that selects it sits near 21 bits. One to three orders of magnitude of headroom — and the drift is an *improvement*, since an FMA rounds once where the strict form rounds twice |
| `dwt97_roundtrips_within_tolerance` | asserts `< 1e-2`; worst observed error `1.068e-4` | ~94× of headroom. **This test would not have caught the change at any plausible magnitude** — recorded because a green run here proves nothing |
| `Quant::irrev_delta` algebraic | 0 bit differences, 0 ulps over the **entire input domain** (262 144 cases) | Structurally exact: the numerator is an integer in [2048, 16380], inside `f32`'s exact 2²⁴, and both divisors are powers of two. There is nothing for `algebraic_*` to license |
| `delta_float!` algebraic | 0 differing, 0 ulps over 4 fixtures of 4.2 M elements | Forbidden regardless — the codec exists to reproduce `numcodecs.delta` in NumPy element order so the Rust, Python, and TypeScript readers agree byte-for-byte, and a `cumsum` is a **prefix scan**, which may not reassociate because every partial sum is stored |
| `algebraic_div` in the ZFP checksum fixture generator | 0 differences under today's LLVM; the reciprocal transform (`arcp`) it permits would change **32 668 of 100 000** values | The clearest case in the migration that **a license is not free just because it is currently unused** |

(every-algebraic-site)=

## Every `algebraic_*` site

**Zero `algebraic_*` calls ship.** Two sites were converted and reverted; eight more were
examined and never converted. Full evidence in
[Algebraic Float in the 9/7 DWT](./algebraic-97-dwt.md) and the
[Algebraic Codec Sweep](./algebraic-codec-sweep.md).

### Reverted

| Site | Expression | What ended it |
| --- | --- | --- |
| `dwt/mod.rs::lift_97`, high-from-low branch | `high[i] += coeff * (l + r)` | Instruction census: `4 × addss, 2 × mulss` in **both** forms on the shipped target — byte-identical codegen. Zero packed (`ps`) operations in any configuration. Interleaved A/B on `transform/dwt97_fwd_2048`: +0.8 / +0.6 / +1.0 / −3.8 %, no consistent sign inside a harness whose own spread is ±8 % |
| `dwt/mod.rs::lift_97`, low-from-high branch | `low[i] += coeff * (l + r)` | Same |

**No test forced either revert, and no test failed at any point in this migration.** The
phase's own decision rule did: a converted site with no vectorization change and no
measurable speedup is a net loss, because it buys an accuracy license for nothing. The
test that would have adjudicated a numeric regression —
`dwt97_roundtrips_within_tolerance` — passed in both forms with 94× of headroom and is
listed above precisely because it *could not* have caught the change.

Two independent reasons stack up behind the codegen result, and both generalize:
`lift_97` is **elementwise with no carried reduction**, so there is nothing to reassociate;
and `.cargo/config.toml` sets no `target-cpu`, so the baseline `x86-64` target applies and
there is no FMA to contract into. What actually blocks vectorization is neither — it is
the symmetric-extension index clamps, `(i + 1).min(nl - 1)` and
`i.saturating_sub(1).min(nh - 1)`, which no amount of arithmetic permission addresses.

### Examined and not converted

| Site | Finding |
| --- | --- |
| `dwt/mod.rs::syn_97_1d` | Compile-time constant sign flips — exact under IEEE, nothing to license |
| `dwt/mod.rs::ana_97_1d`, `forward_97`, `inverse_97` | De-interleave and gather/scatter; all arithmetic is `usize` index math. There is **no per-coefficient scaling step** in this 9/7 implementation — like OpenJPH, `K` is absorbed into the per-subband quantizer |
| `quant.rs::irrev_delta` | Bit-identical over its whole domain, and still zero callers |
| `quant.rs`, read-path dequantization | **Does not exist** — `ndic-codestream/src/` has no float token outside `quant.rs`; dequantization is an integer shift |
| `delta_codec.rs::delta_float!`, diff and cumsum | Bit-identical, no speedup, does not vectorize (10 `addss`, zero packed ops), and off limits on the cross-language byte-parity contract |
| `ndic-zfp/src/{lib,chunk}.rs`, `ndic-zarr/src/{series,zfp_codec}.rs` | `rate` / `tolerance` configuration and dtype dispatch — forwarded to `zfp-rs`, never computed on |
| `bench/rs/ndic-bench-core` — `sigma`, `ratio`, the comparer | The measuring instrument. Changing it mid-migration invalidates every before/after number |
| `dwt/simd.rs` | **Zero float tokens in 507 lines.** It is the integer 5/3 lane. Named as a prime `algebraic_*` target in two separate phase plans, on the strength of the file's reputation rather than its contents |

The two premises the plan started from both failed on contact with the code, and that is
the most reusable output of Phases 03–04: **there is no reachable float arithmetic in any
codec path in this workspace.** Every byte any codec emits is produced by integer
arithmetic or by `zfp-rs`. The 9/7 DWT and the irreversible quantizer are unreachable —
`writer.rs` rejects `WaveletKind::Irreversible97` outright — and the one reachable float
loop is off limits by contract.

## `unsafe`, before and after

Full inventory, with every block's line number and safety argument, in the
[Unsafe Audit](./unsafe-audit.md).

| | Before | After |
| --- | --- | --- |
| Files containing `unsafe` | 1 | 1 |
| `unsafe` keywords in first-party source | 10 | **9** |
| Blocks resting on a hand-written aliasing argument | 1 | **0** |
| Lines covered by an `allow(unsafe_code)` | 507 (a whole file) | **81** (one `#[cfg]`-selected module) |
| `unsafe` reachable on a wasm build | 1 | **0** |

Only one lane is ever compiled — `mod neon` is `#[cfg(target_arch = "aarch64")]` and
`mod avx2` is `#[cfg(target_arch = "x86_64")]` — so an x86-64 build sees 5 occurrences, an
aarch64 build 6, and every other target, both wasm targets included, now sees **none**.

**What was removed** is `split_three`, and it turned out to be the performance story of
the whole branch (see [above](#the-split-three-gain)). It was not a 1.98 API that made it
possible: `split_at_mut` has been stable since 1.0, and the block survived because nobody
re-derived the split. The audit's value here was looking again.

**What is kept** is the NEON and AVX2 kernel code. It cannot be written safely while
`core::simd` is unstable — portable SIMD is not in 1.98 — and the module carries the
shipped codec path, measured at **4.53× / 7.51× / 11.81× / 9.64×** the scalar oracle at
256², 512², 1024², and 2048². The narrower question is genuinely open: the AVX2 intrinsics
are worth only **1–3 %** over the safe portable lane, which autovectorizes to 128-bit SSE2
by itself, so most of that ~10× is the row restructuring rather than the ISA-specific
code. Acting on it needs an aarch64 host to answer the same question for NEON, and is
[carried forward](#carried-forward).

**What the atomics half of the plan found was nothing to convert.**
`AtomicU32::from_mut_slice` exists to rescue raw-pointer aliasing between concurrent
workers, and this workspace has no concurrency inside any codec: zero `rayon` uses, zero
`thread::scope`, zero `UnsafeCell`, zero `transmute`, and one `thread::spawn` that is a
test HTTP server. Parallelism is the caller's business — `zarrs` over chunks, the
browser's worker pool over the WASM module. The API is recorded as the right tool *if*
block-level parallelism is ever added to the HT coder.

### The final lint configuration

In the root `Cargo.toml`, inherited by all ten member crates via `[lints] workspace = true`
(verified: every one of the ten manifests carries it):

```toml
[workspace.lints.rust]
unsafe_code = "deny"
unsafe_op_in_unsafe_fn = "deny"
missing_docs = "warn"
```

The only overrides in the workspace are the two module-scoped `#[allow(unsafe_code)]`
attributes on `mod neon` and `mod avx2`, each with a written justification. There is no
per-crate override anywhere, and none is needed for the binding crates.

`deny` rather than `forbid` is deliberate — `forbid` cannot be overridden, and those two
modules need an override. The deny is enforced rather than merely configured: a temporary
`unsafe {}` inserted into `simd.rs::forward_53`, in the same file as both overrides but
outside them, fails the build with `error: usage of an unsafe block`.

Two limits of that guarantee, both verified rather than assumed:

- **`deny(unsafe_code)` does not see macro-generated `unsafe`.** `#![forbid(unsafe_code)]`
  on the PyO3 and `wasm-bindgen` modules compiles clean even though `pyo3-macros-backend`
  demonstrably emits `unsafe` in its `quote!` templates. Good news for the cost (the
  binding crates need no override); bad news for the reading (the lint is a statement
  about **diffs**, not about artifacts). Auditing that surface needs `cargo expand`.
- `unsafe_op_in_unsafe_fn` was turned on in the same change and cost nothing, because both
  `rows` functions already used explicit inner `unsafe {}` blocks — which is exactly why
  it was worth doing before anything came to depend on the laxer edition default.

## What the 1.98 APIs actually bought

Six API groups were probed on the pinned compiler and all six are stable on 1.98.0 with
no `UNAVAILABLE` — the exact signatures are in the
[Capability Probe](./capability-probe.md). What they bought in this codebase:

| API | Sites | Outcome |
| --- | --- | --- |
| `<[T]>::subslice_range` | **3** | The two `reader.rs` marker loops and the `packet.rs` packet-header reader. Each was a slice and a separately computed offset *for* that slice, travelling together and free to disagree; one derivation replaces each pair. `HeaderBitReader::new_at` became `new_in(parent, sub, base)` — the one consumer-visible API change of the migration |
| `u64::format_into` + `NumBuffer` | **1** | The integer branch of the bench reporter's `fmt_ns`: 23.4 → 8.9 ns/call, −62 %, byte-identical output. Recorded at its true size — three cells per record against records that take milliseconds to produce |
| `f32`/`f64::algebraic_*` | **0** | See [above](#every-algebraic-site) |
| `AtomicU32::from_mut_slice` | **0** | No shared-mutable-slice hand-off exists to rescue |
| `str::strip_circumfix` | **0** | The CLI strips exactly one prefix (`@file`, no suffix), and `strip_circumfix` yields `None` unless both affixes match |
| `NonZero::from_str_radix` | **0** | No radix parsing, and no parse-then-check-zero to collapse |
| `str::substr_range` | **0** | Nothing recovers a position in a string it already holds |

**Five of the seven had no site at all**, and three of those five — `strip_circumfix`,
`NonZero::from_str_radix`, and `substr_range` — were named specifically against
`ndic-cli`, which strips exactly one prefix, parses no radix, and never asks where a
substring sits. One more site was named and had nothing either, for a reason worth
keeping: `range.rs` was expected to want `subslice_range` because the plan builder
computes byte offsets. It does — it computes those offsets from an *index of integers*, never from a slice
of the chunk. "Computes offsets" and "holds a subslice whose offset it needs" read the
same from outside and are not the same thing. **An API list drawn from a release
announcement describes what the language gained, not what a codebase does.**

## Conformance at the close

Every gate CI runs, at `9ae6dc4`:

| Gate | Result |
| --- | --- |
| `cargo test --workspace --release` | **211 passed, 0 failed, 0 ignored** |
| The five exactness suites, individually | all pass with non-zero counts — see the [golden-value table](#golden-values) |
| `ndic-zarr` codec tests `--features zarrs` | **53 passed** |
| Python binding — wheel → `pytest` | **285 passed, 0 skipped**, native extension present |
| TypeScript binding — `build:wasm` → `build` → `test` | **203 passed** across 6 files |
| `check-series-equality.py` | **148 cases identical** across all three implementations |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, both feature sets |
| `cargo clippy -p ndic-zarr -p ndic-core --target wasm32-unknown-unknown` / `wasm32-wasip2` | clean on both |
| `cargo fmt --all --check` | clean |
| `docs`: `check-docs-toc.py` + `npm run check` (strict) | exit 0 |

The workspace count moved 207 → 211, and the +4 is entirely Phase 06:
`ndic_codestream`'s unit tests 33 → 36 plus the bench reporter's `fmt_ns` test. Nothing
else in the tree gained or lost a test.

Two boundaries are worth naming so nobody rediscovers them as regressions:

- **`--workspace --all-targets` does not build for either wasm target, and never has.** It
  reaches `getrandom` and `wait-timeout` through `ndic-cli`'s HTTP stack — an HTTP client
  and a process-timeout helper, neither of which means anything on wasm. On `wasm32-wasip2`
  the `--all-targets` form additionally needs a wasi sysroot for `zstd-sys`'s C build,
  reached through `ndic-zarr`'s *dev*-dependency on `zarrs`. Both are structural, both
  reproduce on a clean tree, and both are why CI scopes its `wasm` job to
  `-p ndic-zarr -p ndic-core`.
- **`cargo test --workspace --release` does not link in this repo**, on 1.98 and equally on
  1.91. `[profile.release]` sets `panic = "abort"`, so `cargo test` builds the dependency
  graph twice, and `ndic-zarr`'s `cdylib` carries no metadata hash, so the two builds
  collide on one filename ([cargo#6313](https://github.com/rust-lang/cargo/issues/6313)).
  `scripts/rust198-remeasure.sh` passes `--config profile.release.panic="unwind"`, which
  collapses the graphs. A panic strategy cannot move a golden value, but the real fix is
  still owed.

### Records corrected

The final sweep is also where the phase records were checked against reality rather than
against each other. Five corrections landed:

1. **The `split_three` rewrite is a 10–17 % gain, not a ~1 % cost** — the sign error
   described [above](#the-measurement-lesson). `unsafe-audit.md` and `adoption-notes.md`
   were rewritten with both measurements and the lesson.
2. **Phase 06 added four tests, not three.** The fourth is the `format_into` rendering
   test, described in the sweep's own prose and missing from its list; it is what makes
   207 → 211.
3. **`ndic-codestream` has 19 integration tests, not 20.**
4. **Four more test files are green-and-empty without their feature.** `ndic-zarr`'s
   `{delta,htj2k,lift,zfp}_zarrs.rs` are all `#![cfg(feature = "zarrs")]`, so
   `cargo test -p ndic-zarr --release` reports 19 where `--features zarrs` reports 53:
   **34 tests green and absent** — every registered-codec round-trip in the workspace.
   Nothing was untested (the workspace run passes the feature), but the per-crate form is
   what a person types when narrowing a failure.
5. **Not a discrepancy:** `unsafe-audit.md`'s 207 was correct when written.

(carried-forward)=

## Carried forward

Open at the close of the migration, with what each one needs:

1. **Measure NEON against the portable lane on aarch64 hardware.** If the gap is the same
   1–3 % AVX2 shows, both intrinsic lanes can go, the row restructuring that produces the
   actual ~10× stays, and the workspace can take `forbid(unsafe_code)`.
2. **`neon::rows` can drop its `unsafe fn`** — 5 occurrences to 1 — but only with an
   aarch64 differential test behind it. An untested edit to the shipped DWT is exactly
   what this migration is structured to avoid.
3. **Establish why `split_at_mut` is 10–17 % faster.** The provenance hypothesis is
   plausible and unproven; it needs the assembly, not another timing.
4. **The 9/7 kernel's real bottleneck is the boundary clamps.** Peeling the first and last
   iterations so the interior is unconditionally stride-1 is the change that would
   vectorize it — an operation-sequence change, out of scope for an operators-only phase.
5. **`rayon` and `wide` are declared in `[workspace.dependencies]` and used by nothing.**
   `AGENTS.md` documents both as if they were load-bearing. Either wire them up or drop
   them.
6. **`bench/baselines/main/` is aarch64 under 1.91.** Since no ratio moved, the ratio gate
   is still valid against it; its timings never were comparable to an x86-64 box. Whether
   to refresh it is a separate decision with its own reviewed workflow —
   [`bench-baseline-refresh.yml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/.github/workflows/bench-baseline-refresh.yml).

## The five lessons worth keeping

Stated generally, because each cost a phase to learn:

1. **Read the file before naming it.** `dwt/simd.rs` was named as an `algebraic_*` target
   in two separate phase plans and contains no floats. The 9/7 kernel was named as a
   performance target and has no callers.
2. **Check the loop shape and the target features before predicting a float win.** Only a
   *carried reduction* can reassociate; a prefix scan stores every partial sum and cannot.
   And every FMA-dependent argument is void while the workspace builds for baseline
   `x86-64`.
3. **A license is not free just because it is currently unused.** `algebraic_div` changed
   nothing measurable on the ZFP fixture generator, yet the reciprocal transform it
   permits would change a third of the values.
4. **Interleave the A/B inside one process, and tie it back to the shipped binary.** Two
   separately compiled binaries produced a confident, reproducible, and entirely false
   10 % result in Phase 04; an isolated harness produced a confident, reproducible, and
   sign-inverted 1 % result in Phase 05.
5. **An `allow` at file scope is a permanent blind spot, not a local exception.** The one
   hand-written aliasing argument in the workspace sat under a 507-line `allow` for as long
   as it existed. Narrowing the scope is what turns the next one into something a reviewer
   has to see.
