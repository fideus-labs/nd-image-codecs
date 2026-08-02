//! `ndic-bench` — the benchmark driver binary.
//!
//! Discovers every [`BenchEntry`] registered at link time via `inventory`,
//! runs each across the requested codec-configuration matrix, writes one JSON
//! [`BenchRecord`] per `(bench, config)` under
//! `target/benchmarks/<git-hash>/<config>/`, and (optionally) diffs the run
//! against a committed baseline under `bench/baselines/<name>/`.
//!
//! Interface: `run`, `compare`, and `list`
//! subcommands; `--format ascii|json|both|markdown|csv`; `--fail-on-regression`
//! for the PR gate; `--gate time|ratio|both` to select which regressions
//! gate (ratio is deterministic and safe across machine classes).
//! Exit codes: `0` success, `1` regression detected with
//! `--fail-on-regression`, `2` benchmark failure.
//!
//! Python-side lanes (e.g. `bench/py/run_nd_delta.py`) write the same record
//! schema into the same records tree, so `compare` gates them identically.
//!
//! See `bench/README.md` and `bench/docs/architecture.md`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use ndic_bench_core::{Baseline, BenchConfig, BenchEntry, BenchRecord, DiffRow, load_records};

mod report;
// The workloads themselves: compiled into this binary, they register with
// `inventory` at link time and are never named directly. Rust workloads land
// with each codec's roadmap phase; the Phase 1 nd-delta lanes are Python-side
// (see bench/py/).
mod workloads;

/// Output format for reports.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Format {
    /// Aligned plain-text table.
    Ascii,
    /// Pretty-printed JSON.
    Json,
    /// Ascii table followed by JSON.
    Both,
    /// GitHub-flavored markdown table (CI comments).
    Markdown,
    /// Comma-separated values.
    Csv,
}

/// Which regression kinds cause a non-zero exit with `--fail-on-regression`.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Gate {
    /// Median time ≥ 10 % over baseline and beyond its σ envelope.
    Time,
    /// Compression ratio ≥ 2 % worse than baseline (deterministic).
    Ratio,
    /// Either kind.
    Both,
}

impl Gate {
    fn tripped(self, row: &DiffRow) -> bool {
        match self {
            Self::Time => row.time_regressed,
            Self::Ratio => row.ratio_regressed,
            Self::Both => row.time_regressed || row.ratio_regressed,
        }
    }

    /// Whether time regressions are held by this gate.
    fn gates_time(self) -> bool {
        matches!(self, Self::Time | Self::Both)
    }

    /// Whether ratio regressions are held by this gate.
    fn gates_ratio(self) -> bool {
        matches!(self, Self::Ratio | Self::Both)
    }
}

/// nd-image-codecs benchmark driver.
#[derive(Parser)]
#[command(name = "ndic-bench", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run benchmarks across the codec-config matrix and write JSON records.
    Run {
        /// Only run benchmarks whose `<module>/<name>` matches this substring.
        #[arg(long)]
        filter: Option<String>,
        /// Config labels to run (default: all built-in configs).
        #[arg(long)]
        config: Vec<String>,
        /// Report format.
        #[arg(long, value_enum, default_value = "ascii")]
        format: Format,
        /// Compare against this committed baseline (e.g. `main`).
        #[arg(long)]
        baseline: Option<String>,
        /// Exit non-zero if any benchmark regresses beyond the gate.
        #[arg(long)]
        fail_on_regression: bool,
        /// Which regression kinds gate.
        #[arg(long, value_enum, default_value = "both")]
        gate: Gate,
        /// Suppress progress output.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Diff two record directories (or a run against a named baseline).
    Compare {
        /// Baseline records directory or baseline name.
        baseline: PathBuf,
        /// Candidate records directory (default: latest run).
        candidate: Option<PathBuf>,
        /// Report format.
        #[arg(long, value_enum, default_value = "ascii")]
        format: Format,
        /// Exit non-zero if any benchmark regresses beyond the gate.
        #[arg(long)]
        fail_on_regression: bool,
        /// Which regression kinds gate.
        #[arg(long, value_enum, default_value = "both")]
        gate: Gate,
    },
    /// List every benchmark registered in this binary.
    List,
}

/// Built-in codec-configuration matrix (see bench/benchmarks.toml).
fn builtin_configs() -> Vec<BenchConfig> {
    let cfg = |label: &str, family: &str, simd, irreversible, lift_levels| BenchConfig {
        label: label.into(),
        family: family.into(),
        simd,
        irreversible,
        lift_levels,
    };
    vec![
        cfg("blosc-zstd", "baseline", false, false, 0),
        cfg("nd-delta-zstd", "nd-delta", false, false, 0),
        cfg("nd-delta-lz4", "nd-delta", false, false, 0),
        cfg("nd-lift-delta-zstd", "nd-lift", false, false, 0),
        cfg("nd-lift-53-zstd", "nd-lift", false, false, 2),
        cfg("scalar-53-ht", "nd-lift-ht", false, false, 0),
        cfg("simd-53-ht", "nd-lift-ht", true, false, 0),
        cfg("simd-97-ht", "nd-lift-ht", true, true, 0),
        cfg("simd-53-lift-z2", "nd-lift-ht", true, false, 2),
        cfg("zfp-rate8", "nd-zfp", true, true, 0),
    ]
}

/// Short git hash of the working tree, or `unknown` outside a checkout.
fn git_hash() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".into(), |s| s.trim().to_owned())
}

/// Resolve a baseline argument: an existing directory is used as-is, anything
/// else is looked up under `bench/baselines/<name>/`.
fn resolve_baseline(arg: &Path) -> PathBuf {
    if arg.is_dir() {
        arg.to_path_buf()
    } else {
        Path::new("bench/baselines").join(arg)
    }
}

/// Load baseline records from a directory (with or without a manifest).
fn load_baseline(dir: &Path) -> Result<Vec<BenchRecord>, String> {
    if dir.join("manifest.json").is_file() {
        Baseline::load(dir)
            .map(|b| b.records)
            .map_err(|e| format!("failed to load baseline {}: {e}", dir.display()))
    } else {
        load_records(dir).map_err(|e| format!("failed to load records {}: {e}", dir.display()))
    }
}

fn gate_exit(rows: &[DiffRow], gate: Gate, fail_on_regression: bool) -> ExitCode {
    let tripped: Vec<&DiffRow> = rows.iter().filter(|r| gate.tripped(r)).collect();
    for row in &tripped {
        eprintln!("regressed: {} [{}]", row.name, row.config);
    }
    if fail_on_regression && !tripped.is_empty() {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// The `run` subcommand: execute registered workloads across the selected
/// configs, write records, and optionally gate against a baseline.
fn run(
    filter: Option<&str>,
    config: &[String],
    format: Format,
    baseline: Option<&str>,
    fail_on_regression: bool,
    gate: Gate,
    quiet: bool,
) -> ExitCode {
    let configs = builtin_configs();
    let selected: Vec<_> = if config.is_empty() {
        configs
    } else {
        // A typo'd label would otherwise select nothing and let the gate
        // pass without measuring anything — fail fast instead.
        let known: Vec<&str> = configs.iter().map(|c| c.label.as_str()).collect();
        if let Some(bad) = config.iter().find(|c| !known.contains(&c.as_str())) {
            eprintln!(
                "ndic-bench: unknown config {bad:?} (known: {})",
                known.join(", ")
            );
            return ExitCode::from(2);
        }
        configs
            .into_iter()
            .filter(|c| config.contains(&c.label))
            .collect()
    };
    let entries: Vec<&BenchEntry> = inventory::iter::<BenchEntry>
        .into_iter()
        .filter(|e| filter.is_none_or(|f| format!("{}/{}", e.module, e.name).contains(f)))
        .collect();
    let hash = git_hash();
    let root = Path::new("target/benchmarks").join(&hash);
    let mut records: Vec<BenchRecord> = Vec::new();
    for entry in &entries {
        for cfg in &selected {
            if !quiet {
                eprintln!("running {}/{} [{}]", entry.module, entry.name, cfg.label);
            }
            let output = (entry.run)(cfg);
            if output.raw_ns.is_empty() {
                // The entry opted out of this config (e.g. a transform
                // workload on a non-transform lane) — no record.
                continue;
            }
            let name = format!("{}/{}", entry.module, entry.name);
            let record = BenchRecord::from_output(&name, &cfg.label, &hash, &output);
            if let Err(e) = record.save(&root) {
                eprintln!("ndic-bench: failed to write record for {name}: {e}");
                return ExitCode::from(2);
            }
            records.push(record);
        }
    }
    if entries.is_empty() && !quiet {
        eprintln!(
            "ndic-bench: no Rust benchmarks registered (Phase 1 nd-delta lanes are \
             Python-side; see bench/py/). Records dir: {}",
            root.display()
        );
    }
    if let Some(name) = baseline {
        let dir = resolve_baseline(Path::new(name));
        let base = match load_baseline(&dir) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("ndic-bench: {e}");
                return ExitCode::from(2);
            }
        };
        let rows = ndic_bench_core::diff(&records, &base);
        println!("{}", report::diffs(&rows, format, gate));
        gate_exit(&rows, gate, fail_on_regression)
    } else {
        println!("{}", report::records(&records, format));
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::List => {
            let mut entries: Vec<&BenchEntry> = inventory::iter::<BenchEntry>.into_iter().collect();
            entries.sort_by_key(|e| (e.module, e.name));
            if entries.is_empty() {
                println!(
                    "no Rust benchmarks registered yet — the Phase 1 nd-delta lanes are \
                     Python-side (bench/py/run_nd_delta.py)"
                );
            }
            for e in &entries {
                println!("{}/{}", e.module, e.name);
            }
            ExitCode::SUCCESS
        }
        Command::Run {
            filter,
            config,
            format,
            baseline,
            fail_on_regression,
            gate,
            quiet,
        } => run(
            filter.as_deref(),
            &config,
            format,
            baseline.as_deref(),
            fail_on_regression,
            gate,
            quiet,
        ),
        Command::Compare {
            baseline,
            candidate,
            format,
            fail_on_regression,
            gate,
        } => {
            let baseline_dir = resolve_baseline(&baseline);
            let candidate_dir =
                candidate.unwrap_or_else(|| Path::new("target/benchmarks").join(git_hash()));
            let (base, current) =
                match (load_baseline(&baseline_dir), load_baseline(&candidate_dir)) {
                    (Ok(b), Ok(c)) => (b, c),
                    (Err(e), _) | (_, Err(e)) => {
                        eprintln!("ndic-bench: {e}");
                        return ExitCode::from(2);
                    }
                };
            if current.is_empty() {
                eprintln!(
                    "ndic-bench: no candidate records under {}",
                    candidate_dir.display()
                );
                return ExitCode::from(2);
            }
            let rows = ndic_bench_core::diff(&current, &base);
            println!("{}", report::diffs(&rows, format, gate));
            gate_exit(&rows, gate, fail_on_regression)
        }
    }
}
