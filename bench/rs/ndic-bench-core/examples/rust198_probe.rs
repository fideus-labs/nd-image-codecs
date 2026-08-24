//! Rust 1.98 capability probe.
//!
//! Every later phase of the 1.98 adoption work takes API names from a release
//! announcement rather than from a compiler, so this example is the ground
//! truth: it compiles and runs each candidate standard-library API on *this*
//! machine and prints one `PASS` / `FAIL` / `UNAVAILABLE` line per feature,
//! with the observed value. Run it with:
//!
//! ```bash
//! cargo run -p ndic-bench-core --example rust198_probe
//! ```
//!
//! The findings are transcribed into
//! `docs/development/rust-198/capability-probe.md`; later phases copy confirmed
//! signatures from there instead of guessing.
//!
//! It lives in `ndic-bench-core` because that crate is `publish = false`, so
//! the probe never ships to crates.io. It has no dependency on the crate's
//! library target — it is a standalone `std`-only binary.

// `NumBuffer` and the new `Range` are `core`-only in 1.98 — neither is
// re-exported through `std::fmt` / `std::range`, so these paths must say
// `core::` even in a `std` binary.
use core::fmt::NumBuffer;
use core::range::Range;
use std::hint::black_box;
use std::num::NonZero;
use std::sync::atomic::{AtomicU32, Ordering};

/// Outcome of a single capability probe.
///
/// `Unavailable` is distinct from `Fail`: it means the API does not exist on
/// this toolchain at all (its probe body is commented out with an
/// `// UNAVAILABLE:` note), whereas `Fail` means the API compiled but did not
/// behave as the adoption plan assumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    Unavailable,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Unavailable => "UNAVAILABLE",
        }
    }

    /// `Pass` when `ok`, otherwise `Fail`.
    fn of(ok: bool) -> Self {
        if ok { Outcome::Pass } else { Outcome::Fail }
    }
}

/// Prints one result line and hands the outcome back for tallying.
///
/// `feature` is the `#[stable(feature = "…")]` gate name from the standard
/// library source, so a reader can grep the sysroot for the exact item.
fn report(feature: &str, outcome: Outcome, detail: &str) -> Outcome {
    println!(
        "{label:<11}  {feature:<24}  {detail}",
        label = outcome.label()
    );
    outcome
}

/// `f32`/`f64` `algebraic_add` / `algebraic_sub` / `algebraic_mul` /
/// `algebraic_div` — arithmetic that permits the optimizer to reassociate and
/// contract, without the whole-function contagion of `-ffast-math`.
///
/// This is the feature Phases 03 and 04 are built on, so the probe prints a
/// deliberately reassociation-sensitive sum computed both ways.
fn probe_float_algebraic() -> Outcome {
    // Catastrophic-cancellation ladder: `[HUGE, -HUGE, 1.0, 1.0, …]`. Summed
    // strictly left to right the two huge terms annihilate immediately and the
    // 62 ones all survive, so the total is 62. Summed by a *vectorized*
    // accumulator — which is exactly what `algebraic_add` licenses — `HUGE`
    // and `-HUGE` land in different lanes, each swallows the ones added into
    // its own lane, and they only meet again in the final horizontal reduce,
    // where they cancel and take every 1.0 with them: the total is 0. The gap
    // between the two columns is the freedom Phase 03 is buying in the lifting
    // loops, and it is also the accuracy that freedom costs.
    //
    // Two details are load-bearing:
    //   * `black_box` on the slice. Left as a `const` array, LLVM folds both
    //     loops at compile time — strictly, left to right — and the columns
    //     come out identical at every opt-level, so the probe would report
    //     nothing.
    //   * Iterating a *slice* of runtime length rather than the array by
    //     value. A statically-known 64-element loop is fully unrolled instead
    //     of vectorized, and the unrolled chain is left in source order.
    const LEN: usize = 64;

    let mut ladder_32 = [1.0_f32; LEN];
    ladder_32[0] = 1.0e30;
    ladder_32[1] = -1.0e30;
    let ladder_32: &[f32] = black_box(&ladder_32[..]);

    let mut ladder_64 = [1.0_f64; LEN];
    ladder_64[0] = 1.0e300;
    ladder_64[1] = -1.0e300;
    let ladder_64: &[f64] = black_box(&ladder_64[..]);

    let mut strict_32 = 0.0_f32;
    for &v in ladder_32 {
        strict_32 += v;
    }
    let mut algebraic_32 = 0.0_f32;
    for &v in ladder_32 {
        algebraic_32 = algebraic_32.algebraic_add(v);
    }

    let mut strict_64 = 0.0_f64;
    for &v in ladder_64 {
        strict_64 += v;
    }
    let mut algebraic_64 = 0.0_f64;
    for &v in ladder_64 {
        algebraic_64 = algebraic_64.algebraic_add(v);
    }

    // Whether the two columns actually differ depends on the opt-level: at
    // `-C opt-level=0` nothing is reassociated, so both walk the ladder in
    // order — the two large terms cancel to zero and the 62 trailing `1.0`s
    // leave both columns reading 62. Reported, not asserted — the pass
    // criterion below is the exact-arithmetic check.
    let reassociated = strict_32.to_bits() != algebraic_32.to_bits()
        || strict_64.to_bits() != algebraic_64.to_bits();

    // Exact-in-binary operands, so every one of these is bit-determined
    // whatever reassociation the backend chooses. Compared through `to_bits`
    // rather than `==` because these are floats and the intent is an exact
    // bit check, not an approximate one.
    let ops_32 = 5.0_f32.algebraic_sub(3.0).to_bits() == 2.0_f32.to_bits()
        && 3.0_f32.algebraic_mul(2.0).to_bits() == 6.0_f32.to_bits()
        && 6.0_f32.algebraic_div(3.0).to_bits() == 2.0_f32.to_bits()
        && 1.5_f32.algebraic_add(2.5).to_bits() == 4.0_f32.to_bits();
    let ops_64 = 5.0_f64.algebraic_sub(3.0).to_bits() == 2.0_f64.to_bits()
        && 3.0_f64.algebraic_mul(2.0).to_bits() == 6.0_f64.to_bits()
        && 6.0_f64.algebraic_div(3.0).to_bits() == 2.0_f64.to_bits()
        && 1.5_f64.algebraic_add(2.5).to_bits() == 4.0_f64.to_bits();

    report(
        "float_algebraic",
        Outcome::of(ops_32 && ops_64),
        &format!(
            "64-term ladder: f32 strict={strict_32:e} algebraic={algebraic_32:e}, \
             f64 strict={strict_64:e} algebraic={algebraic_64:e} \
             (reassociated by this build: {reassociated}); \
             add/sub/mul/div exact f32={ops_32} f64={ops_64}"
        ),
    )
}

/// `AtomicU32::from_mut_slice` / `AtomicU32::get_mut_slice` — reinterpret a
/// `&mut [u32]` as `&mut [AtomicU32]` and back, with no `unsafe` at the call
/// site.
///
/// Phase 05 wants this to replace hand-rolled pointer casts in the
/// parallel-fill paths, so the probe round-trips a slice through the atomic
/// view and mutates it while it is there.
fn probe_atomic_slices() -> Outcome {
    let mut plane: [u32; 4] = [1, 2, 3, 4];

    let view: &mut [AtomicU32] = AtomicU32::from_mut_slice(&mut plane);
    for (i, cell) in view.iter().enumerate() {
        let bump = u32::try_from(i).expect("probe slice length fits in u32");
        cell.fetch_add(bump, Ordering::Relaxed);
    }
    let back: &mut [u32] = AtomicU32::get_mut_slice(view);

    let ok = back == [1, 3, 5, 7];
    let observed = format!("{back:?}");
    report(
        "atomic_from_mut",
        Outcome::of(ok),
        &format!("[1,2,3,4] --from_mut_slice--> fetch_add(idx) --get_mut_slice--> {observed}"),
    )
}

/// `<[T]>::subslice_range` and `str::substr_range` — recover the offset of an
/// interior subslice/substring within its parent.
///
/// Both return the *new* `core::range::Range` (stable since 1.95), not
/// `core::ops::Range`; the two are distinct types and later phases must import
/// the right one.
fn probe_subslice_range() -> Outcome {
    let plane: [u16; 8] = [10, 11, 12, 13, 14, 15, 16, 17];
    let interior: &[u16] = &plane[2..6];
    let slice_range: Option<Range<usize>> = plane.subslice_range(interior);

    let name = "codec_series";
    let tail: &str = &name[6..];
    let str_range: Option<Range<usize>> = name.substr_range(tail);

    // A slice that is not part of the parent allocation must report `None`,
    // not a bogus offset — that is the property the index-recovery code will
    // depend on.
    let foreign: [u16; 2] = [12, 13];
    let disjoint = plane.subslice_range(&foreign);

    let ok = slice_range == Some(Range { start: 2, end: 6 })
        && str_range == Some(Range { start: 6, end: 12 })
        && disjoint.is_none();

    report(
        "substr_range",
        Outcome::of(ok),
        &format!(
            "plane[2..6] -> {slice_range:?}; \"codec_series\"[6..] -> {str_range:?}; \
             foreign slice -> {disjoint:?}"
        ),
    )
}

/// `i32::format_into` / `u32::format_into` with `core::fmt::NumBuffer` —
/// render an integer as `&str` into a stack buffer, with no allocation.
///
/// `NumBuffer<T>` is sized from the maximum digit count of `T`, so the probe
/// also prints its footprint: that number is the whole argument for using it
/// on the per-block metadata paths instead of `to_string()`.
fn probe_format_into() -> Outcome {
    let mut buf = NumBuffer::new();
    let rendered = u32::MAX.format_into(&mut buf);
    let unsigned_ok = rendered == "4294967295";
    let unsigned = rendered.to_owned();

    let mut signed_buf = NumBuffer::new();
    let signed = (-1972_i32).format_into(&mut signed_buf);
    let signed_ok = signed == "-1972";

    report(
        "int_format_into",
        Outcome::of(unsigned_ok && signed_ok),
        &format!(
            "u32::MAX -> {unsigned:?}, -1972i32 -> {signed:?}, \
             size_of::<NumBuffer<u32>>() = {} bytes (stack, no alloc)",
            size_of::<NumBuffer<u32>>()
        ),
    )
}

/// `str::strip_circumfix` — strip a matching prefix *and* suffix in one call,
/// yielding `None` unless both match.
///
/// Phase 06 uses this on codec-name parsing, where the two-step
/// `strip_prefix(..)?.strip_suffix(..)` chain currently reads badly.
fn probe_strip_circumfix() -> Outcome {
    let both = "codec:nd_lift:end".strip_circumfix("codec:", ":end");
    // Suffix absent: the whole call must fail, not silently strip the prefix.
    let prefix_only = "codec:nd_lift".strip_circumfix("codec:", ":end");
    // The prefix and suffix must not be allowed to overlap.
    let overlapping = "foo:bar:baz".strip_circumfix("foo:bar:", ":bar:baz");
    // A `char` suffix is accepted alongside a `&str` prefix — the two pattern
    // type parameters are independent.
    let mixed_pattern = "zfp:brick;".strip_circumfix("zfp:", ';');

    let ok = both == Some("nd_lift")
        && prefix_only.is_none()
        && overlapping.is_none()
        && mixed_pattern == Some("brick");

    report(
        "strip_circumfix",
        Outcome::of(ok),
        &format!(
            "\"codec:nd_lift:end\" -> {both:?}; missing suffix -> {prefix_only:?}; \
             overlapping -> {overlapping:?}; &str+char patterns -> {mixed_pattern:?}"
        ),
    )
}

/// `NonZero::<T>::from_str_radix` — parse a non-zero integer in an arbitrary
/// radix without the `parse().and_then(NonZero::new)` dance.
fn probe_nonzero_from_str_radix() -> Outcome {
    // `ParseIntError` is not `Copy`, so collapse each `Result` to a `Copy`
    // `Option<u32>` up front and use that for both the check and the report.
    let hex = NonZero::<u32>::from_str_radix("ff", 16)
        .map(NonZero::get)
        .ok();
    // Zero is not representable, so it must be a parse error rather than a
    // panic or a silent `NonZero(0)`.
    let zero_rejected = NonZero::<u32>::from_str_radix("0", 16).is_err();
    let binary = NonZero::<u32>::from_str_radix("1011", 2)
        .map(NonZero::get)
        .ok();

    let ok = hex == Some(255) && zero_rejected && binary == Some(0b1011);

    report(
        "nonzero_from_str_radix",
        Outcome::of(ok),
        &format!(
            "\"ff\" base 16 -> {hex:?}; \"0\" base 16 rejected -> {zero_rejected}; \
             \"1011\" base 2 -> {binary:?}"
        ),
    )
}

fn main() {
    println!(
        "nd-image-codecs Rust {} capability probe",
        env!("CARGO_PKG_RUST_VERSION")
    );
    println!(
        "{status:<11}  {feature:<24}  OBSERVED",
        status = "STATUS",
        feature = "FEATURE"
    );

    let outcomes = [
        probe_float_algebraic(),
        probe_atomic_slices(),
        probe_subslice_range(),
        probe_format_into(),
        probe_strip_circumfix(),
        probe_nonzero_from_str_radix(),
    ];

    let tally = |want: Outcome| outcomes.iter().filter(|o| **o == want).count();
    let (passed, failed, unavailable) = (
        tally(Outcome::Pass),
        tally(Outcome::Fail),
        tally(Outcome::Unavailable),
    );

    println!();
    println!(
        "summary: {passed} PASS, {failed} FAIL, {unavailable} UNAVAILABLE of {} probes",
        outcomes.len()
    );

    if failed > 0 {
        // Non-zero exit so a probe regression is visible to CI, not just to a
        // reader of the log.
        std::process::exit(1);
    }
}
