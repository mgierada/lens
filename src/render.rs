//! Plain-text rendering of a [`Report`], for terminals and quick sanity checks.

use std::collections::HashMap;
use std::fmt::Write;

use crate::diff::LineKind;
use crate::model::{CallCheck, Change, ChangedSymbol, Cluster, Confidence, Report};

const SHORT_SHA: usize = 8;

/// Render the report cluster by cluster.
pub fn render_text(report: &Report, show_diff: bool) -> String {
    let mut out = String::new();
    header(&mut out, report);
    for cluster in &report.clusters {
        render_cluster(&mut out, report, cluster, show_diff);
    }
    if !report.unclustered_files.is_empty() {
        let _ = writeln!(
            out,
            "\nNot analysed: {}",
            report.unclustered_files.join(", ")
        );
    }
    out
}

fn header(out: &mut String, report: &Report) {
    let s = &report.stats;
    let _ = writeln!(
        out,
        "lens {}..{} (merge-base {})",
        report.base.rev,
        report.head.rev,
        short(&report.merge_base)
    );
    let _ = writeln!(
        out,
        "{} files, +{} -{}, {} hunks, {} symbols, {} clusters, {} files indexed",
        s.files, s.additions, s.deletions, s.hunks, s.symbols, s.clusters, s.indexed_files
    );
}

fn render_cluster(out: &mut String, report: &Report, cluster: &Cluster, show_diff: bool) {
    let _ = writeln!(out, "\n== [{}] {} ==", cluster.id + 1, cluster.title);
    let summaries: HashMap<&str, &str> = report
        .commits
        .iter()
        .map(|c| (c.sha.as_str(), c.summary.as_str()))
        .collect();
    for sha in &cluster.commits {
        let summary = summaries.get(sha.as_str()).unwrap_or(&"");
        let _ = writeln!(out, "  commit {} {}", short(sha), summary);
    }
    for &id in &cluster.symbols {
        symbol_line(out, report, &report.symbols[id]);
    }
    impact_lines(out, report, cluster);
    if show_diff {
        for &hunk in &cluster.hunks {
            hunk_lines(out, report, hunk);
        }
    }
}

fn symbol_line(out: &mut String, report: &Report, symbol: &ChangedSymbol) {
    let marker = match symbol.change {
        Change::Added => '+',
        Change::Deleted => '-',
        Change::Modified => '~',
        Change::Moved => '>',
    };
    let range = symbol
        .head
        .or(symbol.base)
        .map_or(String::new(), |r| format!(":{}", r.start));
    let callees: Vec<String> = report
        .edges
        .iter()
        .filter(|e| e.from == symbol.id)
        .map(|e| {
            let low = if e.confidence == Confidence::Low {
                "?"
            } else {
                ""
            };
            let broken = if matches!(e.call_check, Some(CallCheck::Broken { .. })) {
                " ✗"
            } else {
                ""
            };
            format!("{}{low}{broken}", report.symbols[e.to].qualname)
        })
        .collect();
    let _ = write!(
        out,
        "  {marker} {}{range} {} [{:?}]",
        symbol.path, symbol.qualname, symbol.kind
    );
    if let Some(from) = &symbol.moved_from {
        let _ = write!(out, " (from {}:{})", from.path, from.qualname);
    }
    if symbol.hub {
        let _ = write!(out, " (hub, {} refs)", symbol.ref_count);
    }
    if !callees.is_empty() {
        let _ = write!(out, " -> {}", callees.join(", "));
    }
    out.push('\n');
    summary_lines(out, symbol);
}

/// Items listed per summary line before eliding.
const SUMMARY_ITEMS: usize = 8;

fn summary_lines(out: &mut String, symbol: &ChangedSymbol) {
    if let (Change::Added, Some(signature)) = (symbol.change, &symbol.signature) {
        let _ = writeln!(out, "      signature {signature}");
    }
    if let Some(summary) = &symbol.summary {
        let signature: Vec<&str> = summary.signature.iter().map(|c| c.text.as_str()).collect();
        if !signature.is_empty() {
            let _ = writeln!(out, "      signature: {}", signature.join(" · "));
        }
        delta_line(out, "calls", &summary.calls_added, &summary.calls_removed);
        delta_line(out, "uses", &summary.uses_added, &summary.uses_removed);
        delta_line(out, "flow", &summary.flow_added, &summary.flow_removed);
    }
    if symbol.broken_calls > 0 {
        let _ = writeln!(
            out,
            "      ✗ {} call(s) no longer match the signature",
            symbol.broken_calls
        );
    }
}

fn delta_line(out: &mut String, label: &str, added: &[String], removed: &[String]) {
    let items: Vec<String> = added
        .iter()
        .map(|a| format!("+{a}"))
        .chain(removed.iter().map(|r| format!("-{r}")))
        .collect();
    if items.is_empty() {
        return;
    }
    let shown = items
        .iter()
        .take(SUMMARY_ITEMS)
        .cloned()
        .collect::<Vec<String>>()
        .join(", ");
    let more = items.len().saturating_sub(SUMMARY_ITEMS);
    let suffix = if more > 0 {
        format!(" …+{more}")
    } else {
        String::new()
    };
    let _ = writeln!(out, "      {label}: {shown}{suffix}");
}

fn impact_lines(out: &mut String, report: &Report, cluster: &Cluster) {
    if cluster.impact_total == 0 {
        return;
    }
    let _ = writeln!(
        out,
        "  impact: {} unchanged referrer(s)",
        cluster.impact_total
    );
    for impact in &cluster.impact {
        let caller = impact.qualname.as_deref().unwrap_or("<module>");
        let target = &report.symbols[impact.target].qualname;
        let dangling = if impact.dangling { " (dangling)" } else { "" };
        let check = match &impact.call_check {
            Some(CallCheck::Broken { reason }) => format!(" ✗ {reason}"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "    {}:{} {caller} -> {target}{dangling}{check}",
            impact.path, impact.line
        );
    }
    let hidden = cluster.impact_total - cluster.impact.len();
    if hidden > 0 {
        let _ = writeln!(out, "    ... {hidden} more");
    }
}

fn hunk_lines(out: &mut String, report: &Report, id: usize) {
    let hunk = &report.hunks[id];
    let h = &hunk.hunk;
    let _ = writeln!(
        out,
        "  --- {} @@ -{},{} +{},{} @@ {}",
        hunk.path, h.old_start, h.old_lines, h.new_start, h.new_lines, h.header
    );
    for line in &h.lines {
        let prefix = match line.kind {
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::Context => ' ',
        };
        let _ = writeln!(out, "  {prefix}{}", line.text);
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(SHORT_SHA)]
}
