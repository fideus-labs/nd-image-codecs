---
title: Rust 1.98 Adoption Notes
short_title: Adoption Notes
description: The running record of the Rust 1.98 migration — why the MSRV moved to 1.98 in one step, what that costs downstream consumers on crates.io, PyPI, and npm, and what each phase changed.
date: 2026-08-21
tags:
  - rust-198
  - migration
  - note
---

# Rust 1.98 Adoption Notes

This page is the running record of the Rust 1.98 migration: the decisions behind it,
what each phase changed, and what a reader in six months needs in order to understand
why some hot loop is written the way it is. It is appended to as the migration
proceeds; nothing here is provisional once written.

What 1.98 actually offers — measured on a real compiler, with exact signatures — is a
separate page: the [Rust 1.98 Capability Probe](./capability-probe.md). Read that one
before writing code against any of these APIs.

## Why the MSRV moved to 1.98

The workspace went from `rust-version = "1.91"` to `rust-version = "1.98"` in a single
step, on 2026-08-21, one day after 1.98.0 shipped. Three things make that the right
call here rather than a reckless one.

**The adoption is aggressive on purpose.** 1.98's headline additions are not
conveniences, they are the things this project has been working around by hand.
`algebraic_add` and friends give the wavelet and quantization loops permission to
vectorize without the whole-function contagion of `-ffast-math`; `AtomicU32::from_mut_slice`
removes hand-rolled pointer casts from the parallel-fill paths in a codebase where
`unsafe_code` is a warn-level lint. Adopting these late would mean carrying the
workarounds *and* the eventual migration.

**The floor is set here, not by a dependency.** Until now the MSRV tracked whatever
`zarrs` required, and the number in `Cargo.toml` was a report rather than a decision.
It is now a decision: this workspace uses 1.98 standard-library APIs directly, and the
number says so. The [Rust style page](../style/rust.md) was corrected to match.

**Downstream consumers see the new floor, and there is no way to soften that.** The
crates publish to crates.io, the Python binding to PyPI, and the TypeScript binding to
npm. For crates.io that is the direct, familiar cost: anyone depending on `ndic-core`
et al. on a pre-1.98 toolchain gets a resolver error, and `cargo add` will refuse the
new version rather than silently pick it. The PyPI and npm artifacts are compiled
wheels and prebuilt WASM, so end users of *those* are unaffected — but anyone building
them from source needs 1.98 too. This is a consumer-visible change and is recorded in
`CHANGELOG.md` as one.

The counterweight is that 1.98 is a stable release, `rust-toolchain.toml` pins it, and
CI's `dtolnay/rust-toolchain@stable` jobs honour that pin — so contributors get the
right compiler automatically and nobody has to know the number.

## What the phases must not break

The migration touches numerics. Two invariants bound everything that follows, and any
phase that cannot hold both stops rather than proceeding:

- **No codec output may change.** Encoded bytes are a wire format with published
  conformance expectations. A benchmark win that moves a single byte is not a win.
- **Algebraic float substitution changes accuracy, provably.** The capability probe
  demonstrates a reassociated sum landing on 0 where the strict sum lands on 62 — a
  contrived case, but not a hypothetical one. Any loop converted to algebraic
  operations needs a numerical-tolerance argument backed by golden vectors, not just a
  faster benchmark. See
  [Algebraic float reassociation is an optimizer decision](./capability-probe.md#algebraic-reassociation-is-an-optimizer-decision).

That is why the phase that captures a benchmark and golden-vector baseline runs
*before* any hot code is touched.

## Phase log

### Phase 01 — toolchain bump and capability probe (2026-08-21)

- `rust-toolchain.toml` pinned to `1.98.0`; `[workspace.package] rust-version` set to
  `1.98`. All ten member crates inherit through `rust-version.workspace = true` — there
  are no per-crate overrides to chase.
- The `README.md` prerequisites line and the [Rust style page](../style/rust.md)
  toolchain line were updated to 1.98. `bench/baselines/main/manifest.json` was
  deliberately left recording `rustc 1.91.0`: it is the toolchain a past baseline was
  captured with, and rewriting it would falsify the record.
- Added the capability probe at
  [`bench/rs/ndic-bench-core/examples/rust198_probe.rs`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/rs/ndic-bench-core/examples/rust198_probe.rs)
  and wrote up its findings in the
  [Rust 1.98 Capability Probe](./capability-probe.md). All six probed feature groups
  are stable on 1.98.0; none had to be marked `UNAVAILABLE`.
- Two findings changed how later phases should be written: `NumBuffer` and the new
  `Range` are reachable only through `core::`, not `std::`; and algebraic-float
  reassociation is an optimizer decision that shows up at `opt-level=3` and not at
  `opt-level=0`, so any before/after comparison must state its profile.
