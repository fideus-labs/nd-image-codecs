---
title: Test Data
description: 'The three tiers of data nd-image-codecs validates against: committed micro-fixtures, generated volumes, and a fetched-and-cached conformance corpus. Nothing large is committed.'
---

nd-image-codecs validates against three tiers of data. Nothing large is committed; fixtures are
either tiny (checked in), generated, or fetched-and-cached by scripts under
[`scripts/`](https://github.com/fideus-labs/nd-image-codecs/tree/main/scripts).

## Tier 1 — committed micro-fixtures (`fixtures/`)

Hand-constructed, byte-stable, < 100 KB total:

- `tiny-*.j2c` / `tiny-*.jph` — minimal HT codestreams (single block, known markers)
  used by parser unit tests; each has a `.md` sibling describing every byte region.
- Synthetic raw planes/volumes with closed-form wavelet/lifting answers (impulse,
  ramp, DC) for the 2D DWT and the `nd_lift` kinds.
- **ZFP stream pins** (`fixtures/zfp/`) — FNV-1a checksums of `nd_zfp`
  streams across the dims × dtype × mode matrix plus a pinned chunk, all
  reproduced bit-exactly by `ndic-zfp`'s tests (and re-encoded byte-for-byte
  from Python). Upstream C-suite checksum parity is carried by the
  [`zfp-rs`](https://crates.io/crates/zfp-rs) core's own CI; the in-repo
  differential against the C library runs through `imagecodecs`
  ([LLNL/zfp tests](https://github.com/LLNL/zfp)).
- The `codec_series` cross-language fixture matrix (axis layouts × chunk shapes ×
  dtypes × families) with expected pipeline JSON, shared by the Rust, Python,
  and TypeScript builder tests.

## Tier 2 — conformance corpora (fetched, cached)

- **OpenJPH test streams** — [aous72/jp2k_test_codestreams](https://github.com/aous72/jp2k_test_codestreams),
  the corpus OpenJPH's own GoogleTest suite decodes; our decoder must match its
  reference outputs (`scripts/fetch-conformance.sh`).
- **ISO/IEC 15444-4 (conformance) HT streams** where publicly redistributable.
- **Cross-implementation streams** — encoded by OpenJPH CLI, `imagecodecs`, and
  the reference ZFP library (via `zfp-sys`) in CI to test decode interop; our
  encodes are decoded back through those implementations in the same job (see
  the `ci.yml` interop matrix).

## Tier 3 — domain volumes (benchmarks, fetched)

Representative volumetric data for rate/throughput benchmarks and
decorrelation-gain measurements, fetched by
[`scripts/fetch-bench-data.sh`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/fetch-bench-data.sh)
and pinned in
[`scripts/bench-data.lock.toml`](https://github.com/fideus-labs/nd-image-codecs/blob/main/scripts/bench-data.lock.toml)
by URL, license, and the SHA-256 of the volume's *decoded* bytes — so an
upstream re-chunk or recompression is caught as the content change it is.
Two volumes are pinned today, both OME-Zarr levels from the
[Image Data Resource](https://idr.openmicroscopy.org/) under CC BY 4.0:

- an **anisotropic-z** serial-section EM stack (~0.50 µm in z against ~0.36 µm
  in x/y), which is the case `nd_lift`'s cross-axis gain is measured on;
- a **multi-timepoint** fluorescence series (40 timepoints × 3 channels × 31
  z), which exercises the builder's t-grouping path — no single-timepoint
  fixture reaches it.

A float-valued simulation field for the nd-zfp lanes is not pinned yet: the
public OME-NGFF corpora are integer microscopy, so float coverage still comes
from the deterministic generator described below. Finding one with a stable
URL and a clear license is a data-sourcing task rather than a code change.

Volumes cache under `~/.cache/nd-image-codecs/bench-data/` (override with
`NDIC_BENCH_DATA_DIR`); the nightly workflow restores that cache rather than
re-downloading. Nothing *requires* them: the macro lanes
([`bench/py/run_macro.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/py/run_macro.py))
skip cleanly when the cache is empty, and the micro/meso lanes run on the
deterministic synthetic microscopy generator at
[`bench/py/synthetic.py`](https://github.com/fideus-labs/nd-image-codecs/blob/main/bench/py/synthetic.py)
(seeded Gaussian blobs + Poisson noise), so records are reproducible without
any download.

## Round-trip invariants (enforced by proptest)

- `decode(encode(v)) == v` for every integer dtype × 5/3 × any `nd_lift`
  transform set (bit-exact); likewise for nd-zfp reversible mode.
- `decode_lowres(encode(v), r)` equals the reference wavelet pyramid level `r`.
- Byte-range plans: decoding only the planned ranges for a thumbnail yields the same
  pixels as full-file thumbnail decode.
- Scalar and SIMD lanes produce byte-identical codestreams.
- `codec_series` output is byte-identical across Rust, Python, and TypeScript.
