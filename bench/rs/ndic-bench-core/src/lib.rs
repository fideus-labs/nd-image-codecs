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
//! - [`Baseline`] load/save and a median-plus-σ regression [`diff`]
//!   implementing the ≥10 % **and** beyond-noise CI time gate, plus a
//!   deterministic compressed-size gate for ratio-bearing records.
//!
//! The nd-image-codecs "backend matrix" is a **codec-configuration** matrix rather
//! than a compute-backend matrix: scalar vs SIMD block coder, 5/3 vs 9/7,
//! lift levels 0 (per-plane) vs >0 (`nd_lift` z-decorrelation), and the reference lane
//! (`OpenJPH` `ojph_compress`/`ojph_expand`, and `imagecodecs` HTJ2K).

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What one benchmark run produces: per-sample durations, plus the processed
/// and produced byte counts when the workload is a codec (enabling ratio
/// tracking alongside throughput).
#[derive(Debug, Clone, Default)]
pub struct BenchOutput {
    /// Raw per-sample durations (nanoseconds).
    pub raw_ns: Vec<u64>,
    /// Uncompressed bytes processed per sample, if applicable.
    pub bytes_in: Option<u64>,
    /// Compressed bytes produced per sample, if applicable.
    pub bytes_out: Option<u64>,
}

impl From<Vec<u64>> for BenchOutput {
    fn from(raw_ns: Vec<u64>) -> Self {
        Self {
            raw_ns,
            ..Self::default()
        }
    }
}

/// A single registered benchmark. Constructed in each crate's `benches` module
/// and collected at link time via [`inventory`].
pub struct BenchEntry {
    /// Module slug, e.g. `htj2k` or `transform`.
    pub module: &'static str,
    /// Benchmark name within the module, e.g. `cleanup_encode_64x64`.
    pub name: &'static str,
    /// The workload: timed samples plus optional codec byte counts.
    pub run: fn(&BenchConfig) -> BenchOutput,
}

impl BenchEntry {
    /// Construct a registry entry.
    #[must_use]
    pub const fn new(
        module: &'static str,
        name: &'static str,
        run: fn(&BenchConfig) -> BenchOutput,
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
    /// Codec family: `nd-delta`, `nd-lift-ht`, `nd-zfp`, or `baseline` for
    /// reference lanes such as plain blosc-zstd.
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
    /// Uncompressed bytes processed per sample (codec workloads only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_in: Option<u64>,
    /// Compressed bytes produced per sample (codec workloads only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_out: Option<u64>,
}

impl BenchRecord {
    /// Build a record from one workload run.
    #[must_use]
    pub fn from_output(name: &str, config: &str, git_hash: &str, output: &BenchOutput) -> Self {
        Self {
            name: name.to_owned(),
            config: config.to_owned(),
            git_hash: git_hash.to_owned(),
            num_samples: output.raw_ns.len(),
            median_ns: median(&output.raw_ns),
            min_ns: output.raw_ns.iter().copied().min().unwrap_or(0),
            max_ns: output.raw_ns.iter().copied().max().unwrap_or(0),
            raw_ns: output.raw_ns.clone(),
            bytes_in: output.bytes_in,
            bytes_out: output.bytes_out,
        }
    }

    /// `bytes_out / bytes_in` when both are present and non-zero.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn ratio(&self) -> Option<f64> {
        match (self.bytes_in, self.bytes_out) {
            (Some(i), Some(o)) if i > 0 => Some(o as f64 / i as f64),
            _ => None,
        }
    }

    /// The record's on-disk file name: `<module>__<name>.json`.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}.json", self.name.replace('/', "__"))
    }

    /// Write the record to `<root>/<config>/<module>__<name>.json`.
    ///
    /// # Errors
    /// Propagates filesystem and serialization errors.
    pub fn save(&self, root: &Path) -> io::Result<()> {
        let dir = root.join(&self.config);
        fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(dir.join(self.file_name()), json + "\n")
    }
}

/// Load every `*.json` record under `root` (searching one level of config
/// subdirectories, mirroring the layout [`BenchRecord::save`] writes).
///
/// # Errors
/// Propagates filesystem errors; files that fail to parse as records (e.g. a
/// baseline `manifest.json`) are skipped.
pub fn load_records(root: &Path) -> io::Result<Vec<BenchRecord>> {
    let mut records = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "json")
                && let Ok(text) = fs::read_to_string(&path)
                && let Ok(record) = serde_json::from_str::<BenchRecord>(&text)
            {
                records.push(record);
            }
        }
    }
    records.sort_by(|a, b| (&a.config, &a.name).cmp(&(&b.config, &b.name)));
    Ok(records)
}

/// Provenance sidecar for a committed baseline (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineManifest {
    /// Baseline name, e.g. `main`.
    pub name: String,
    /// Revision the records were captured at.
    pub git_hash: String,
    /// Runner class the records were captured on; the σ noise envelope is
    /// only meaningful against records from the same class.
    pub machine: String,
    /// Toolchain string, e.g. `rustc 1.91.0`.
    pub toolchain: String,
    /// Capture date, `YYYY-MM-DD`.
    pub captured: String,
}

/// A committed baseline: its manifest plus its records tree.
#[derive(Debug, Clone)]
pub struct Baseline {
    /// Provenance manifest.
    pub manifest: BaselineManifest,
    /// All records in the baseline.
    pub records: Vec<BenchRecord>,
}

impl Baseline {
    /// Load a baseline directory (`manifest.json` + records tree).
    ///
    /// # Errors
    /// Fails when the manifest is missing/invalid or the tree is unreadable.
    pub fn load(dir: &Path) -> io::Result<Self> {
        let manifest_text = fs::read_to_string(dir.join("manifest.json"))?;
        let manifest: BaselineManifest = serde_json::from_str(&manifest_text)?;
        let records = load_records(dir)?;
        Ok(Self { manifest, records })
    }

    /// Write the baseline: `manifest.json` plus one file per record.
    ///
    /// # Errors
    /// Propagates filesystem and serialization errors.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(&self.manifest)?;
        fs::write(dir.join("manifest.json"), json + "\n")?;
        for record in &self.records {
            record.save(dir)?;
        }
        Ok(())
    }
}

/// A regression is flagged when the current median is ≥ this fraction over the
/// baseline median **and** the increase exceeds the baseline's σ envelope.
pub const REGRESSION_PCT_THRESHOLD: f64 = 0.10;

/// A compression regression is flagged when the ratio (`bytes_out /
/// bytes_in`) worsens by ≥ this fraction over the baseline. Normalizing by
/// `bytes_in` keeps the gate meaningful when the fixture size changes, and
/// sizes are deterministic, so no noise envelope applies — this gate holds
/// even across machine classes.
pub const RATIO_REGRESSION_PCT_THRESHOLD: f64 = 0.02;

/// Median of raw per-sample durations (0 for an empty slice).
#[must_use]
pub fn median(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        u64::midpoint(sorted[mid - 1], sorted[mid])
    } else {
        sorted[mid]
    }
}

/// Population standard deviation of raw per-sample durations.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn sigma(samples: &[u64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().map(|&s| s as f64).sum::<f64>() / n;
    let var = samples
        .iter()
        .map(|&s| (s as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt()
}

/// One row of a current-vs-baseline comparison.
#[derive(Debug, Clone, Serialize)]
pub struct DiffRow {
    /// `<module>/<name>`.
    pub name: String,
    /// Config label.
    pub config: String,
    /// Current median nanoseconds.
    pub median_ns: u64,
    /// Baseline median nanoseconds (`None` ⇒ new benchmark, ungated).
    pub baseline_median_ns: Option<u64>,
    /// Median time change as a fraction (+0.25 = 25 % slower).
    pub time_change_pct: Option<f64>,
    /// Current compression ratio (`bytes_out / bytes_in`).
    pub ratio: Option<f64>,
    /// Baseline compression ratio.
    pub baseline_ratio: Option<f64>,
    /// Median ≥ 10 % over baseline **and** beyond the baseline's σ envelope.
    pub time_regressed: bool,
    /// Compression ratio (`bytes_out / bytes_in`) worsened ≥ 2 % over the
    /// baseline.
    pub ratio_regressed: bool,
}

/// Compare current records against baseline records, pairing by
/// `(config, name)`. Current records with no baseline partner appear with
/// `baseline_median_ns: None` and never gate.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn diff(current: &[BenchRecord], baseline: &[BenchRecord]) -> Vec<DiffRow> {
    current
        .iter()
        .map(|cur| {
            let base = baseline
                .iter()
                .find(|b| b.config == cur.config && b.name == cur.name);
            let time_change_pct = base.and_then(|b| {
                (b.median_ns > 0)
                    .then(|| (cur.median_ns as f64 - b.median_ns as f64) / b.median_ns as f64)
            });
            let time_regressed = base.is_some_and(|b| {
                b.median_ns > 0
                    && cur.median_ns as f64 >= b.median_ns as f64 * (1.0 + REGRESSION_PCT_THRESHOLD)
                    && (cur.median_ns as f64 - b.median_ns as f64) > sigma(&b.raw_ns)
            });
            // Compare ratios, not absolute bytes_out, so a fixture-size
            // change between baseline and candidate doesn't misfire.
            let ratio_regressed = base.is_some_and(|b| match (cur.ratio(), b.ratio()) {
                (Some(cur_ratio), Some(base_ratio)) if base_ratio > 0.0 => {
                    cur_ratio >= base_ratio * (1.0 + RATIO_REGRESSION_PCT_THRESHOLD)
                }
                _ => false,
            });
            DiffRow {
                name: cur.name.clone(),
                config: cur.config.clone(),
                median_ns: cur.median_ns,
                baseline_median_ns: base.map(|b| b.median_ns),
                time_change_pct,
                ratio: cur.ratio(),
                baseline_ratio: base.and_then(BenchRecord::ratio),
                time_regressed,
                ratio_regressed,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        config: &str,
        name: &str,
        raw_ns: Vec<u64>,
        bytes: Option<(u64, u64)>,
    ) -> BenchRecord {
        BenchRecord::from_output(
            name,
            config,
            "deadbeef",
            &BenchOutput {
                raw_ns,
                bytes_in: bytes.map(|(i, _)| i),
                bytes_out: bytes.map(|(_, o)| o),
            },
        )
    }

    #[test]
    fn median_and_sigma() {
        assert_eq!(median(&[]), 0);
        assert_eq!(median(&[5]), 5);
        assert_eq!(median(&[1, 9]), 5);
        assert_eq!(median(&[3, 1, 2]), 2);
        assert!(sigma(&[10, 10, 10]) < f64::EPSILON);
        assert!((sigma(&[8, 12]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn from_output_computes_stats_and_ratio() {
        let rec = record("cfg", "m/n", vec![30, 10, 20], Some((1000, 250)));
        assert_eq!(
            (rec.median_ns, rec.min_ns, rec.max_ns, rec.num_samples),
            (20, 10, 30, 3)
        );
        assert!((rec.ratio().unwrap() - 0.25).abs() < 1e-12);
        assert_eq!(rec.file_name(), "m__n.json");
    }

    #[test]
    fn save_load_roundtrip_and_manifest_skipped() {
        let dir = std::env::temp_dir().join(format!("ndic-bench-core-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let rec = record("lane-a", "mod/bench", vec![10, 20, 30], Some((100, 50)));
        rec.save(&dir).unwrap();
        // A manifest must not be parsed as a record.
        fs::write(dir.join("manifest.json"), "{\"name\":\"main\"}").unwrap();
        let loaded = load_records(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "mod/bench");
        assert_eq!(loaded[0].bytes_out, Some(50));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn baseline_save_load_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("ndic-bench-baseline-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let baseline = Baseline {
            manifest: BaselineManifest {
                name: "main".into(),
                git_hash: "deadbeef".into(),
                machine: "test".into(),
                toolchain: "rustc test".into(),
                captured: "2026-08-01".into(),
            },
            records: vec![record("lane-a", "mod/bench", vec![10], None)],
        };
        baseline.save(&dir).unwrap();
        let loaded = Baseline::load(&dir).unwrap();
        assert_eq!(loaded.manifest.git_hash, "deadbeef");
        assert_eq!(loaded.records.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    fn one(cur: &BenchRecord, base: &BenchRecord) -> DiffRow {
        diff(std::slice::from_ref(cur), std::slice::from_ref(base))
            .pop()
            .unwrap()
    }

    #[test]
    fn time_gate_needs_both_conditions() {
        // Baseline: median 100, noisy (σ ≈ 20).
        let noisy_base = record("cfg", "m/n", vec![70, 90, 100, 110, 130], None);
        // +20 % but within the noise envelope (Δ20 ≤ σ≈20.0) → clean.
        let within_noise = record("cfg", "m/n", vec![120, 120, 120], None);
        assert!(!one(&within_noise, &noisy_base).time_regressed);
        // +50 % and beyond noise → regressed.
        let slow = record("cfg", "m/n", vec![150, 150, 150], None);
        assert!(one(&slow, &noisy_base).time_regressed);
        // Quiet baseline: +20 % now exceeds σ ≈ 0 → regressed.
        let quiet_base = record("cfg", "m/n", vec![100, 100, 100], None);
        let slower = record("cfg", "m/n", vec![120, 120, 120], None);
        assert!(one(&slower, &quiet_base).time_regressed);
        // +5 % is under the 10 % threshold → clean.
        let ok = record("cfg", "m/n", vec![105, 105, 105], None);
        assert!(!one(&ok, &quiet_base).time_regressed);
    }

    #[test]
    fn ratio_gate_is_deterministic() {
        let base = record("cfg", "m/n", vec![100], Some((1000, 200)));
        let bigger = record("cfg", "m/n", vec![100], Some((1000, 205)));
        assert!(one(&bigger, &base).ratio_regressed);
        let close = record("cfg", "m/n", vec![100], Some((1000, 203)));
        assert!(!one(&close, &base).ratio_regressed);
        let no_bytes = record("cfg", "m/n", vec![100], None);
        assert!(!one(&no_bytes, &base).ratio_regressed);
    }

    #[test]
    fn ratio_gate_normalizes_for_fixture_size_changes() {
        let base = record("cfg", "m/n", vec![100], Some((1000, 200)));
        // Fixture doubled, ratio unchanged → bytes_out doubling is not a
        // regression.
        let doubled = record("cfg", "m/n", vec![100], Some((2000, 400)));
        assert!(!one(&doubled, &base).ratio_regressed);
        // Fixture halved but ratio worsened (0.206 vs 0.200) → regression
        // even though absolute bytes_out shrank.
        let worse = record("cfg", "m/n", vec![100], Some((500, 103)));
        assert!(one(&worse, &base).ratio_regressed);
    }

    #[test]
    fn new_benchmark_never_gates() {
        let cur = record("cfg", "m/new", vec![100], Some((10, 20)));
        let rows = diff(&[cur], &[]);
        assert_eq!(rows[0].baseline_median_ns, None);
        assert!(!rows[0].time_regressed && !rows[0].ratio_regressed);
    }
}
