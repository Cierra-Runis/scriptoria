//! The whole pipeline on the project's sample document `test/test.pdf`: PDF → strokes → skeletons
//! → outlines at weight ×1, against the original strokes.
//!
//! Strokes in the handwriting regions declared below are held to the limits; strokes elsewhere
//! (drawings, scribbles) only have to come out roughly right. Page 2 is where the methods were
//! developed; pages 0 and 1 were added later and show whether they carry over.
//! Writes the `pipeline` report.

use scriptoria_ink::kurbo::Rect;
use scriptoria_ink::{InkDocument, InkPage, StrokeShape};
use scriptoria_outline::{PlanParams, WeightParams, expand, plan};
use scriptoria_testkit::metrics::compare;
use scriptoria_testkit::report::{Panel, Report, Svg, color};

/// Handwriting regions per page (page points, y down); `None`: the whole page.
const TEXT: &[(usize, Option<[f64; 4]>)] = &[
    (0, Some([60.0, 4.0, 100.0, 27.0])),    // A–Z
    (0, Some([177.0, 100.0, 357.0, 205.0])), // notes on matrices
    (0, Some([397.0, 299.0, 531.0, 362.0])), // notes on functions
    (1, None),                               // code written by hand
    (2, Some([93.0, 59.0, 137.0, 67.0])),    // name labels under the faces
    (2, Some([190.0, 130.0, 320.0, 235.0])), // Chinese, kana, English
];

/// Handwriting, per page: mean IoU of the regenerated outlines with the strokes (measured
/// 2026-11-07: 0.9881, 0.9892, 0.9874 on pages 0, 1, 2)…
const MIN_MEAN_IOU: f64 = 0.985;
/// …and the 10th percentile (measured: 0.9795, 0.9829, 0.9786).
const MIN_P10_IOU: f64 = 0.975;
/// Everything else (drawings): only the mean.
const MIN_OTHER_MEAN_IOU: f64 = 0.95;

fn is_text(page: usize, r: Rect) -> bool {
    TEXT.iter().any(|&(p, rect)| p == page && rect.is_none_or(|[x0, y0, x1, y1]| Rect::new(x0, y0, x1, y1).contains(r.center())))
}

fn percentile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len().max(1) - 1) as f64 * q).floor() as usize).copied().unwrap_or(f64::NAN)
}

#[test]
fn pdf_reading_basics() {
    let doc = open();
    assert!(doc.page_count() >= 3);
    for i in 0..doc.page_count() {
        let page = doc.page(i).unwrap();
        assert!(page.paper.is_some(), "page {i}: paper fill");
        assert!(page.strokes.len() > 100, "page {i}: {} strokes", page.strokes.len());
        assert!(page.strokes.iter().enumerate().all(|(k, s)| s.order == k), "page {i}: writing order");
        assert!(page.strokes.iter().all(|s| s.bbox.width() >= 0.0 && s.bbox.height() >= 0.0));
    }
    // The monoline pen's strokes come as constant-width lines.
    let lines = doc.page(2).unwrap().strokes.iter().filter(|s| matches!(s.shape, StrokeShape::Line { .. })).count();
    assert!(lines >= 20, "{lines} lines on page 2");
}

#[test]
fn handwriting_regenerates_close_to_the_original() {
    let doc = open();
    let mut report = Report::new("pipeline", "pipeline · test.pdf · regenerated outlines (×1) against the strokes");
    report.summary(format!("limits per page, handwriting: mean IoU ≥ {MIN_MEAN_IOU}, p10 ≥ {MIN_P10_IOU}; elsewhere: mean ≥ {MIN_OTHER_MEAN_IOU}"));
    let mut failures = Vec::new();
    for page_index in 0..doc.page_count() {
        let page = doc.page(page_index).unwrap();
        let scores = scriptoria::par_map(&page.strokes, |stroke| {
            let outline = expand(&plan(&scriptoria::skeleton(stroke), &PlanParams::default()), &WeightParams::scaled(1.0));
            compare(&scriptoria::covered_area(stroke), &outline).iou
        });
        let (mut text, mut other): (Vec<(usize, f64)>, Vec<(usize, f64)>) =
            scores.iter().copied().enumerate().partition(|&(i, _)| is_text(page_index, page.strokes[i].bbox));
        let mean = |v: &[(usize, f64)]| v.iter().map(|(_, x)| x).sum::<f64>() / v.len().max(1) as f64;
        let mut text_iou: Vec<f64> = text.iter().map(|(_, x)| *x).collect();
        let (text_mean, other_mean, p10) = (mean(&text), mean(&other), percentile(&mut text_iou, 0.1));
        let role = if page_index == 2 { "development page" } else { "held-out page" };
        let line = format!(
            "page {page_index} ({role}): handwriting {} strokes, IoU mean {text_mean:.4}, p10 {p10:.4} · elsewhere {} strokes{}",
            text.len(),
            other.len(),
            if other.is_empty() { String::new() } else { format!(", mean {other_mean:.4}") }
        );
        eprintln!("{line}");
        report.summary(line.clone());
        if !text.is_empty() && (text_mean < MIN_MEAN_IOU || p10 < MIN_P10_IOU) || !other.is_empty() && other_mean < MIN_OTHER_MEAN_IOU {
            failures.push(line);
        }
        // The worst handwriting strokes, for a look.
        text.sort_by(|a, b| a.1.total_cmp(&b.1));
        other.clear();
        let section = report.section(format!("page {page_index}: worst handwriting strokes"), "gray: original · red: regenerated outline (×1)");
        for &(i, iou) in text.iter().take(12) {
            section.panels.push(panel(&page, i, iou));
        }
    }
    report.write();
    assert!(failures.is_empty(), "pages out of limits:\n{}", failures.join("\n"));
}

fn open() -> InkDocument {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test/test.pdf");
    InkDocument::open(std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))).unwrap()
}

fn panel(page: &InkPage, i: usize, iou: f64) -> Panel {
    let stroke = &page.strokes[i];
    let sk = scriptoria::skeleton(stroke);
    let outline = expand(&plan(&sk, &PlanParams::default()), &WeightParams::scaled(1.0));
    let pad = sk.max_radius() + 0.1;
    let view = stroke.bbox.inflate(pad, pad);
    let side = view.width().max(view.height());
    let mut svg = Svg::new(Rect::from_center_size(view.center(), (side, side)), 300.0);
    svg.path(&scriptoria::covered_area(stroke), color::ORIGINAL, "none", 0.0);
    svg.path(&outline, color::REGENERATED_FILL, color::REGENERATED, 1.0);
    svg.dot(stroke.start, 3.0, color::START);
    Panel { caption: format!("#{i} · IoU {iou:.3}"), svg: svg.render(), flagged: true }
}
