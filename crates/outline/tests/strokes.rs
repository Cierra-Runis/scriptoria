//! Single strokes (`tests/data/strokes.svg`, one skeleton per situation): at every weight the
//! outline must match the region its skeleton sweeps, with the same structure at every weight.
//! Writes the `outline-strokes` report.

mod common;

use common::{WEIGHTS, evaluate, panel, read};
use scriptoria_testkit::report::Report;

/// At every weight: IoU with the swept region at least this…
const MIN_IOU: f64 = 0.95;
/// …Hausdorff distance at most this (× the largest radius at that weight)…
const MAX_HAUSDORFF_R: f64 = 0.5;
/// …and no burrs: boundary at most this much longer than the swept region's.
const MAX_EDGE_RATIO: f64 = 1.15;

/// Strokes known to fail at some weight, with the reason (reported, not failed; the test fails if
/// one starts passing, so the list stays honest).
const KNOWN_ISSUES: &[(&str, &str)] = &[];

#[test]
fn single_strokes_expand_at_every_weight() {
    let groups = read("strokes.svg");
    let mut report = Report::new("outline-strokes", "outline · single strokes at several weights");
    report.summary(format!("limits at every weight: IoU ≥ {MIN_IOU}, Hausdorff ≤ {MAX_HAUSDORFF_R} r, edges ≤ ×{MAX_EDGE_RATIO}; same structure at every weight"));
    let mut failures = Vec::new();
    let mut total_points = 0;
    for d in groups.iter().flat_map(|g| &g.items) {
        let o = evaluate(d);
        total_points += o.points;
        let bad: Vec<String> = o
            .at
            .iter()
            .filter(|e| e.cmp.iou < MIN_IOU || e.hausdorff_r > MAX_HAUSDORFF_R || e.cmp.edge_ratio > MAX_EDGE_RATIO)
            .map(|e| format!("×{}", e.weight))
            .collect();
        let ok = bad.is_empty() && o.compatible;
        let known = KNOWN_ISSUES.iter().find(|(id, _)| *id == d.id);
        let per_weight: Vec<String> =
            o.at.iter().map(|e| format!("×{} {:.3}/{:.2}/{:.2}", e.weight, e.cmp.iou, e.hausdorff_r, e.cmp.edge_ratio)).collect();
        let line = format!(
            "{:<10} {:<20} {} pts · {}{}{}",
            d.id,
            d.tags.join(" "),
            o.points,
            per_weight.join("  "),
            if o.compatible { "" } else { " · STRUCTURE DIFFERS BETWEEN WEIGHTS" },
            known.map_or(String::new(), |(_, why)| format!(" · known issue: {why}"))
        );
        eprintln!("{line}");
        report.summary(line.clone());
        match (ok, known) {
            (false, None) => failures.push(format!("{line} · out of limits at {}", bad.join(" "))),
            (true, Some(_)) => failures.push(format!("{} passes now: remove it from KNOWN_ISSUES", d.id)),
            _ => {}
        }
        let section = report.section(format!("{} · {}", d.id, d.tags.join(" ")), "gray: swept region · red: outline and its on-curve points · blue: centerline");
        for e in &o.at {
            let flagged = e.cmp.iou < MIN_IOU || e.hausdorff_r > MAX_HAUSDORFF_R || e.cmp.edge_ratio > MAX_EDGE_RATIO;
            let caption = format!("×{} · IoU {:.3} · Hausdorff {:.2} r · edges ×{:.2}", e.weight, e.cmp.iou, e.hausdorff_r, e.cmp.edge_ratio);
            section.panels.push(panel(&[(&o, e)], caption, flagged));
        }
    }
    let line = format!("total {total_points} points at ×1 · weights {WEIGHTS:?} (per weight: IoU/Hausdorff r/edge ratio)");
    eprintln!("{line}");
    report.summary(line);
    report.write();
    assert!(failures.is_empty(), "single strokes out of limits:\n{}", failures.join("\n"));
}
