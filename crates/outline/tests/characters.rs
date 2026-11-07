//! Multi-stroke characters (`tests/data/characters.svg`, skeletons in writing order), judged per
//! character at every weight. Writes the `outline-characters` report.

mod common;

use common::{Outcome, WEIGHTS, evaluate, mean, panel, read};
use scriptoria_testkit::report::Report;

/// Per character and weight: mean IoU of its strokes with their swept regions…
const MIN_MEAN_IOU: f64 = 0.97;
/// …the worst stroke…
const MIN_IOU: f64 = 0.9;
/// …and the mean boundary length ratio (burrs make it grow).
const MAX_MEAN_EDGE_RATIO: f64 = 1.05;

/// Characters known to fail at some weight, with the reason (reported, not failed; the test fails
/// if one starts passing, so the list stays honest).
const KNOWN_ISSUES: &[(&str, &str)] = &[];

#[test]
fn characters_expand_at_every_weight() {
    let groups = read("characters.svg");
    let mut report = Report::new("outline-characters", "outline · characters at several weights");
    report.summary(format!("limits per character and weight: mean IoU ≥ {MIN_MEAN_IOU}, every stroke ≥ {MIN_IOU}, mean edges ≤ ×{MAX_MEAN_EDGE_RATIO}"));
    let mut failures = Vec::new();
    let mut sections: Vec<(String, Vec<_>)> = WEIGHTS.iter().map(|w| (format!("×{w}"), Vec::new())).collect();
    for group in &groups {
        let name = group.id.clone().unwrap_or_default();
        let outcomes: Vec<Outcome> = group.items.iter().map(evaluate).collect();
        let points: usize = outcomes.iter().map(|o| o.points).sum();
        let known = KNOWN_ISSUES.iter().find(|(n, _)| *n == name);
        let mut all_ok = true;
        let mut cells = Vec::new();
        for (k, &w) in WEIGHTS.iter().enumerate() {
            let ious: Vec<f64> = outcomes.iter().map(|o| o.at[k].cmp.iou).collect();
            let edges: Vec<f64> = outcomes.iter().map(|o| o.at[k].cmp.edge_ratio).collect();
            let (avg, worst, edge) = (mean(&ious), ious.iter().copied().fold(1.0, f64::min), mean(&edges));
            let ok = avg >= MIN_MEAN_IOU && worst >= MIN_IOU && edge <= MAX_MEAN_EDGE_RATIO;
            all_ok &= ok;
            if !ok && known.is_none() {
                failures.push(format!("{name} ×{w}: IoU mean {avg:.3}, worst {worst:.3}, edges ×{edge:.3}"));
            }
            cells.push(format!("×{w} {avg:.3}/{worst:.3}/{edge:.2}"));
            let items: Vec<_> = outcomes.iter().map(|o| (o, &o.at[k])).collect();
            sections[k].1.push(panel(&items, format!("{name} ×{w} · IoU mean {avg:.3} · worst {worst:.3} · edges ×{edge:.2}"), !ok));
        }
        if let Some(o) = outcomes.iter().find(|o| !o.compatible) {
            failures.push(format!("{name}: {} has a different structure at different weights", o.id));
        }
        if all_ok && known.is_some() {
            failures.push(format!("{name} passes now: remove it from KNOWN_ISSUES"));
        }
        let note = known.map_or(String::new(), |(_, why)| format!(" · known issue: {why}"));
        let line = format!("{name} · {} strokes · {points} pts · {}{note}", group.items.len(), cells.join("  "));
        eprintln!("{line}");
        report.summary(line);
    }
    report.summary("per weight: IoU mean / worst stroke / mean edge ratio");
    for (heading, panels) in sections {
        report.section(heading, "gray: swept regions · red: outlines and on-curve points · blue: centerlines").panels.extend(panels);
    }
    report.write();
    assert!(failures.is_empty(), "characters out of limits:\n{}", failures.join("\n"));
}
