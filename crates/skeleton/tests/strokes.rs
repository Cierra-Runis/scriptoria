//! Single strokes (`tests/data/strokes.svg`), one or a few per situation: every stroke's skeleton
//! must explain it closely. Writes the `skeleton-strokes` report.

mod common;

use common::{Fit, fit, panel, read};
use scriptoria_testkit::report::Report;

/// Every stroke: the area its skeleton sweeps overlaps the stroke at least this much…
const MIN_IOU: f64 = 0.95;
/// …and stays within this distance of it (× the largest radius).
const MAX_HAUSDORFF_R: f64 = 0.35;

/// Strokes known to fail the limits above, with the reason; the test reports them but does not
/// fail on them (and fails if one starts passing, so the list stays honest).
const KNOWN_ISSUES: &[(&str, &str)] = &[];

#[test]
fn single_strokes_are_explained_by_their_skeletons() {
    let groups = read("strokes.svg");
    let mut report = Report::new("skeleton-strokes", "skeleton · single strokes");
    report.summary(format!("limits: IoU ≥ {MIN_IOU}, Hausdorff ≤ {MAX_HAUSDORFF_R} r"));
    let mut failures = Vec::new();
    let mut panels = Vec::new();
    for stroke in groups.iter().flat_map(|g| &g.items) {
        let f: Fit = fit(stroke);
        let ok = f.iou >= MIN_IOU && f.hausdorff_r <= MAX_HAUSDORFF_R;
        let known = KNOWN_ISSUES.iter().find(|(id, _)| *id == stroke.id);
        let line = format!(
            "{:<10} {:<20} IoU {:.4} · Hausdorff {:.2} r · worst subpath {:.2} r · {} circles{}",
            stroke.id,
            stroke.tags.join(" "),
            f.iou,
            f.hausdorff_r,
            f.worst_deviation_r,
            f.skeleton.circle_count(),
            known.map_or(String::new(), |(_, why)| format!(" · known issue: {why}"))
        );
        eprintln!("{line}");
        report.summary(line.clone());
        match (ok, known) {
            (false, None) => failures.push(line.clone()),
            (true, Some(_)) => failures.push(format!("{} passes now: remove it from KNOWN_ISSUES", stroke.id)),
            _ => {}
        }
        let caption = format!("{} · {}\nIoU {:.3} · Hausdorff {:.2} r", stroke.id, stroke.tags.join(" "), f.iou, f.hausdorff_r);
        panels.push((f.iou, panel(&[(stroke, &f)], caption, !ok)));
    }
    panels.sort_by(|a, b| a.0.total_cmp(&b.0));
    report
        .section("Strokes (worst first)", "gray: stroke · thin colors: its subpaths · blue: skeleton circles and centerline")
        .panels
        .extend(panels.into_iter().map(|(_, p)| p));
    report.write();
    assert!(failures.is_empty(), "single strokes out of limits:\n{}", failures.join("\n"));
}
