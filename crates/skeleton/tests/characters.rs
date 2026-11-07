//! Multi-stroke characters (`tests/data/characters.svg`: Chinese, kana, Latin), strokes in writing
//! order. Judged per character. Writes the `skeleton-characters` report.

mod common;

use common::{Fit, fit, mean, panel, read};
use scriptoria_testkit::report::Report;

/// Per character: mean IoU of its strokes' swept areas with the strokes…
const MIN_MEAN_IOU: f64 = 0.97;
/// …and the worst single stroke.
const MIN_IOU: f64 = 0.9;

#[test]
fn characters_are_explained_by_their_skeletons() {
    let groups = read("characters.svg");
    let mut report = Report::new("skeleton-characters", "skeleton · characters");
    report.summary(format!("limits per character: mean IoU ≥ {MIN_MEAN_IOU}, every stroke ≥ {MIN_IOU}"));
    let section_note = "gray: strokes · blue: skeleton circles and centerlines";
    let mut failures = Vec::new();
    let mut panels = Vec::new();
    for group in &groups {
        let name = group.id.clone().unwrap_or_default();
        let fits: Vec<Fit> = group.items.iter().map(fit).collect();
        let ious: Vec<f64> = fits.iter().map(|f| f.iou).collect();
        let (avg, worst) = (mean(&ious), ious.iter().copied().fold(1.0, f64::min));
        let worst_id = group.items.iter().zip(&ious).min_by(|a, b| a.1.total_cmp(b.1)).map(|(s, _)| s.id.clone()).unwrap_or_default();
        let ok = avg >= MIN_MEAN_IOU && worst >= MIN_IOU;
        let line = format!("{name} · {} strokes · IoU mean {avg:.4} · worst {worst:.4} ({worst_id})", group.items.len());
        eprintln!("{line}");
        report.summary(line.clone());
        if !ok {
            failures.push(line);
        }
        let pairs: Vec<_> = group.items.iter().zip(&fits).collect();
        panels.push(panel(&pairs, format!("{name} · IoU mean {avg:.3} · worst {worst:.3} ({worst_id})"), !ok));
    }
    report.section("Characters", section_note).panels.extend(panels);
    report.write();
    assert!(failures.is_empty(), "characters out of limits:\n{}", failures.join("\n"));
}
