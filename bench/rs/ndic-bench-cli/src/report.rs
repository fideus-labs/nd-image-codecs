//! Report rendering for `ndic-bench`: records (a plain run) and diff rows
//! (a run or compare against a baseline) in ascii / markdown / json / csv.

use core::fmt::NumBuffer;

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
        // The one integer branch of the table's busiest cell — every record
        // renders three of these (median, min, max), and the sub-microsecond
        // rows are the block-coder and wavelet lanes, i.e. most of a run.
        // `format_into` writes the digits into a stack `NumBuffer`, so the
        // only allocation left is the returned `String`, sized exactly.
        let mut buf = NumBuffer::new();
        let digits = ns.format_into(&mut buf);
        let mut out = String::with_capacity(digits.len() + 3);
        out.push_str(digits);
        out.push_str(" ns");
        out
    }
}

fn fmt_ratio(ratio: Option<f64>) -> String {
    ratio.map_or_else(|| "-".to_owned(), |r| format!("{r:.4}"))
}

fn fmt_change(pct: Option<f64>) -> String {
    pct.map_or_else(|| "new".to_owned(), |p| format!("{:+.1} %", p * 100.0))
}

/// The status column honors the **active gate**: only regressions of a gated
/// kind shout. A threshold exceeded on an ungated kind renders as an `ok`
/// annotation instead — the PR gate compares CI-runner timings against
/// baselines from a different machine class, where the time thresholds are
/// not meaningful (`--gate ratio`; see `docs/development/benchmarking.md`).
fn row_status(row: &DiffRow, gate: crate::Gate) -> String {
    match (
        row.time_regressed && gate.gates_time(),
        row.ratio_regressed && gate.gates_ratio(),
    ) {
        (true, true) => "TIME+RATIO-REGRESSED".to_owned(),
        (true, false) => "TIME-REGRESSED".to_owned(),
        (false, true) => "RATIO-REGRESSED".to_owned(),
        (false, false) => {
            let mut ungated = Vec::new();
            if row.time_regressed && !gate.gates_time() {
                ungated.push("time n/a");
            }
            if row.ratio_regressed && !gate.gates_ratio() {
                ungated.push("ratio n/a");
            }
            if ungated.is_empty() {
                "ok".to_owned()
            } else {
                format!("ok ({}: ungated)", ungated.join(", "))
            }
        }
    }
}

/// A trailing note for the human-readable formats when thresholds were
/// exceeded on kinds the active gate does not hold.
fn ungated_note(rows: &[DiffRow], gate: crate::Gate) -> Option<String> {
    let time = !gate.gates_time() && rows.iter().any(|r| r.time_regressed);
    let ratio = !gate.gates_ratio() && rows.iter().any(|r| r.ratio_regressed);
    let kinds = match (time, ratio) {
        (true, true) => "time and ratio",
        (true, false) => "time",
        (false, true) => "ratio",
        (false, false) => return None,
    };
    Some(format!(
        "note: {kinds} changes are informational only — that gate is not active for this \
         comparison (timing is not comparable across machine classes; \
         see docs/development/benchmarking.md)"
    ))
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

/// Render diff rows (current vs baseline) in the given format. `gate` selects
/// which regression kinds the status column shouts about; the JSON encoding
/// keeps the raw per-kind booleans untouched.
pub fn diffs(rows: &[DiffRow], format: crate::Format, gate: crate::Gate) -> String {
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
                row_status(d, gate),
            ]
        })
        .collect();
    let with_note = |table_text: String, prefix: &str| match ungated_note(rows, gate) {
        Some(note) => format!("{table_text}\n\n{prefix}{note}"),
        None => table_text,
    };
    match format {
        crate::Format::Json => serde_json::to_string_pretty(rows).expect("rows serialize"),
        crate::Format::Both => format!(
            "{}\n{}",
            with_note(table(&header, &cells, TableStyle::Ascii), ""),
            serde_json::to_string_pretty(rows).expect("rows serialize")
        ),
        crate::Format::Ascii => with_note(table(&header, &cells, TableStyle::Ascii), ""),
        crate::Format::Markdown => with_note(table(&header, &cells, TableStyle::Markdown), "> "),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Gate;

    fn row(time_regressed: bool, ratio_regressed: bool) -> DiffRow {
        DiffRow {
            name: "m/n".into(),
            config: "cfg".into(),
            median_ns: 120,
            baseline_median_ns: Some(100),
            time_change_pct: Some(0.2),
            ratio: Some(0.5),
            baseline_ratio: Some(0.4),
            time_regressed,
            ratio_regressed,
        }
    }

    #[test]
    fn ns_rendering_is_unchanged_across_the_unit_boundaries() {
        // `format_into` replaced `format!("{ns} ns")`; the rendered column
        // must be byte-identical, boundaries included.
        assert_eq!(fmt_ns(0), "0 ns");
        assert_eq!(fmt_ns(1), "1 ns");
        assert_eq!(fmt_ns(999), "999 ns");
        assert_eq!(fmt_ns(1_000), "1.00 µs");
        assert_eq!(fmt_ns(999_999), "1000.00 µs");
        assert_eq!(fmt_ns(1_000_000), "1.00 ms");
        assert_eq!(fmt_ns(1_000_000_000), "1.00 s");
    }

    #[test]
    fn status_honors_the_active_gate() {
        assert_eq!(row_status(&row(false, false), Gate::Both), "ok");
        assert_eq!(row_status(&row(true, false), Gate::Both), "TIME-REGRESSED");
        assert_eq!(
            row_status(&row(true, true), Gate::Both),
            "TIME+RATIO-REGRESSED"
        );
        // The ratio-only PR gate renders time thresholds as informational.
        assert_eq!(
            row_status(&row(true, false), Gate::Ratio),
            "ok (time n/a: ungated)"
        );
        assert_eq!(row_status(&row(true, true), Gate::Ratio), "RATIO-REGRESSED");
        assert_eq!(
            row_status(&row(false, true), Gate::Time),
            "ok (ratio n/a: ungated)"
        );
    }

    #[test]
    fn ungated_note_appears_only_when_relevant() {
        assert!(ungated_note(&[row(false, false)], Gate::Ratio).is_none());
        assert!(ungated_note(&[row(true, false)], Gate::Both).is_none());
        let note = ungated_note(&[row(true, false)], Gate::Ratio).expect("note");
        assert!(note.contains("time changes are informational"), "{note}");
    }

    #[test]
    fn markdown_diffs_carry_the_note_json_stays_raw() {
        let rows = [row(true, false)];
        let md = diffs(&rows, crate::Format::Markdown, Gate::Ratio);
        assert!(md.contains("ok (time n/a: ungated)"), "{md}");
        assert!(md.contains("> note:"), "{md}");
        let json = diffs(&rows, crate::Format::Json, Gate::Ratio);
        assert!(json.contains("\"time_regressed\": true"), "{json}");
    }
}
