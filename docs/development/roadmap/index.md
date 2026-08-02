---
title: Implementation Roadmap
short_title: Roadmap
description: nd-image-codecs is built in six strictly ordered phases; each phase document says what to build, against which spec clauses, with which tests and benchmarks, and what done means.
---

# Implementation Roadmap

:::{note} Document status
**Version:** 0.1 · **Status:** Draft — phases are strictly ordered; each phase's
acceptance criteria gate the next.
:::

nd-image-codecs is built in six phases. Each phase document gives concrete implementation
guidance: what to build, in what order, against which spec clauses and reference
implementations, with which tests and benchmarks, and what "done" means.

| Phase | Deliverable | Document |
| --- | --- | --- |
| **1** | Baselines & the codec-series builder: the **nd-delta** family from existing Zarr codecs, the tri-language `codec_series` builder, and a live benchmark harness | [Phase 1 — Baselines & Series](./phase-1-baselines-and-series.md) |
| **2** | The **`nd_lift`** array-to-array codec: `delta`/`haar`/`5/3` lifting, boundary/overflow handling, validated behind a Blosc-Zstd backend | [Phase 2 — nd_lift](./phase-2-nd-lift.md) |
| **3** | HTJ2K core in Rust: FBCOT block coder, 2D DWT, Part 1/15 codestream + `TLM`/`PLT` index, SIMD lanes, `ndic` CLI | [Phase 3 — HTJ2K Core](./phase-3-htj2k-core.md) |
| **4** | The **nd-lift-ht** integration: `htj2k` plane codec over `nd_lift` output, coefficient-plane index, 2D/3D thumbnails & range access | [Phase 4 — nd-lift-ht](./phase-4-nd-lift-ht.md) |
| **5** | The **`nd_zfp`** codec: clean-room Rust ZFP port for 2D/3D/4D reproducing upstream checksums, brick index, fixed-rate GPU bricks | [Phase 5 — nd_zfp](./phase-5-nd-zfp.md) |
| **6** | Cross-ecosystem validation & docs: `zarrs` + `zarr-python` + `imagecodecs` round-trip matrix, performance at scale, standardization, complete usage docs | [Phase 6 — Validation & Docs](./phase-6-validation-and-docs.md) |

## Conventions for phase work

- Read the linked [architecture docs](../../architecture/index.md) before the phase doc;
  the phase doc tells you *what and when*, the architecture doc *how and why*.
- Every phase lands incrementally through PRs that keep `cargo test --workspace` and
  clippy green; scaffolded `Unsupported` stubs are replaced, never bypassed.
- Any change to the `codec_series` builder must be mirrored across the Rust, Python, and
  TypeScript implementations in the same PR; the cross-language equality test must stay
  green.
- No JPEG 2000 Part 2 (MCT) syntax is introduced in any phase.
- Performance-sensitive work registers benchmarks as it lands (see
  [benchmarking](../benchmarking.md)).
- Acceptance criteria are checked off in the phase doc itself via PR edits, so the
  roadmap is always the source of truth for project status.
