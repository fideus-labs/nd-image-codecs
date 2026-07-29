## Test Data

nd-image-codecs validates against three tiers of data. Nothing large is committed; fixtures are
either tiny (checked in), generated, or fetched-and-cached by scripts under
[`scripts/`](../../scripts/).

### Tier 1 — committed micro-fixtures (`fixtures/`)

Hand-constructed, byte-stable, < 100 KB total:

- `tiny-*.j2c` / `tiny-*.jph` — minimal HT codestreams (single block, known markers)
  used by parser unit tests; each has a `.md` sibling describing every byte region.
- Synthetic raw planes/volumes with closed-form wavelet/lifting answers (impulse,
  ramp, DC) for the 2D DWT and the `nd_lift` kinds.
- **ZFP checksum vectors** — the upstream C test suite's per-configuration
  checksums (dimension × type × mode × rate), extracted into a committed table
  that `ndic-zfp`'s conformance tests reproduce bit-exactly
  ([LLNL/zfp tests](https://github.com/LLNL/zfp)).
- The `codec_series` cross-language fixture matrix (axis layouts × chunk shapes ×
  dtypes × families) with expected pipeline JSON, shared by the Rust, Python,
  and TypeScript builder tests.

### Tier 2 — conformance corpora (fetched, cached)

- **OpenJPH test streams** — [aous72/jp2k_test_codestreams](https://github.com/aous72/jp2k_test_codestreams),
  the corpus OpenJPH's own GoogleTest suite decodes; our decoder must match its
  reference outputs (`scripts/fetch-conformance.sh`).
- **ISO/IEC 15444-4 (conformance) HT streams** where publicly redistributable.
- **Cross-implementation streams** — encoded by OpenJPH CLI, `imagecodecs`, and
  the reference ZFP library (via `zfp-sys`) in CI to test decode interop; our
  encodes are decoded back through those implementations in the same job (see
  the `ci.yml` interop matrix).

### Tier 3 — domain volumes (benchmarks, fetched)

Representative volumetric data for rate/throughput benchmarks and
decorrelation-gain measurements (`scripts/fetch-bench-data.sh`):

- OME-Zarr microscopy volumes from public S3 buckets listed on
  [OME-NGFF example data](https://ngff.openmicroscopy.org/data/) for isotropic
  fluorescence stacks and multi-timepoint (t > 1) series;
- an anisotropic-z volumetric series (e.g. light-sheet or EM stacks) for
  `nd_lift` gain measurement;
- float-valued simulation/scientific fields for the nd-zfp lanes.

Exact URLs, licenses, and SHA-256 pins live in `scripts/bench-data.lock.toml` (created
with Phase 1's first fetch script). Cached under `~/.cache/nd-image-codecs/` — CI restores this
cache rather than re-downloading.

### Round-trip invariants (enforced by proptest)

- `decode(encode(v)) == v` for every integer dtype × 5/3 × any `nd_lift`
  transform set (bit-exact); likewise for nd-zfp reversible mode.
- `decode_lowres(encode(v), r)` equals the reference wavelet pyramid level `r`.
- Byte-range plans: decoding only the planned ranges for a thumbnail yields the same
  pixels as full-file thumbnail decode.
- Scalar and SIMD lanes produce byte-identical codestreams.
- `codec_series` output is byte-identical across Rust, Python, and TypeScript.
