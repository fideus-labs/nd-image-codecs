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

(how-to-re-measure)=
## How to re-measure

Every phase from 03 on changes float arithmetic, so every claim about a speed or a golden
value has to compare like with like. One command does that:

```bash
scripts/rust198-remeasure.sh --label phase03-after
```

It runs the full bench suite, diffs it against `bench/baselines/main/`, runs the whole
release test suite, and runs the five exactness suites individually — into one timestamped
folder under `target/rust198-measurements/`, with a `manifest.txt` recording git hash,
toolchain, machine, profile, and feature set. `--bench-only` / `--tests-only` narrow it;
`--out` moves the parent directory. Copy the folder somewhere outside `target/` if it
needs to outlive a `cargo clean` — the phase baseline lives in the playbook's
`Working/baseline-1.98-pre/` for exactly that reason.

Reading the output is the point, so two things it pins deserve saying out loud.

**Timings are only comparable within a machine.** `bench/baselines/main/manifest.json`
records `wsl2-aarch64-12core-dev` on `rustc 1.91.0`. The Phase 02 baseline was captured on
x86-64 bare metal under 1.98 — a different **ISA**, not just a different machine class, so
`simd-*` lanes are comparing NEON against AVX. Against the committed baseline only the
deterministic `bytes_out / bytes_in` ratio means anything; for throughput, compare two runs
of this script from the same box. The ratio result from Phase 02 is the one that matters
and the one later phases must preserve: all 29 ratio-carrying pairs reproduced the
committed baseline to full `f64` bit equality, so the 1.91 → 1.98 compiler change moved no
compressed byte.

**Two of the scripts it invokes would otherwise lie to you**, and it works around both:

- `cargo test --workspace --release` **does not link** in this repo — on 1.98 and equally
  on 1.91, so it is a pre-existing defect and not a migration regression.
  `[profile.release]` sets `panic = "abort"`, so `cargo test` builds the dependency graph
  twice; `ndic-zarr` is `crate-type = ["cdylib", "rlib"]` and its `libndic_zarr.so` carries
  no metadata hash, so the two builds collide on one filename (cargo warns
  `output filename collision`, [cargo#6313](https://github.com/rust-lang/cargo/issues/6313))
  and the `ndic` binary then fails with undefined symbols. The script passes
  `--config profile.release.panic="unwind"`, which collapses the two graphs into one.
  Panic strategy cannot move a golden value. The real fix belongs in the consolidation
  phase.
- `crates/ndic-lift/tests/vectors.rs` and `crates/ndic-zfp/tests/checksums.rs` are both
  `#![cfg(feature = "serde")]`. Run per-crate without `--features serde` and they report
  `ok. 0 passed` — green, and testing nothing. The script passes the feature and prints
  each suite's test count, flagging a zero rather than letting it read as a pass.

Some suites need fetched data or a local build, and skip cleanly (green, having verified
nothing) without it. Run these once before trusting an exactness result:

```bash
scripts/fetch-conformance.sh --with-tools   # corpus + ojph_compress/ojph_expand
scripts/ht-differential.sh 2000             # the block-coder oracle vectors
```

Which suite is worth re-running after a given change — and which are structurally
incapable of noticing a float change at all — is the subject of the
[Float Drift Inventory](./float-drift-inventory.md) (`[[Float-Drift-Inventory]]`). Read it
before concluding anything from a green test run: it documents, among other things, that
the one reachable float loop in the workspace is covered by a test whose fixture is
exactly representable, so a reassociated sum keeps it green.

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

### Phase 02 — benchmark and golden-vector baseline (2026-08-21)

No code changed in this phase, by design: it freezes the measurement baseline so every
later delta is attributable to a specific change rather than to the compiler upgrade.

- Captured the full bench suite and the whole exactness suite on 1.98 at `cc0cd12`, and
  added [`scripts/rust198-remeasure.sh`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/rust198-remeasure.sh)
  to reproduce both in one command — see [How to re-measure](#how-to-re-measure).
- **The 1.91 → 1.98 toolchain change moved no compressed byte.** All 29 ratio-carrying
  record pairs reproduce `bench/baselines/main/` to full `f64` bit equality, and
  `compare main --gate ratio --fail-on-regression` exits 0. Throughput moved a lot
  (median −24 %), but the baseline was captured on aarch64 and this run on x86-64, so
  that number confounds compiler, CPU, and SIMD backend and is not evidence about 1.98.
- Wrote the [Float Drift Inventory](./float-drift-inventory.md), whose headline changes
  the shape of Phases 03–04: **there is no reachable float arithmetic in any codec path.**
  The irreversible 9/7 DWT and `Quant::irrev_delta` have no callers — the writer rejects
  `WaveletKind::Irreversible97` outright — so converting them to `algebraic_*` can neither
  break a golden vector nor show a benchmark win. `dwt/simd.rs`, named as a prime target,
  contains no floats at all: it is the integer 5/3 lane.
- The one reachable float loop is `delta_float!` in `ndic-zarr`'s `numcodecs.delta`
  reimplementation, and it is **off limits**: its `cumsum` exists to match NumPy
  element-order exactly so Rust, Python, and TypeScript readers agree. Its round-trip
  test would not catch a reassociation, because the fixture is exactly representable.
- Two pre-existing traps were found and are worked around by the script rather than
  papered over: `cargo test --workspace --release` does not link (a `panic = "abort"` +
  `cdylib` filename collision, reproducible on 1.91), and two golden-vector suites report
  `ok. 0 passed` when run per-crate without `--features serde`.
