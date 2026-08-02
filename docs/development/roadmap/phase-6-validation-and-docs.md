---
title: Phase 6 — Cross-Ecosystem Validation, Performance & Docs
short_title: Phase 6 — Validation & Docs
description: Phase 6 proves the three families work identically in Rust, Python, and TypeScript, hardens performance at scale, and finishes the usage documentation with example-verified snippets.
---

# Phase 6 — Cross-Ecosystem Validation, Performance & Docs

**Depends on:** Phases 4 + 5 · **Gates:** release · **Architecture:** [](../../architecture/zarr-codec.md)

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
   See [](../decisions/adr-001-documentation-toolchain.md) for what is already
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

- [ ] Full encode/decode matrix green: {zarrs, zarr-python, zarrita.js} ×
      {nd-delta, nd-lift-ht, nd-zfp} × fixture corpus, both directions.
- [ ] `imagecodecs` accepts our ZFP/JPEG 2000/delta output where semantics
      overlap, and we accept its.
- [ ] OME-Zarr 0.5 datasets written with each family validate and open in
      ngff-zarr and ome-zarr-py.
- [ ] CI regression gates on ratio and throughput vs committed baselines.
- [ ] All usage-doc snippets execute in docs CI.
- [ ] Codec specs submitted to zarr-extensions.
