---
title: Rust 1.98 Capability Probe
short_title: Capability Probe
description: What the Rust 1.98 standard library actually offers this project, measured by a runnable probe rather than read from a release announcement — with the exact confirmed signature of every API the migration depends on.
date: 2026-08-21
tags:
  - rust-198
  - toolchain
  - reference
---

# Rust 1.98 Capability Probe

Every phase of the 1.98 migration takes API names from a release announcement rather
than from a compiler, and release announcements are prose. This page is the
correction: it records what
[`bench/rs/ndic-bench-core/examples/rust198_probe.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/rs/ndic-bench-core/examples/rust198_probe.rs)
observed when it was compiled and run against the pinned toolchain. Later phases copy
signatures from the [Confirmed API surface](#confirmed-api-surface) section below
verbatim, instead of guessing at them.


The running record of what the migration then *did* with these APIs is the
[Rust 1.98 Adoption Notes](./adoption-notes.md).

## How to run it

```bash
cargo run -p ndic-bench-core --example rust198_probe
```

The probe lives in `ndic-bench-core` because that crate is `publish = false`, so it
never ships to crates.io. It has no dependency on the crate's library target — it is a
standalone `std`-only binary. It exits non-zero if any probe reports `FAIL`, so it is
safe to wire into a check.

Run it under `--release` as well. One of the findings below is only visible with the
optimizer on, which is the whole reason the probe reports the observed value rather
than a bare pass mark.

## Capture

| | |
| --- | --- |
| Date | 2026-08-21 |
| `rustc` | `rustc 1.98.0 (88d9e12ae 2026-08-18)` |
| `cargo` | `cargo 1.98.0 (797e8a9bc 2026-08-05)` |
| Host | `x86_64-unknown-linux-gnu` |
| Result | 6 PASS, 0 FAIL, 0 UNAVAILABLE of 6 probes |

Nothing the migration plans to use is missing from 1.98 stable. Every signature below
was additionally cross-checked against the `rust-src` copy of the standard library in
`$(rustc --print sysroot)`, so the `#[stable(feature = "…", since = "1.98.0")]` gate
names in the *Feature* column are the real ones and can be grepped for.

## Results

| Feature gate | Availability | Confirmed signature | Probe output |
| --- | --- | --- | --- |
| `float_algebraic` | Stable 1.98.0 | `pub const fn algebraic_add(self, rhs: f32) -> f32` (and `_sub`, `_mul`, `_div`, `_rem`; same shape on `f64`) | `64-term ladder: f32 strict=6.2e1 algebraic=0e0, f64 strict=6.2e1 algebraic=0e0 (reassociated by this build: true); add/sub/mul/div exact f32=true f64=true` |
| `atomic_from_mut` | Stable 1.98.0 | `pub fn from_mut_slice(v: &mut [u32]) -> &mut [AtomicU32]` / `pub fn get_mut_slice(this: &mut [AtomicU32]) -> &mut [u32]` | `[1,2,3,4] --from_mut_slice--> fetch_add(idx) --get_mut_slice--> [1, 3, 5, 7]` |
| `substr_range` | Stable 1.98.0 | `pub fn subslice_range(&self, subslice: &[T]) -> Option<core::range::Range<usize>>` / `pub fn substr_range(&self, substr: &str) -> Option<core::range::Range<usize>>` | `plane[2..6] -> Some(2..6); "codec_series"[6..] -> Some(6..12); foreign slice -> None` |
| `int_format_into` | Stable 1.98.0 | `pub fn format_into(self, buf: &mut NumBuffer<Self>) -> &str` | `u32::MAX -> "4294967295", -1972i32 -> "-1972", size_of::<NumBuffer<u32>>() = 10 bytes (stack, no alloc)` |
| `strip_circumfix` | Stable 1.98.0 | `pub fn strip_circumfix<P: Pattern, S: Pattern>(&self, prefix: P, suffix: S) -> Option<&str>` | `"codec:nd_lift:end" -> Some("nd_lift"); missing suffix -> None; overlapping -> None; &str+char patterns -> Some("brick")` |
| `nonzero_from_str_radix` | Stable 1.98.0 | `pub const fn from_str_radix(src: &str, radix: u32) -> Result<Self, ParseIntError>` | `"ff" base 16 -> Some(255); "0" base 16 rejected -> true; "1011" base 2 -> Some(11)` |

The `float_algebraic` line above is the `--release` run. See
[Algebraic float reassociation is an optimizer decision](#algebraic-reassociation-is-an-optimizer-decision)
for the dev-profile line and why it differs.

(confirmed-api-surface)=

## Confirmed API surface

Copy from here. These are the signatures as they appear in the 1.98.0 standard library
source, not paraphrases.

### Algebraic float arithmetic

```rust
impl f32 {
    pub const fn algebraic_add(self, rhs: f32) -> f32;
    pub const fn algebraic_sub(self, rhs: f32) -> f32;
    pub const fn algebraic_mul(self, rhs: f32) -> f32;
    pub const fn algebraic_div(self, rhs: f32) -> f32;
    pub const fn algebraic_rem(self, rhs: f32) -> f32;
}
// identical shape on f64 (and on the still-unstable f16 / f128)
```

All five are `const fn`, and all carry
`#[must_use = "method returns a new number and does not mutate the original value"]`.
`algebraic_rem` is not in the migration plan but exists, so it is recorded here rather
than rediscovered later.

### Atomic slice views

```rust
impl AtomicU32 {
    pub fn from_mut_slice(v: &mut [u32]) -> &mut [AtomicU32];
    pub fn get_mut_slice(this: &mut [AtomicU32]) -> &mut [u32];
}
```

Both are associated functions, not methods: call them as
`AtomicU32::from_mut_slice(&mut buf)`, never `buf.from_mut_slice()`. The same pair is
stable on every other atomic integer type, and on `AtomicBool` and `AtomicPtr<T>`.

### Subslice offset recovery

```rust
impl<T> [T] {
    pub fn subslice_range(&self, subslice: &[T]) -> Option<core::range::Range<usize>>;
}

impl str {
    pub fn substr_range(&self, substr: &str) -> Option<core::range::Range<usize>>;
}
```

### Allocation-free integer formatting

```rust
pub struct core::fmt::NumBuffer<T: NumBufferTrait>;

impl<T: NumBufferTrait> NumBuffer<T> {
    pub const fn new() -> Self;
}

impl u32 {
    pub fn format_into(self, buf: &mut NumBuffer<Self>) -> &str;
}
// stable on every signed and unsigned integer type
```

### Circumfix stripping

```rust
impl str {
    pub fn strip_circumfix<P: Pattern, S: Pattern>(&self, prefix: P, suffix: S) -> Option<&str>
    where
        for<'a> S::Searcher<'a>: ReverseSearcher<'a>;
}

impl<T> [T] {
    pub fn strip_circumfix<S, P>(&self, prefix: &P, suffix: &S) -> Option<&[T]>
    where
        T: PartialEq,
        S: SlicePattern<Item = T> + ?Sized,
        P: SlicePattern<Item = T> + ?Sized;
}
```

Note the asymmetry between the two: `str` takes its patterns **by value**, the slice
version takes them **by reference**. Note also the type-parameter order in the slice
version — the `where` clauses read `S`, `P` while the arguments are `prefix: &P,
suffix: &S`.

### Non-zero radix parsing

```rust
impl NonZero<u32> {
    pub const fn from_str_radix(src: &str, radix: u32) -> Result<Self, ParseIntError>;
}
```

`const fn`, and stable across all the `NonZero<T>` integer types.

## Findings that are not in the release notes

### `NumBuffer` and the new `Range` are `core`-only

Neither is re-exported through `std`. `use std::fmt::NumBuffer` does not resolve, and
there is no `std::range` module at all — the paths are `core::fmt::NumBuffer` and
`core::range::Range`, in `std` binaries as much as in `no_std` crates. That is worth
knowing before the first `use` line of a migration patch, and it is
[the repository import convention](../style/rust.md)'s first group either way.

`core::range::Range` (stable since 1.95) is a **different type** from
`core::ops::Range`. It is what `subslice_range` and `substr_range` return, it is a
plain `{ start, end }` struct, and it is not an iterator. Importing the wrong `Range`
produces a type error, not a subtle bug, but it costs a compile cycle.

(algebraic-reassociation-is-an-optimizer-decision)=

### Algebraic float reassociation is an optimizer decision

The point of `algebraic_add` is that it *permits* reassociation, not that it performs
it. Both facts are observable:

```text
dev      (opt-level=0)            f32 strict=6.2e1  algebraic=6.2e1   reassociated: false
release  (opt-level=3, lto=thin)  f32 strict=6.2e1  algebraic=0e0     reassociated: true
```

The probe sums `[HUGE, -HUGE, 1.0 × 62]`. Left to right, the two huge terms annihilate
immediately and all 62 ones survive: 62. Under a vectorized accumulator, `HUGE` and
`-HUGE` land in different SIMD lanes, each swallows every 1.0 added into its own lane,
and the two only meet again in the final horizontal reduce — where they cancel and take
the ones with them: 0.

Two details of the probe are load-bearing, and both were found by the first version of
it reporting nothing:

- The ladder is passed through `std::hint::black_box`. As a `const` array, LLVM folds
  both loops at compile time — strictly, left to right — and the two columns read
  identically at every optimization level.
- The loops iterate a **slice of runtime length**, not the array by value. A loop with
  a statically known 64-element bound is fully unrolled rather than vectorized, and the
  unrolled chain is left in source order.

Confirming this at the machine-code level, over `&[f32]` on this host: a `+=` reduction
compiles to nine scalar `addss` instructions, and the `algebraic_add` reduction to four
packed `addps` plus a scalar tail. **The vectorization is real, and so is the accuracy
change.** Any phase that converts a hot loop to algebraic operations needs a numerical
tolerance argument, not just a benchmark — see the
[golden-vector baseline](./adoption-notes.md#what-the-phases-must-not-break).

### `NumBuffer<u32>` is 10 bytes

Ten bytes, stack-resident: the buffer is sized from the maximum digit count of its
integer type. That number is the entire argument for using `format_into` on per-block
metadata paths instead of `to_string()`.

### `strip_circumfix` will not let the affixes overlap

`"foo:bar:baz".strip_circumfix("foo:bar:", ":bar:baz")` is `None`, not `Some("")` and
not a panic. The implementation is literally `self.strip_prefix(prefix)?.strip_suffix(suffix)`,
so the suffix search runs on what is left after the prefix is removed. A partial match
strips nothing at all, which is the behaviour any parser built on it wants.
