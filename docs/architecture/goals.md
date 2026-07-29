## Design Goals and Non-Goals

### Goals

- **Composability first.** Every capability is a Zarr v3 codec that composes
  with the existing Zarr codec ecosystem (`transpose`, `numcodecs.delta`,
  `blosc`, `crc32c`, sharding). nd-image-codecs adds codecs and a builder — it
  does not define a new container format
  ([Zarr v3 core spec](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html)).
- **Explicit cross-axis decorrelation, no Part 2.** All z / time / channel
  decorrelation flows through the `nd_lift` array-to-array codec with a
  published specification — never JPEG 2000 Part 2 MCT syntax. This is a
  deliberate IP choice: it captures spatial correlation while staying clear of
  Part 2 patents and tooling gaps (see [nd-transform.md](./nd-transform.md)).
- **Standards-only plane coding.** The `htj2k` codec emits only conforming
  JPEG 2000 Part 1 (T.800) + Part 15 (T.814) codestreams; any HTJ2K decoder can
  read a plane.
- **Faithful ZFP.** The `nd_zfp` codec is a clean-room Rust port of
  [LLNL ZFP](https://github.com/LLNL/zfp) for 2D/3D/4D blocks that reproduces
  upstream's bitstreams and test vectors, verified against the C library via an
  FFI reference lane (see [zfp.md](./zfp.md)).
- **Thumbnails from dumb storage.** With nd-lift-ht, 2D and low-resolution 3D
  thumbnails must be fetchable with plain HTTP Range requests against
  S3/GCS/any static server — no JPIP, no server-side smarts — using RPCL
  progression plus the coefficient-plane index (see
  [range-access.md](./range-access.md)).
- **Bit-exact losslessness.** The nd-delta family, the `nd_lift` 5/3 and haar
  paths, and ZFP reversible mode round-trip every supported dtype exactly;
  property tests enforce encode∘decode = identity.
- **One builder, three ecosystems, identical output.** The `codec_series`
  builder is implemented in Rust (`zarrs`), pure Python (`zarr-python`), and
  pure TypeScript (`numcodecs.js` / zarrita.js); CI asserts byte-identical
  pipelines so metadata authored anywhere is portable.
- **Cross-ecosystem validation as a contract.** Pipelines are validated against
  `imagecodecs` (ZFP, JPEG 2000, delta) through `zarr-python` — our encoder's
  output decodes with third-party codecs and vice-versa (see roadmap Phase 6).
- **Benchmarks as a contract.** Every performance-sensitive change runs against
  committed baselines with a ≥10 % + noise-envelope regression gate in CI,
  including comparison lanes against `imagecodecs` and reference ZFP.
- **Agent-discoverable docs.** Hierarchical documentation (index → topic →
  phase) written so both humans and coding agents can locate exactly the
  context they need.

### Non-Goals

- **No JPEG 2000 Part 2 (MCT).** This is the central IP decision. We never emit
  or parse `MCT`/`MCC`/`MCO`/`CBD` markers; cross-axis decorrelation is the
  explicit `nd_lift` codec instead.
- **No JP3D (Part 10).** The market abandoned it (OpenJPEG removed it in
  [2.5.0](https://www.openjpeg.org/2022/05/13/OpenJPEG-2.5.0-released); Kakadu
  never shipped it).
- **No C++ source port.** nd-image-codecs is a clean-room implementation guided
  by public specs (T.814, T.800, the ZFP papers) and the published architecture
  of OpenJPH and LLNL ZFP — it ports conformance behavior and performance ideas,
  not code.
- **No classic EBCOT Tier-1 encoder.** The `htj2k` codec decodes HT codestreams
  and encodes HT; producing legacy J2K-1 (MQ-coded) codestreams is out of scope.
- **No JPIP.** Interactive-protocol support is explicitly replaced by the static
  byte-range index.
- **No new container format.** Series are ordinary Zarr v3 codec chains stored
  in ordinary Zarr metadata; OME-Zarr multiscales/sharding are consumed as-is,
  not reinvented ([OME-NGFF](https://ngff.openmicroscopy.org/latest/)).
- **Not an image-processing library.** nd-image-codecs codes arrays; scaling,
  color management, and visualization belong to downstream consumers.
