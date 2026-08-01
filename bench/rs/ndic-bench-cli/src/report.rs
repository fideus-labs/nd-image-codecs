//! Report rendering for `ndic-bench`: records (a plain run) and diff rows
//! (a run or compare against a baseline) in ascii / markdown / json / csv.

use ndic_bench_core::{BenchRecord, DiffRow};

/// Render nanoseconds human-readably.
#[allow(clippy::cast_precision_loss)]
fn fmt_ns(ns: u64) -> String {
    let ns_f = ns as f64;
    if ns >= 1_000_000_000 {
        format!("{:.2} s", ns_f / 1e9)
    } else if ns >= 1_000_000 {
        format!("{:.2} ms", ns_f / 1e6)
    } else if ns >= 1_000 {
        format!("{:.2} µs", ns_f / 1e3)
    } else {
        format!("{ns} ns")
    }
}

fn fmt_ratio(ratio: Option<f64>) -> String {
    ratio.map_or_else(|| "-".to_owned(), |r| format!("{r:.4}"))
}

fn fmt_change(pct: Option<f64>) -> String {
    pct.map_or_else(|| "new".to_owned(), |p| format!("{:+.1} %", p * 100.0))
}

fn row_status(row: &DiffRow) -> &'static str {
    match (row.time_regressed, row.ratio_regressed) {
        (false, false) => "ok",
        (true, false) => "TIME-REGRESSED",
        (false, true) => "RATIO-REGRESSED",
        (true, true) => "TIME+RATIO-REGRESSED",
    }
}

/// Render plain records (no baseline) in the given format.
pub fn records(records: &[BenchRecord], format: crate::Format) -> String {
    let header = ["config", "benchmark", "median", "min", "max", "ratio"];
    let rows: Vec<[String; 6]> = records
        .iter()
        .map(|r| {
            [
                r.config.clone(),
                r.name.clone(),
                fmt_ns(r.median_ns),
                fmt_ns(r.min_ns),
                fmt_ns(r.max_ns),
                fmt_ratio(r.ratio()),
            ]
        })
        .collect();
    match format {
        crate::Format::Json => serde_json::to_string_pretty(records).expect("records serialize"),
        crate::Format::Both => format!(
            "{}\n{}",
            table(&header, &rows, TableStyle::Ascii),
            serde_json::to_string_pretty(records).expect("records serialize")
        ),
        crate::Format::Ascii => table(&header, &rows, TableStyle::Ascii),
        crate::Format::Markdown => table(&header, &rows, TableStyle::Markdown),
        crate::Format::Csv => table(&header, &rows, TableStyle::Csv),
    }
}

/// Render diff rows (current vs baseline) in the given format.
pub fn diffs(rows: &[DiffRow], format: crate::Format) -> String {
    let header = [
        "config",
        "benchmark",
        "median",
        "baseline",
        "change",
        "ratio",
        "base ratio",
        "status",
    ];
    let cells: Vec<[String; 8]> = rows
        .iter()
        .map(|d| {
            [
                d.config.clone(),
                d.name.clone(),
                fmt_ns(d.median_ns),
                d.baseline_median_ns.map_or_else(|| "-".to_owned(), fmt_ns),
                fmt_change(d.time_change_pct),
                fmt_ratio(d.ratio),
                fmt_ratio(d.baseline_ratio),
                row_status(d).to_owned(),
            ]
        })
        .collect();
    match format {
        crate::Format::Json => serde_json::to_string_pretty(rows).expect("rows serialize"),
        crate::Format::Both => format!(
            "{}\n{}",
            table(&header, &cells, TableStyle::Ascii),
            serde_json::to_string_pretty(rows).expect("rows serialize")
        ),
        crate::Format::Ascii => table(&header, &cells, TableStyle::Ascii),
        crate::Format::Markdown => table(&header, &cells, TableStyle::Markdown),
        crate::Format::Csv => table(&header, &cells, TableStyle::Csv),
    }
}

#[derive(Clone, Copy)]
enum TableStyle {
    Ascii,
    Markdown,
    Csv,
}

fn table<const N: usize>(header: &[&str; N], rows: &[[String; N]], style: TableStyle) -> String {
    match style {
        TableStyle::Csv => {
            let mut out = header.join(",");
            for row in rows {
                out.push('\n');
                out.push_str(&row.join(","));
            }
            out
        }
        TableStyle::Markdown => {
            let mut lines = vec![
                format!("| {} |", header.join(" | ")),
                format!("|{}", " --- |".repeat(N)),
            ];
            lines.extend(rows.iter().map(|row| format!("| {} |", row.join(" | "))));
            lines.join("\n")
        }
        TableStyle::Ascii => {
            let mut widths = header.map(str::len);
            for row in rows {
                for (w, cell) in widths.iter_mut().zip(row) {
                    *w = (*w).max(cell.chars().count());
                }
            }
            let line = |cells: &[String; N]| {
                cells
                    .iter()
                    .zip(&widths)
                    .map(|(c, w)| format!("{c:w$}"))
                    .collect::<Vec<_>>()
                    .join("  ")
                    .trim_end()
                    .to_owned()
            };
            let mut out = line(&header.map(str::to_owned));
            out.push('\n');
            out.push_str(&widths.map(|w| "-".repeat(w)).join("  "));
            for row in rows {
                out.push('\n');
                out.push_str(&line(row));
            }
            out
        }
    }
}
