---
type: analysis
title: Unsafe Audit
short_title: Unsafe Audit
description: Every unsafe block in the workspace, taken fresh rather than inherited from an earlier list — what was removed, what is kept and why, why the new atomic slice views had nothing to convert, and the lint configuration that now makes the next unsafe block an argument rather than a warning.
created: 2026-08-21
date: 2026-08-21
tags:
  - rust-198
  - unsafe
  - safety
related:
  - '[[Rust-198-Adoption-Notes]]'
  - '[[Capability-Probe]]'
  - '[[Algebraic-Codec-Sweep]]'
---

# Unsafe Audit

Phase 05 of the [Rust 1.98 adoption](./adoption-notes.md) (`[[Rust-198-Adoption-Notes]]`)
was planned around
`Atomic<T>::from_mut_slice` — the 1.98 API that lets concurrent workers share disjoint
mutable pieces of one buffer without a raw-pointer cast. The plan assumed that pattern
existed here. **It does not.** This page records the search that established that, the
complete `unsafe` inventory it produced along the way, the one site the audit removed,
the two it keeps, and the lint change that is the phase's actual deliverable: workspace
`unsafe_code` moved from `warn` to `deny`.

The short version, before the evidence:

| | Before | After |
| --- | --- | --- |
| Files containing `unsafe` | 1 | 1 |
| `unsafe` keywords in first-party source | 10 | 9 |
| `unsafe` blocks whose safety argument is hand-written aliasing reasoning | 1 | **0** |
| Lines covered by an `allow(unsafe_code)` | 507 (a whole file) | **81** (one `#[cfg]`-selected module) |
| Workspace `unsafe_code` | `warn` | **`deny`** |
| Workspace `unsafe_op_in_unsafe_fn` | unset (edition default `warn`) | **`deny`** |

## How the inventory was taken

Reproducible, and deliberately re-run from scratch rather than copied from Phase 04's
carry-forward note. Over `crates/`, `bindings/`, and `bench/rs/`:

```bash
grep -rnw  "unsafe"                --include="*.rs" crates/ bindings/ bench/
grep -rn   "unsafe fn"             --include="*.rs" crates/ bindings/ bench/
grep -rn   "unsafe impl"           --include="*.rs" crates/ bindings/ bench/
grep -rn   "unsafe_code"           --include="*.rs" --include="*.toml" crates/ bindings/ bench/ Cargo.toml
grep -rn   "unsafe_op_in_unsafe_fn" -r --include="*.rs" --include="*.toml" .
```

`grep -w unsafe` is the one that matters: it is a superset of the other four and it
catches an `unsafe` this audit would otherwise have missed — the one inside a
`macro_rules!` body, which `unsafe {` alone finds but which is easy to under-count
because it expands four times.

Two negative results are worth recording because they bound the audit:

- **`unsafe impl`: zero hits.** No `Send`/`Sync` is asserted by hand anywhere in the
  workspace.
- **`build.rs`: none exist.** `find . -name build.rs` outside `target/` returns nothing,
  so there is no build-script code path outside the lint's reach.

## The inventory, before this phase

Every first-party `unsafe` in the workspace lived in one file,
[`crates/ndic-htj2k/src/dwt/simd.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/crates/ndic-htj2k/src/dwt/simd.rs),
under a single file-scoped `#![allow(unsafe_code)]` on line 12. Line numbers are as of
`fa533a7`.

| Line | Item | What it does | Why it is `unsafe` | Safe replacement in 1.98? |
| --- | --- | --- | --- | --- |
| 77 | `neon::rows` — `unsafe fn` | Row-lifting driver over NEON intrinsics | Calls raw-pointer load/store intrinsics | **No** — see [What is kept](#what-is-kept) |
| 88 | `unsafe {}` in `neon::rows` | `vld1q_s32` ×3, `vst1q_s32` ×1 over `ptr.add(i)` | `core::arch` loads/stores take `*const i32` / `*mut i32` | No |
| 105, 117, 134, 151 | `unsafe {}` ×4 in `neon::{predict_sub, update_add, update_sub, predict_add}` | Each calls `rows` with its own step + tail closure | Calling an `unsafe fn` | Partly — see the [`target_feature` note](#the-target-feature-note) |
| 179 | `avx2::rows` — `#[target_feature(enable = "avx2")] unsafe fn` | Same driver over AVX2 intrinsics | Raw-pointer intrinsics **and** a target-feature contract | No |
| 190 | `unsafe {}` in `avx2::rows` | `_mm256_loadu_si256` ×3, `_mm256_storeu_si256` ×1 | Same | No |
| 209 | `unsafe {}` inside `macro_rules! kernel` | Calls `rows`; **expands four times** into `predict_sub`/`update_add`/`update_sub`/`predict_add` | Calling a `#[target_feature]` `unsafe fn`; the caller owes the AVX2 guarantee, discharged by `is_x86_feature_detected!` in `kernels()` | Partly — see the [`target_feature` note](#the-target-feature-note) |
| 357 | `unsafe {}` in `split_three` | Builds `(&[i32], &mut [i32], &[i32])` for three rows of one plane out of a single `*mut i32` | Aliasing: three slices conjured from one pointer, disjointness argued in a comment | **Yes** — removed, see [What was removed](#what-was-removed) |

Only one lane is ever compiled: `mod neon` is `#[cfg(target_arch = "aarch64")]`, `mod avx2`
is `#[cfg(target_arch = "x86_64")]`. So of the ten source occurrences, an x86-64 build
sees five (lines 179, 190, and 209 expanded four times), an aarch64 build sees six, and
every other target — including both wasm targets — sees **one**, the `split_three` block,
which is the one this phase removed. On wasm the workspace is now `unsafe`-free.

### The bindings: `unsafe` the lint cannot see

The plan flagged the PyO3 and `wasm-bindgen` crates as places where `unsafe` is generated
by macros and cannot be removed by hand. Both halves of that need correcting, in opposite
directions.

`pyo3-macros-backend` does emit `unsafe` — its `quote!` templates contain 58 `unsafe`
tokens, including the `unsafe { … }` block `#[pymodule]` wraps around module
initialisation. So the expanded `ndic-py` really does contain `unsafe`, and no hand edit
can remove it.

But the lint never fires on it. Verified directly rather than assumed: adding
`#![forbid(unsafe_code)]` — which, unlike `deny`, **cannot** be overridden by any inner
attribute — to `bindings/python/nd-image-codecs/src/lib.rs` and to
`crates/ndic-zarr/src/wasm.rs` leaves both compiling clean. rustc suppresses lints on
code originating in an external macro expansion, so `unsafe_code` at any level is a
statement about *first-party* source only.

That cuts both ways and the audit should say so plainly:

- **Good:** the workspace `deny` costs the binding crates nothing. No per-crate override
  is needed for `ndic-py` or for `ndic-zarr`'s `wasm` feature.
- **Bad:** `deny(unsafe_code)` is *not* a guarantee that the compiled artifact contains no
  `unsafe`. It is a guarantee about what a reviewer will see in a diff. The `unsafe` in a
  PyO3 module initialiser is real, is trusted on pyo3's reputation rather than on this
  lint, and would be equally invisible if it were wrong.

(shared-mutable-slice-search)=

## What the atomics API had to work with: nothing

`AtomicU32::from_mut_slice` earns its keep when disjoint mutable pieces of one buffer are
handed to concurrent workers. The search for that pattern, and what it returned:

| Pattern searched | Hits | What they are |
| --- | --- | --- |
| `rayon`, `par_iter`, `par_chunks`, `into_par_iter` in `*.rs` | **0** | — |
| `rayon` in any `Cargo.toml` | 1 | `[workspace.dependencies] rayon = "1"` in the root manifest — **declared and never used**. No member crate lists it. |
| `std::thread::scope` | **0** | — |
| `std::thread::spawn` | 1 | `crates/ndic-cli/tests/plans_cli.rs:242` — a test-only HTTP server thread for the Range-request CLI tests. Shares a `Vec<u8>` by `move`, not by split borrow. |
| `UnsafeCell` | **0** | — |
| `transmute` | **0** | — |
| `from_raw_parts` / `from_raw_parts_mut` | 3 | All three in `split_three`, single-threaded — removed by this phase. |
| `split_at_mut` | 9 | `ndic-lift/src/kernel.rs` ×6, `ndic-codestream/src/reader.rs` ×2, and (new here) `simd.rs` ×2. Every one is single-threaded and already safe. |
| `chunks_mut` | 2 | `ndic-lift/src/chunk.rs` — single-threaded slab iteration. |
| `Atomic*` / `sync::atomic` | 1 file | `bench/rs/ndic-bench-core/examples/rust198_probe.rs` — the Phase 01 capability probe, which is where `from_mut_slice` is exercised and nowhere else. |

**The explicit finding the plan asked for: this workspace is single-threaded inside every
codec.** Encode and decode run to completion on the calling thread; parallelism is the
caller's business — `zarrs` over chunks, `dask` or `ngff-zarr` over arrays, the browser's
worker pool over the WASM module. There is no shared-mutable-slice hand-off anywhere,
which is exactly why there is no raw-pointer aliasing to rescue. The `rayon` entry in
`[workspace.dependencies]` is a leftover: `AGENTS.md` lists it as providing
"code-block-level encode/decode parallelism", and no such code exists.

### `Atomic<T>::from_mut_slice` — not applied

Zero sites converted, because zero sites exist. The API is real and confirmed —
[Capability Probe](./capability-probe.md) (`[[Capability-Probe]]`) records
`pub fn from_mut_slice(v: &mut [u32]) -> &mut [AtomicU32]` verified against the 1.98
sysroot, exercised by the probe — but there is nothing here for it to replace, and
manufacturing a use would add an atomic where a plain `&mut` is correct and faster.

The finding to carry forward is a design note, not a task: **if block-level parallelism is
ever added to the HT coder, `from_mut_slice` is the right tool and it is already
available.** The candidate shape is the per-code-block loop in `ndic-codestream`'s writer,
where each block owns a disjoint coefficient range of one plane. That work needs a
benchmark showing the serial loop is the bottleneck first; this audit does not assert
that it is.

(what-was-removed)=

## What was removed: `split_three`

The one `unsafe` block in the workspace whose correctness rested on a hand-written
aliasing argument rather than on a hardware fact.

```rust
// before
let ptr = plane.as_mut_ptr();
assert!(a_off + w <= len && d_off + w <= len && b_off + w <= len);
// SAFETY: the three row ranges are in-bounds (asserted above) and the
// mutable row is disjoint from both source rows (distinct row offsets,
// debug-asserted to differ by at least one full stride).
unsafe {
    (
        core::slice::from_raw_parts(ptr.add(a_off), w),
        core::slice::from_raw_parts_mut(ptr.add(d_off), w),
        core::slice::from_raw_parts(ptr.add(b_off), w),
    )
}
```

The 5/3 vertical lifting pass needs three rows of one plane at once: two read-only
neighbours and one mutable destination. `a` and `b` are frequently *the same row* — the
mirror cases at the region boundary pass `down = 2*i` when `2*i + 2` is off the end — so
this cannot be a three-way `split_at_mut` on the two source offsets. That is what made
the raw pointer look necessary.

It is not. The split that works is on the **destination**, not on the sources:

```rust
// after
let (before, rest) = plane.split_at_mut(d_off);
let (dst, after) = rest.split_at_mut(w);
let (before, after): (&[i32], &[i32]) = (before, after);
let a = if a_off < d_off { &before[a_off..a_off + w] } else { &after[a_off - d_off - w..a_off - d_off] };
let b = if b_off < d_off { &before[b_off..b_off + w] } else { &after[b_off - d_off - w..b_off - d_off] };
(a, dst, b)
```

Every source row starts at least one full stride from the destination row — the
precondition the old `debug_assert!` already stated — so each lands wholly in `before` or
wholly in `after` and is reachable by ordinary indexing. `a` and `b` may still be the same
row, because two shared borrows of one piece are unremarkable. The disjointness that used
to be a comment is now the thing `split_at_mut` returns.

Note what this is *not*: it is not a 1.98 API. `split_at_mut` has been stable since 1.0.
The block survived because nobody re-derived the split; the audit's value here was
looking again, not having a new tool.

### Bit-exactness

`crates/ndic-htj2k/src/dwt/simd.rs::tests::matches_scalar_bit_exactly` is the gate, and it
is a strong one: 9 plane geometries × 6 level counts, each asserting the SIMD forward
against the scalar oracle, the SIMD inverse back to the original, and the SIMD inverse of
the *scalar* forward. It includes the degenerate shapes (`1×1`, `7×1`, `1×9`) and the odd
ones (`3×5`, `65×33`, `129×77`) where the mirror branches fire. It passes unchanged.

Downstream, the SIMD lane is not optional — `ndic-codestream`'s `writer.rs:151` and
`reader.rs:605` call it directly — so `openjph_differential`, `openjph_interop`, and
`corpus_conformance` all run through this code, and all three are byte-exact suites. They
pass with identical counts.

### Cost

The splitter is called once per lifted row per level, so what changed is per-row borrow
setup — two `split_at_mut` calls and four bounds-checked range indexes in place of six
pointer offsets — measured against per-row kernel work. Interleaved single-process A/B of
five vertical-pass levels, AVX2 kernels on both arms (deliberately: AVX2 is the cheapest
per row, which makes the splitter's share as large as it can be), 11 rounds of 12
iterations, arms alternating order each round:

| Plane | raw pointer (ns) | `split_at_mut` (ns) | Delta | Splitter calls |
| --- | --- | --- | --- | --- |
| 256² | 14 860 | 14 970 | **+0.74 %** | ~496 |
| 512² | 63 770 | 64 390 | **+0.97 %** | ~992 |
| 1024² | 263 388 | 266 247 | **+1.09 %** | ~1984 |
| 2048² | 1 773 034 | 1 789 634 | **+0.94 %** | ~3968 |

**Approximately 1 %, flat across four sizes** — which is the shape a fixed per-row cost
should have, and the consistency is what makes the number believable. An earlier
unpinned run of the same harness read +0.81 / +0.65 / +0.92 / **−3.29 %**; the negative
cell is the tell that the host was saturated by an unrelated build, and it is recorded
here rather than quietly dropped. The table above is the pinned re-run
(`taskset -c 0-3`), and the +1 % is real.

Two things put that 1 % in proportion:

- It is the **vertical pass in isolation**. Five levels of vertical lifting on a 2048²
  plane measure ~1.77 ms here; the full `transform/dwt53_fwd_2048` benchmark measures the
  SIMD lane at **5.76 ms median** on the same pinned cores. The splitter's pass is roughly
  a third of the transform, so ~1 % there is **~0.3 % end to end** — well inside the
  benchmark's own 5.09–7.60 ms spread.
- It is measured against the **cheapest** kernel. AVX2 is deliberate: it does the least
  work per row, so it maximises the splitter's share. The portable and NEON lanes spend
  more per row and would show less.

Harness (scratch, not in the repository):
`.maestro/playbooks/2026-08-21-Rust-198-Adoption/Working/phase05/split_three_ab.rs`.

(what-is-kept)=

## What is kept: the NEON and AVX2 lanes

Two modules, one compiled per target, each now carrying its own justification.

**They cannot be made safe in 1.98.** `vld1q_s32`, `vst1q_s32`, `_mm256_loadu_si256`, and
`_mm256_storeu_si256` take raw pointers; there is no safe form of a vector load. The
alternative that would remove them entirely is `core::simd`, and portable SIMD is still
unstable — it is not among the six APIs the [Capability Probe](./capability-probe.md)
confirmed, and it is not in 1.98 stable at all.

**Deleting them is not on the table either.**
[Phase 04](./algebraic-codec-sweep.md) (`[[Algebraic-Codec-Sweep]]`)
measured the module at **4.53× / 7.51× / 11.81× / 9.64×** the scalar oracle across the same
four plane sizes, and the shipped codec path is hardcoded to it. Phase 04 also measured the
narrower question this audit inherits — how much of that win is the *intrinsics* rather
than the row restructuring — and found the AVX2 lane worth only **1–3 %** over the safe
portable lane, which autovectorizes to 128-bit SSE2 by itself.

That 1–3 % is the whole case for keeping 5 of the 9 remaining `unsafe` occurrences, and it
is thin. It is not acted on here for two reasons: 1–3 % on the encode hot path is a real
regression to accept for a lint number, and the *other* lane is NEON, which is
unmeasurable on this x86-64 host — deleting AVX2 while keeping NEON would leave the module
`unsafe` anyway and buy nothing. **The decision to revisit is on aarch64 hardware**, where
the equivalent portable-vs-NEON measurement can actually be taken. If NEON is also worth
only a few percent, the entire module can become safe portable Rust and the workspace can
go to `forbid`.

### The narrowed override

The file-scoped `#![allow(unsafe_code)]` covering all 507 lines is gone. In its place, two
module-scoped attributes:

```rust
#[cfg(target_arch = "aarch64")]
#[allow(unsafe_code)]
mod neon { … }      // 97 lines

#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
mod avx2 { … }      // 81 lines
```

Because the two are mutually exclusive by `#[cfg]`, **exactly one is ever live**: 81 lines
on x86-64, 97 on aarch64, zero on wasm and everywhere else. The rest of the file — the row
splitter, both vertical passes, the de/interleave, `kernels()`, `lane_name()`, and both
public entry points — is back under the workspace `deny`. A new `unsafe` block in
`forward_53` would now fail the build; before this phase it would have been silently
covered by the file-level allow.

(the-target-feature-note)=

### The `target_feature` note

Six of the nine remaining occurrences are not intrinsics at all — they are `unsafe {}`
blocks around *calls* to `rows` (four in `neon`, and the one inside `macro_rules! kernel`
that expands four times in `avx2`). Since `target_feature_11`, a `#[target_feature]`
function may be declared safe and called without `unsafe` from a caller that carries the
same feature. That would remove the AVX2 call blocks — but only by moving
`#[target_feature(enable = "avx2")]` onto all four public kernel functions, which changes
their type and breaks the `RowKernel = fn(&mut [i32], &[i32], &[i32])` function-pointer
table the lane dispatch is built on.

For `neon::rows` the story is different and simpler: NEON is baseline aarch64, the function
carries no `#[target_feature]` at all, and its `unsafe` is purely the raw-pointer loads. It
could be `fn rows` with the `unsafe {}` block kept inside — turning 5 occurrences into 1 —
and that is a genuine cleanup. It is not taken here because it is unverifiable on this
host: the aarch64 lane cannot be compiled, let alone differentially tested, on x86-64, and
an untested edit to the shipped DWT is exactly the kind of change this phase exists to
discourage. **Carried forward to the same aarch64 session as the NEON keep-or-delete
measurement.**

## The final lint configuration

In the root `Cargo.toml`, inherited by all ten member crates via `[lints] workspace = true`
(verified: every one of the ten manifests carries it):

```toml
[workspace.lints.rust]
unsafe_code = "deny"
unsafe_op_in_unsafe_fn = "deny"
missing_docs = "warn"
```

`unsafe_op_in_unsafe_fn` is warn-by-default in edition 2024; `deny` closes the gap so a
surviving `unsafe fn` cannot treat its whole body as an implicit unsafe block. Both
`rows` functions already used explicit inner `unsafe {}` blocks, so this cost nothing to
turn on — which is the point of turning it on now rather than after something depends on
the laxer rule.

`deny` rather than `forbid` is deliberate: `forbid` cannot be overridden, and the two SIMD
modules need an override. When the NEON measurement lands and the module can go safe,
`forbid` becomes available and should be taken.

The only two overrides in the workspace are the module-scoped `#[allow(unsafe_code)]` on
`mod neon` and `mod avx2`, each with a `///` comment stating why. There is no per-crate
override anywhere, and none is needed for the binding crates.

## Verification

Every gate, on the tightened configuration:

| Gate | Result |
| --- | --- |
| **Negative test** — a temporary `unsafe {}` inserted into `simd.rs::forward_53`, in the same file as the two overrides but outside both | **`error: usage of an unsafe block`**, build fails. The deny is enforced, not merely configured, and the module scoping is what makes the difference. |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Same, run **per crate** so a failure would be attributable | all 10 crates clean: `ndic-core`, `ndic-htj2k`, `ndic-codestream`, `ndic-lift`, `ndic-zfp`, `ndic-zarr`, `ndic-cli`, `ndic-py`, `ndic-bench-core`, `ndic-bench-cli` |
| `cargo clippy -p ndic-zarr -p ndic-core --target wasm32-unknown-unknown -- -D warnings` | clean |
| `cargo clippy -p ndic-zarr --features wasm --target wasm32-unknown-unknown -- -D warnings` | clean — this is the `wasm-bindgen` macro surface |
| `cargo clippy -p ndic-zarr -p ndic-core --target wasm32-wasip2 -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test --workspace --release` | **207 passed, 0 failed, 0 ignored** — identical to the Phase 04 count |
| Byte-exact suites | `corpus_conformance` 1, `openjph_interop` 2, `openjph_differential` 1, `vectors` 1, `checksums` 2 — all pass, all non-zero |
| Python binding (`maturin build --release` → wheel → `pytest`) | 285 passed, 0 skipped; native extension present |
| TypeScript binding (`npm run build:wasm && npm run build && npm test`) | 203 passed; `ndic_zarr_bg.wasm` present (504 KB) |
| Cross-language series equality | 148-case matrix identical across Rust, Python, TypeScript |

**No golden vector moved and no tolerance was edited**, which is the correct outcome: this
phase changed how three row borrows are constructed, not what they contain.

### One pre-existing failure, unchanged and out of scope

`cargo clippy --workspace --all-targets --target wasm32-unknown-unknown` does not build,
and never has. It fails in `getrandom v0.2.17` (reached through `ndic-cli` → `ureq` →
`rustls` → `ring`) and in `wait-timeout` — an HTTP client and a process-timeout helper,
neither of which has meaning on `wasm32-unknown-unknown`. This is
[the same structural failure Phase 04 documented](./algebraic-codec-sweep.md), it
reproduces on a clean tree, and it is why CI scopes its `wasm` job to
`-p ndic-zarr -p ndic-core`. The scoped commands in the table above are the right reading
of "the crates the project ships for wasm", and they are clean.

## Carried forward

1. **Measure NEON against portable on aarch64.** If the gap is the same 1–3 % AVX2 shows,
   delete both intrinsic lanes, keep the row restructuring that produces the actual ~10×,
   and take the workspace to `forbid(unsafe_code)`.
2. **`neon::rows` can drop its `unsafe fn`** — 5 occurrences to 1 — but only with an
   aarch64 differential test to back it.
3. **`rayon` is declared in `[workspace.dependencies]` and used by nothing.** So is `wide`.
   Either wire them up or drop them; `AGENTS.md` currently documents both as if they were
   load-bearing.
4. **`deny(unsafe_code)` says nothing about macro-generated `unsafe`.** If the PyO3 or
   `wasm-bindgen` surface ever needs auditing, it needs `cargo expand`, not this lint.
