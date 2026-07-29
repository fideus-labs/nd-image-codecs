## In-Plane Wavelet Transform

> Crate: [`ndic-htj2k`](../../crates/ndic-htj2k/) (2D DWT) · Roadmap:
> [Phase 3](../development/roadmap/phase-3-htj2k-core.md)
>
> The **cross-axis** (z/t/c) transform is a separate, explicit codec — see
> [nd-transform.md](./nd-transform.md). This page covers only the in-plane 2D
> wavelet inside the `htj2k` plane codec.

Two kernels from JPEG 2000 Part 1 ([ITU-T T.800](https://www.itu.int/rec/T-REC-T.800)),
applied to each trailing 2D `(y, x)` plane before HT block coding:

| Kernel | `WaveletKind` | Nature | Use |
| --- | --- | --- | --- |
| Le Gall **5/3** | `Reversible53` | Integer lifting, bit-exact | Lossless (default) |
| CDF **9/7** | `Irreversible97` | Real-valued lifting + quantization | Lossy, higher ratio |

### Lifting implementation

Both kernels are implemented as lifting steps over interleaved even/odd sample
lanes:

- **5/3**: two integer lifting steps with the T.800 rounding conventions;
  exactly invertible for all supported integer dtypes.
- **9/7**: four lifting steps plus a scaling step, with a floating-point path and
  a fixed-point integer approximation for SIMD lanes; the fixed-point constants
  and their error budget are pinned by tests against the reference float path.

Boundary handling uses **symmetric (mirror) extension** per T.800 Annex F. (The
`nd_lift` cross-axis codec uses the same boundary rule along z/t; see
[nd-transform.md](./nd-transform.md).)

### Geometry

**`dwt_2d`** — the in-plane transform: `xy_levels` dyadic decompositions
producing the LL/HL/LH/HH subband tree; each resulting subband plane feeds the
HT block coder. Row transforms are SIMD-vectorized along `x`; column transforms
process multiple columns per register, mirroring OpenJPH's per-ISA transform
files ([OpenJPH `transform/`](https://github.com/aous72/OpenJPH/tree/master/src/core/transform)).

The plane codec is intentionally **2D only**. All depth/time/channel
decorrelation happens upstream in `nd_lift`, keeping the `htj2k` codestream pure
Part 1 / Part 15 with no Part 2 machinery.

### Precision budget

Coefficients live in `i32` planes (`CoeffPlane`). For 16-bit input and 5 xy
levels, 5/3 growth stays comfortably within `i32`; the encoder computes the
exact per-subband bit budget and writes it via quantization markers. The 9/7
fixed-point path documents its Q-format per lifting step; overflow behavior is
checked by proptest with extreme-value inputs.

### Testing

- Analytic vectors: known signals (impulse, ramp, DC) against closed-form
  subband values.
- Round-trip identity on 5/3 for random planes (proptest).
- 9/7 float-vs-fixed-point divergence bounded and asserted.
- Cross-check against OpenJPH output on shared inputs (differential lane in the
  bench suite, see [benchmarking](../development/benchmarking.md)).
