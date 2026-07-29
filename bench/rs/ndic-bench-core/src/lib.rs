//! `ndic-bench-core` — the shared benchmark layer for nd-image-codecs.
//!
//! Modeled on the `tracel-ai/burn-bench` approach, this crate provides the
//! pieces the `ndic-bench` CLI needs to run a uniform,
//! cross-configuration benchmark suite:
//!
//! - [`BenchEntry`] — a link-time-registered benchmark descriptor. Any crate
//!   adds a benchmark by `inventory::submit!`-ing a `BenchEntry`; the CLI walks
//!   the [`inventory`] registry at startup, so no central list needs editing.
//! - [`BenchRecord`] — the single JSON record schema written per
//!   `(bench, config)` to `target/benchmarks/<git-hash>/<config>/<name>.json`,
//!   byte-compatible with the named baselines under `bench/baselines/`.
//! - [`Baseline`] load/save and a median-plus-σ regression diff implementing
//!   the ≥10 % **and** beyond-noise CI gate.
//!
//! The nd-image-codecs "backend matrix" is a **codec-configuration** matrix rather
//! than a compute-backend matrix: scalar vs SIMD block coder, 5/3 vs 9/7,
//! lift levels 0 (per-plane) vs >0 (`nd_lift` z-decorrelation), and the reference lane
//! (`OpenJPH` `ojph_compress`/`ojph_expand`, and `imagecodecs` HTJ2K).

use serde::{Deserialize, Serialize};

/// A single registered benchmark. Constructed in each crate's `benches` module
/// and collected at link time via [`inventory`].
pub struct BenchEntry {
    /// Module slug, e.g. `htj2k` or `transform`.
    pub module: &'static str,
    /// Benchmark name within the module, e.g. `cleanup_encode_64x64`.
    pub name: &'static str,
    /// The workload. Returns nanoseconds per sample median-ready durations.
    pub run: fn(&BenchConfig) -> Vec<u64>,
}

impl BenchEntry {
    /// Construct a registry entry.
    #[must_use]
    pub const fn new(
        module: &'static str,
        name: &'static str,
        run: fn(&BenchConfig) -> Vec<u64>,
    ) -> Self {
        Self { module, name, run }
    }
}

inventory::collect!(BenchEntry);

/// One point in the codec-configuration matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchConfig {
    /// Config label used in the on-disk path, e.g. `simd-53-ht`.
    pub label: String,
    /// Codec family: `nd-delta`, `nd-lift-ht`, or `nd-zfp`.
    pub family: String,
    /// Whether SIMD lanes are enabled.
    pub simd: bool,
    /// `false` ⇒ 5/3 reversible, `true` ⇒ 9/7 irreversible.
    pub irreversible: bool,
    /// `nd_lift` decomposition levels along z (0 ⇒ no cross-axis transform).
    pub lift_levels: u8,
}

/// One benchmark measurement, serialized to a per-record JSON file. The field
/// set is deliberately minimal and stable so the viewer, the baseline tooling,
/// and external analysis scripts can all consume it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRecord {
    /// `<module>/<name>`.
    pub name: String,
    /// Config label.
    pub config: String,
    /// Source revision the record was captured at.
    pub git_hash: String,
    /// Sample count.
    pub num_samples: usize,
    /// Median nanoseconds.
    pub median_ns: u64,
    /// Minimum nanoseconds.
    pub min_ns: u64,
    /// Maximum nanoseconds.
    pub max_ns: u64,
    /// Raw per-sample durations (nanoseconds).
    pub raw_ns: Vec<u64>,
}

/// A regression is flagged when the current median is ≥ this fraction over the
/// baseline median **and** the increase exceeds the baseline's σ envelope.
pub const REGRESSION_PCT_THRESHOLD: f64 = 0.10;
