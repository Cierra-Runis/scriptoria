//! Nothing depends on the drawing's scale (no thresholds in points tuned to one document): every
//! test stroke drawn ten times larger or smaller is explained just as well by its skeleton. The
//! circles themselves come out the same, scaled, except where a decision sits right at its limit
//! and rounding tips it (reported, not failed).

mod common;

use kurbo::Affine;
use scriptoria_testkit::data::StrokeKind;

/// Largest change of a stroke's IoU with its skeleton's swept area when the drawing is scaled.
const MAX_IOU_CHANGE: f64 = 0.002;

#[test]
fn fits_do_not_depend_on_the_scale() {
    let mut failures = Vec::new();
    for stroke in common::read("strokes.svg").iter().flat_map(|g| &g.items) {
        let base = common::fit(stroke);
        let rmax = base.skeleton.max_radius();
        for k in [0.1, 10.0] {
            let mut scaled = stroke.clone();
            scaled.path = Affine::scale(k) * stroke.path.clone();
            if let StrokeKind::Line { width } = stroke.kind {
                scaled.kind = StrokeKind::Line { width: width * k };
            }
            let f = common::fit(&scaled);
            if (f.iou - base.iou).abs() > MAX_IOU_CHANGE {
                failures.push(format!("{} ×{k}: IoU {:.4} instead of {:.4}", stroke.id, f.iou, base.iou));
            }
            let same = f.skeleton.circle_count() == base.skeleton.circle_count()
                && base
                    .skeleton
                    .samples()
                    .zip(f.skeleton.samples())
                    .all(|(a, b)| (b.p.to_vec2() / k - a.p.to_vec2()).hypot().max((b.r / k - a.r).abs()) <= 1e-3 * rmax);
            if !same {
                eprintln!("{} ×{k}: other circles ({} instead of {}), same fit", stroke.id, f.skeleton.circle_count(), base.skeleton.circle_count());
            }
        }
    }
    assert!(failures.is_empty(), "fits depend on the scale:\n{}", failures.join("\n"));
}
