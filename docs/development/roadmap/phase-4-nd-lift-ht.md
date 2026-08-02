---
title: Phase 4 — The nd-lift-ht Family
short_title: Phase 4 — nd-lift-ht
description: Phase 4 fuses the nd_lift transform and the HTJ2K core into the flagship transpose → nd_lift → htj2k family, with the byte-range index that makes thumbnails work.
---

# Phase 4 — The nd-lift-ht Family

**Depends on:** Phases 2 + 3 · **Gates:** Phase 6 · **Architecture:** [](../../architecture/zarr-codec.md),
[](../../architecture/range-access.md),
[](../../architecture/nd-transform.md)

Phase 4 fuses the two preceding phases into the flagship family:
`transpose → nd_lift → htj2k`. The `htj2k` **array-to-bytes** codec compresses
each trailing 2D plane of the (decorrelated) chunk as an independent Part 1/15
codestream, writes the coefficient-plane index, and exposes thumbnail/range
plans.

## What to build

1. **The `htj2k` Zarr codec** (`ndic-zarr`): chunk → trailing 2D planes → one
   RPCL `.jph` codestream each; honor `xy_levels`, `reversible`, `progression`,
   `index` from the codec config; assemble the chunk bytes as
   `[header | plane index | codestreams…]`.
2. **Coefficient-plane index**: byte range (and low-resolution prefix lengths)
   per plane; consumed by `RangeIndex`.
3. **`RangeIndex` plans** (`ndic-codestream`): `thumbnail`, `thumbnail_3d`,
   `plane`, `region` → coalesced byte-range lists (1–3 ranges typical).
4. **Thumbnail decode**: execute a plan (local file or HTTP Range) → 2D
   thumbnail or z-downsampled 3D preview; `ndic thumbnail` / `ndic index` CLI.
5. **nd_lift low-pass synergy**: 3D thumbnail plans select each group's low-pass
   plane(s) at low resolution — x, y, *and* z downsampling from a handful of
   ranges.
6. **zarrs + Python + TS registration** of `htj2k` (entry point + numcodecs.js
   class already scaffolded; wire them to the native/WASM core).
7. **Precinct guidance**: measured trade-offs for `region` plans on very large
   planes.

## Order of work

1. `htj2k` codec over undecorrelated chunks (nd_lift identity) — round-trip.
2. Coefficient-plane index + `RangeIndex::plane/thumbnail`.
3. Compose with `nd_lift`; `thumbnail_3d` plans.
4. CLI subcommands; HTTP-Range execution path.
5. Cross-ecosystem registration.

## Spec / reference anchors

- RPCL progression & packet indexing: ITU-T T.800 Annex B/A (`TLM`/`PLT`).
- HTJ2K streaming rationale: <https://ds.jpeg.org/whitepapers/jpeg-htj2k-whitepaper.pdf>
- Zarr partial decoders: <https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html>

## Tests & benchmarks

- Round-trip: nd-lift-ht series over the OME-Zarr fixture corpus, all dtypes.
- Thumbnail-vs-full-decode consistency (prefix decode equals downsampled full).
- Byte-range plan economy: assert ≤3 ranges for standard thumbnail plans.
- Bench lanes: `simd-53-ht` (2D), `simd-53-lift-z2` (z-decorrelated) vs
  nd-delta and blosc baselines — ratio, encode/decode throughput, and
  bytes-fetched-per-thumbnail.

## Acceptance criteria

- [ ] nd-lift-ht chunks round-trip losslessly across zarrs and zarr-python.
- [ ] A 1/32-scale thumbnail of a 100 GB-class volume decodes from ≤3 HTTP Range
      requests without a smart server.
- [ ] 3D previews decode from low-pass planes' low-resolution prefixes only.
- [ ] `ndic index` emits plans executable by plain `curl -r`.
- [ ] Measured compression gain of z-decorrelation over nd-delta on correlated
      volumes is recorded in the bench baselines.
