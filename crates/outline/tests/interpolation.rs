//! A variable font stores the outline at a few weights only (its masters) and makes every weight
//! between them by interpolating each point linearly. For every skeleton in `tests/data/`, this
//! picks the masters with [`masters`] and checks that the outlines interpolated between them fill
//! the stroke about as well as the outlines expanded exactly at those weights (on a raster, which
//! only resolves gross losses on small strokes). It also reports where the interpolation still
//! misses the tolerance on the geometry ([`interpolation_error`]) after [`masters`] has halved
//! the weight range as often as it may: not yet a requirement (see the project plan). Writes the
//! `outline-interpolation` report.

mod common;

use std::collections::BTreeMap;

use common::{mean, read, skeleton};
use kurbo::{BezPath, PathEl, Point, Rect, Shape};
use scriptoria_outline::{PlanParams, WeightParams, expand, interpolation_error, masters, plan};
use scriptoria_testkit::metrics::compare_at;
use scriptoria_testkit::report::{Panel, Report, Svg, color};

/// How far interpolated outlines may stray from the exact ones (see [`masters`]), in units of
/// the plan's precision.
const TOLERANCE: f64 = 2.0;
/// Largest loss of IoU with the stroke's swept region against the exact outline at that weight.
const MAX_IOU_LOSS: f64 = 0.03;

/// Strokes known to lose more ink between masters (reported, not failed; the test fails if one
/// stops losing, so the list stays honest).
const KNOWN_ISSUES: &[(&str, &str)] = &[];

/// The point-by-point linear interpolation of two compatible paths.
fn lerp_path(a: &BezPath, b: &BezPath, t: f64) -> BezPath {
    let l = |p: &Point, q: &Point| p.lerp(*q, t);
    a.elements()
        .iter()
        .zip(b.elements())
        .map(|(x, y)| match (x, y) {
            (PathEl::MoveTo(p), PathEl::MoveTo(q)) => PathEl::MoveTo(l(p, q)),
            (PathEl::LineTo(p), PathEl::LineTo(q)) => PathEl::LineTo(l(p, q)),
            (PathEl::QuadTo(p1, p2), PathEl::QuadTo(q1, q2)) => PathEl::QuadTo(l(p1, q1), l(p2, q2)),
            (PathEl::CurveTo(p1, p2, p3), PathEl::CurveTo(q1, q2, q3)) => PathEl::CurveTo(l(p1, q1), l(p2, q2), l(p3, q3)),
            (PathEl::ClosePath, PathEl::ClosePath) => PathEl::ClosePath,
            _ => panic!("masters are not compatible"),
        })
        .collect()
}

/// One weight between two masters.
struct Probe {
    id: String,
    weight: f64,
    masters: (f64, f64),
    iou_interpolated: f64,
    iou_exact: f64,
    hausdorff_r: f64,
    interpolated: BezPath,
    exact: BezPath,
    reference: BezPath,
}

impl Probe {
    fn loss(&self) -> f64 {
        self.iou_exact - self.iou_interpolated
    }
}

#[test]
fn interpolated_weights_fill_the_strokes() {
    let params = PlanParams::default();
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
    let mut probes: Vec<Probe> = Vec::new();
    // Strokes whose interpolation misses the tolerance somewhere, with the worst miss (pt).
    let mut misses: BTreeMap<String, f64> = BTreeMap::new();
    for file in ["strokes.svg", "characters.svg"] {
        for d in read(file).iter().flat_map(|g| &g.items) {
            let plan = plan(&skeleton(d), &params);
            let weights = masters(&plan, params.weights, TOLERANCE * params.precision);
            *counts.entry(weights.len()).or_default() += 1;
            let rmax = d.max_radius().max(1e-9);
            let outlines: Vec<BezPath> = weights.iter().map(|&w| expand(&plan, &WeightParams::scaled(w))).collect();
            for (k, pair) in weights.windows(2).enumerate() {
                for f in [0.25, 0.5, 0.75] {
                    let weight = pair[0] + (pair[1] - pair[0]) * f;
                    let error = interpolation_error(&plan, pair[0], pair[1], weight);
                    if error > TOLERANCE * params.precision {
                        let worst = misses.entry(d.id.clone()).or_insert(0.0);
                        *worst = worst.max(error);
                    }
                    let interpolated = lerp_path(&outlines[k], &outlines[k + 1], f);
                    let exact = expand(&plan, &WeightParams::scaled(weight));
                    let reference = d.swept(weight);
                    let extent = reference.bounding_box();
                    let step = (rmax * weight / 20.0).max(extent.width().max(extent.height()) / 3000.0);
                    let cmp = compare_at(&reference, &interpolated, step);
                    let iou_exact = compare_at(&reference, &exact, step).iou;
                    let hausdorff_r = cmp.hausdorff / (rmax * weight);
                    probes.push(Probe { id: d.id.clone(), weight, masters: (pair[0], pair[1]), iou_interpolated: cmp.iou, iou_exact, hausdorff_r, interpolated, exact, reference });
                }
            }
        }
    }

    let mut report = Report::new("outline-interpolation", "outline · weights interpolated between masters");
    let strokes: usize = counts.values().sum();
    let per_stroke = counts.iter().map(|(n, c)| n * c).sum::<usize>() as f64 / strokes as f64;
    let masters_line = format!("masters within {TOLERANCE}× the precision ({} pt): {per_stroke:.1} per stroke on average · strokes by count {counts:?}", params.precision);
    let fill_line = format!(
        "{} weights between masters: IoU interpolated {:.4}, exact {:.4} · largest loss {:.4} · Hausdorff at most {:.3} r",
        probes.len(),
        mean(&probes.iter().map(|p| p.iou_interpolated).collect::<Vec<_>>()),
        mean(&probes.iter().map(|p| p.iou_exact).collect::<Vec<_>>()),
        probes.iter().map(Probe::loss).fold(0.0, f64::max),
        probes.iter().map(|p| p.hausdorff_r).fold(0.0, f64::max),
    );
    eprintln!("{masters_line}\n{fill_line}");
    report.summary(masters_line);
    report.summary(fill_line);

    let worst_miss = misses.values().copied().fold(0.0, f64::max);
    let miss_line = format!(
        "still missing the tolerance where the range is halved as far as it goes: {} of {strokes} strokes, by up to {worst_miss:.4} pt ({})",
        misses.len(),
        misses.iter().map(|(id, e)| format!("{id} {e:.4}")).collect::<Vec<_>>().join(", ")
    );
    eprintln!("{miss_line}");
    report.summary(miss_line);

    let mut failures = Vec::new();
    let mut known_seen: Vec<&str> = Vec::new();
    for p in probes.iter().filter(|p| p.loss() > MAX_IOU_LOSS) {
        match KNOWN_ISSUES.iter().find(|(id, _)| *id == p.id) {
            Some((id, _)) => known_seen.push(id),
            None => failures.push(format!("{} ×{:.3} (masters ×{:.3}, ×{:.3}): IoU {:.4} instead of {:.4}", p.id, p.weight, p.masters.0, p.masters.1, p.iou_interpolated, p.iou_exact)),
        }
    }
    for (id, why) in KNOWN_ISSUES {
        if known_seen.contains(id) {
            report.summary(format!("{id} · known issue: {why}"));
        } else {
            failures.push(format!("{id} passes now: remove it from KNOWN_ISSUES"));
        }
    }

    let mut worst: Vec<&Probe> = probes.iter().collect();
    worst.sort_by(|a, b| b.loss().total_cmp(&a.loss()));
    let section = report.section("largest losses", "gray: swept region · blue: exact outline · red: outline interpolated between the masters");
    for p in worst.iter().take(24) {
        let caption = format!(
            "{} ×{:.3} between ×{:.3} and ×{:.3} · IoU {:.4} (exact {:.4}) · Hausdorff {:.3} r",
            p.id, p.weight, p.masters.0, p.masters.1, p.iou_interpolated, p.iou_exact, p.hausdorff_r
        );
        section.panels.push(panel(p, caption));
    }
    report.write();
    assert!(failures.is_empty(), "interpolated weights lose ink:\n{}", failures.join("\n"));
}

fn panel(p: &Probe, caption: String) -> Panel {
    let bounds: Rect = p.reference.bounding_box().union(p.interpolated.bounding_box()).inflate(0.1, 0.1);
    let side = bounds.width().max(bounds.height());
    let mut svg = Svg::new(Rect::from_center_size(bounds.center(), (side, side)), 420.0);
    svg.path(&p.reference, color::ORIGINAL, "none", 0.0);
    svg.path(&p.exact, "none", color::SPINE, 1.0);
    svg.path(&p.interpolated, "none", color::REGENERATED, 1.0);
    Panel { caption, svg: svg.render(), flagged: p.loss() > MAX_IOU_LOSS }
}
