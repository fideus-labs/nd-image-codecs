---
title: Phase 6 — Cross-Ecosystem Validation, Performance & Docs
short_title: Phase 6 — Validation & Docs
description: Phase 6 proves the three families work identically in Rust, Python, and TypeScript, hardens performance at scale, and finishes the usage documentation with example-verified snippets.
---

# Phase 6 — Cross-Ecosystem Validation, Performance & Docs

**Depends on:** Phases 4 + 5 · **Gates:** release · **Architecture:** [Zarr Codecs](../../architecture/zarr-codec.md)

Phase 6 proves the three families work *identically* everywhere users will run
them, hardens performance at scale, and finishes the usage documentation with
example-verified snippets.

## What to build

1. **The validation matrix** — for each family (nd-delta, nd-lift-ht, nd-zfp) ×
   fixture (OME-Zarr corpus, dtypes, axis layouts):
   - encode with **zarrs** (Rust) → decode with **zarr-python** (our entry
     points) and with **zarrita.js/numcodecs.js** (our WASM classes);
   - and the reverse direction;
   - **third-party validation**: where codecs overlap, decode our output with
     `imagecodecs` (ZFP, JPEG 2000, delta) via `zarr-python` instead of our own
     implementations — and decode `imagecodecs`-encoded data with ours.
2. **OME-Zarr integration**: write OME-Zarr `0.5` multiscales with each family;
   validate with `ome-zarr-py` and [ngff-zarr](https://github.com/fideus-labs/ngff-zarr);
   confirm napari/viv-based viewers open nd-delta data unmodified.
3. **Performance at scale**: the `bench/` suite on 100 GB-class volumes;
   profiling (flamegraph, `perf`), allocation audits, thumbnail
   bytes-fetched-per-pixel; regression gates in CI on the committed baselines.
4. **Usage docs completion**: every `docs/usage/*.md` snippet executed by a docs
   CI job — the illustrative Rust snippets in `docs/usage/rust.md` graduate to
   compiled, tested examples as the APIs land.
   The documentation site itself already exists — mystmd under `docs/`, a strict
   build gating every pull request, and a Read the Docs deploy — so this is a
   matter of executing the snippets on top of that pipeline, not building one.
   See [ADR 001](../decisions/adr-001-documentation-toolchain.md) for what is already
   wired up and where executable code blocks were deliberately left out.
5. **Standardization**: submit `nd_lift`, `htj2k`, and `nd_zfp` codec specs to
   [zarr-extensions](https://github.com/zarr-developers/zarr-extensions);
   coordinate naming with the OME-NGFF community.

## Order of work

Validation matrix → OME-Zarr integration → performance hardening → docs CI →
extension registration.

## Reference anchors

- zarr-python entry points: <https://zarr.readthedocs.io/en/stable/user-guide/extending.html>
- imagecodecs codec inventory: <https://pypi.org/project/imagecodecs/>
- zarrita.js: <https://zarrita.dev> · numcodecs.js: <https://github.com/manzt/numcodecs.js>
- OME-NGFF 0.5: <https://ngff.openmicroscopy.org/latest/>

## Acceptance criteria

- [x] Full encode/decode matrix green: {zarrs, zarr-python, zarrita.js} ×
      {nd-delta, nd-lift-ht, nd-zfp} × fixture corpus, both directions.
- [x] `imagecodecs` accepts our ZFP/JPEG 2000/delta output where semantics
      overlap, and we accept its.
- [x] OME-Zarr 0.5 datasets written with each family validate and open in
      ngff-zarr and ome-zarr-py.
- [ ] CI regression gates on ratio and throughput vs committed baselines.
- [x] All usage-doc snippets execute in docs CI.
- [ ] Codec specs submitted to zarr-extensions.

### What the two open criteria still need

**Throughput gating.** The ratio gate runs on every pull request
(`bench-pr-gate`) and the nightly grid additionally opens an issue on a ratio
regression. Throughput is measured and reported but never fails a build,
because the committed baselines were captured on a developer machine and the
time gate's σ envelope is only meaningful against a baseline from the same
machine class. `bench-baseline-refresh` now records on a GitHub runner and
opens a pull request, so closing this is: run it, adopt the result, then flip
the PR gate to `--gate both`. Adopting it moves the baseline's machine class,
which is why the workflow makes you name the machine — a decision, not a
default.

**Extension registration.** Specifications for all three codecs are staged in
[`spec/codecs/`](https://github.com/fideus-labs/nd-image-codecs/tree/main/spec/codecs)
in the layout zarr-extensions expects, with schemas checked against every
configuration the builder emits. Two things block the pull request itself, and
both need a person:

- zarr-extensions already registers a `zfp` codec whose stored bytes are
  identical to `nd_zfp`'s for the same data and mode — only the name and the
  handling of chunks above four dimensions differ. Registering a second name
  for a byte-identical format is probably the wrong outcome; adopting `zfp` is
  a breaking format change. See
  [`spec/README.md`](https://github.com/fideus-labs/nd-image-codecs/blob/main/spec/README.md).
- Extension documents must be licensed CC BY 3.0, which the copyright holder
  has to accept.

### What Phase 6 also changed

Three gaps had to close before the matrix could exist at all, and they are
worth knowing about because they were not on the original list:

- `nd_lift` had no WASM path, so zarrita.js could not decode nd-lift-ht at
  all (a Phase 4 gap). It now shares the Rust core with every other ecosystem.
- `zarrs` ships no delta codec, so the nd-delta family could not run in Rust.
  `numcodecs.delta` is now implemented and registered.
- zarrita.js could not *write* any pipeline containing `transpose`: its
  built-in codec constructs the output buffer from the chunk object rather
  than the chunk's array, so every such write produced garbage. Its delta
  codec also refuses the strides a permutation produces, and its blosc loader
  hands Zarr v3's string `shuffle` to a numcodecs.js codec expecting the v2
  numeric one. The TypeScript package ships corrected replacements and
  registers them alongside our own codecs.
