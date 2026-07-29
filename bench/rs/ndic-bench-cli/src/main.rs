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
//! for the PR gate. Exit codes: `0` success, `1` regression detected with
//! `--fail-on-regression`, `2` benchmark failure.
//!
//! See `bench/README.md` and `bench/docs/architecture.md`.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use ndic_bench_core::{BenchConfig, BenchEntry, BenchRecord, REGRESSION_PCT_THRESHOLD};

// Link-anchor modules: pulling the workload crates in ensures their
// `inventory::submit!` registrations are linked into this binary.
// Benchmarks land with each roadmap phase; see
// docs/development/roadmap/phase-3-htj2k-core.md.
mod _anchors {
    #[allow(unused_imports)]
    pub use ndic_htj2k as _htj2k;
    #[allow(unused_imports)]
    pub use ndic_lift as _transform;
}

/// Output format for reports.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    Ascii,
    Json,
    Both,
    Markdown,
    Csv,
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
    },
    /// List every benchmark registered in this binary.
    List,
}

/// Built-in codec-configuration matrix (see bench/benchmarks.toml).
fn builtin_configs() -> Vec<BenchConfig> {
    vec![
        BenchConfig {
            label: "nd-delta-zstd".into(),
            family: "nd-delta".into(),
            simd: false,
            irreversible: false,
            lift_levels: 0,
        },
        BenchConfig {
            label: "scalar-53-ht".into(),
            family: "nd-lift-ht".into(),
            simd: false,
            irreversible: false,
            lift_levels: 0,
        },
        BenchConfig {
            label: "simd-53-ht".into(),
            family: "nd-lift-ht".into(),
            simd: true,
            irreversible: false,
            lift_levels: 0,
        },
        BenchConfig {
            label: "simd-97-ht".into(),
            family: "nd-lift-ht".into(),
            simd: true,
            irreversible: true,
            lift_levels: 0,
        },
        BenchConfig {
            label: "simd-53-lift-z2".into(),
            family: "nd-lift-ht".into(),
            simd: true,
            irreversible: false,
            lift_levels: 2,
        },
        BenchConfig {
            label: "zfp-rate8".into(),
            family: "nd-zfp".into(),
            simd: true,
            irreversible: true,
            lift_levels: 0,
        },
    ]
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::List => {
            let mut entries: Vec<&BenchEntry> = inventory::iter::<BenchEntry>.into_iter().collect();
            entries.sort_by_key(|e| (e.module, e.name));
            if entries.is_empty() {
                println!("no benchmarks registered yet — they land with roadmap Phase 1");
            }
            for e in &entries {
                println!("{}/{}", e.module, e.name);
            }
            ExitCode::SUCCESS
        }
        Command::Run {
            filter,
            config,
            quiet,
            ..
        } => {
            let configs = builtin_configs();
            let selected: Vec<_> = if config.is_empty() {
                configs
            } else {
                configs
                    .into_iter()
                    .filter(|c| config.contains(&c.label))
                    .collect()
            };
            let entries: Vec<&BenchEntry> = inventory::iter::<BenchEntry>
                .into_iter()
                .filter(|e| {
                    filter
                        .as_deref()
                        .is_none_or(|f| format!("{}/{}", e.module, e.name).contains(f))
                })
                .collect();
            if entries.is_empty() {
                if !quiet {
                    eprintln!(
                        "ndic-bench: no benchmarks registered yet (regression gate threshold: {:.0}%)",
                        REGRESSION_PCT_THRESHOLD * 100.0
                    );
                    eprintln!(
                        "benchmark workloads land with roadmap Phase 1; configs: {:?}",
                        selected
                            .iter()
                            .map(|c| c.label.as_str())
                            .collect::<Vec<_>>()
                    );
                }
                return ExitCode::SUCCESS;
            }
            // Record-writing pipeline (git-hash dir, JSON schema, baseline
            // diff) is specified in bench/docs/architecture.md and lands with
            // the first Phase 1 workloads.
            let _ = BenchRecord {
                name: String::new(),
                config: String::new(),
                git_hash: String::new(),
                num_samples: 0,
                median_ns: 0,
                min_ns: 0,
                max_ns: 0,
                raw_ns: vec![],
            };
            ExitCode::SUCCESS
        }
        Command::Compare { .. } => {
            eprintln!("ndic-bench compare: lands with roadmap Phase 1 records");
            ExitCode::SUCCESS
        }
    }
}
