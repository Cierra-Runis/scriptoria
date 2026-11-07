#![allow(dead_code)] // each test binary uses a different part of this module

//! Shared by the data tests: reading skeletons from `tests/data/`, expanding them at several
//! weights, measuring each outline against the exact region its skeleton sweeps at that weight.

use std::path::PathBuf;

use kurbo::{BezPath, PathEl, Rect, Shape};
use scriptoria_outline::{Plan, PlanParams, WeightParams, expand, plan};
use scriptoria_skeleton::{Piece, Sample, Skeleton};
use scriptoria_testkit::data::{self, Group};
use scriptoria_testkit::metrics::{Comparison, compare_at};
use scriptoria_testkit::report::{Panel, Svg, color};

/// Weights every test checks (radius scale; ×1 is the writer's own).
pub const WEIGHTS: [f64; 5] = [0.3, 0.5, 1.0, 2.0, 3.0];

pub fn read(name: &str) -> Vec<Group<data::Skeleton>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    data::read_skeletons(&path)
}

pub fn skeleton(d: &data::Skeleton) -> Skeleton {
    Skeleton {
        pieces: d
            .pieces
            .iter()
            .map(|p| Piece { samples: p.circles.iter().map(|&(p, r)| Sample { p, r }).collect(), spine: p.spine.clone() })
            .collect(),
        quality: Default::default(),
    }
}

/// One skeleton expanded at one weight, against its swept region.
pub struct Expanded {
    pub weight: f64,
    pub outline: BezPath,
    pub reference: BezPath,
    pub cmp: Comparison,
    /// Hausdorff distance relative to the largest radius at this weight.
    pub hausdorff_r: f64,
}

pub struct Outcome {
    pub id: String,
    pub plan: Plan,
    pub at: Vec<Expanded>,
    /// The sequence of path commands is the same at every weight (compatible masters).
    pub compatible: bool,
    /// Points of the outline at ×1 (on- and off-curve).
    pub points: usize,
}

pub fn evaluate(d: &data::Skeleton) -> Outcome {
    let sk = skeleton(d);
    let plan = plan(&sk, &PlanParams::default());
    let rmax = d.max_radius().max(1e-9);
    let at: Vec<Expanded> = WEIGHTS
        .iter()
        .map(|&weight| {
            let outline = expand(&plan, &WeightParams::scaled(weight));
            let reference = d.swept(weight);
            // Raster fine enough to see a twentieth of the radius at every weight, within reason.
            let extent = reference.bounding_box().union(outline.bounding_box());
            let step = (rmax * weight / 20.0).max(extent.width().max(extent.height()) / 3000.0);
            let cmp = compare_at(&reference, &outline, step);
            let hausdorff_r = cmp.hausdorff / (rmax * weight);
            Expanded { weight, outline, reference, cmp, hausdorff_r }
        })
        .collect();
    let kinds = |p: &BezPath| p.elements().iter().map(std::mem::discriminant).collect::<Vec<_>>();
    let compatible = at.windows(2).all(|w| kinds(&w[0].outline) == kinds(&w[1].outline));
    let points = at.iter().find(|e| e.weight == 1.0).map_or(0, |e| points(&e.outline));
    Outcome { id: d.id.clone(), plan, at, compatible, points }
}

pub fn points(path: &BezPath) -> usize {
    path.elements()
        .iter()
        .map(|e| match e {
            PathEl::MoveTo(_) | PathEl::LineTo(_) => 1,
            PathEl::QuadTo(..) => 2,
            PathEl::CurveTo(..) => 3,
            PathEl::ClosePath => 0,
        })
        .sum()
}

/// The swept region (gray), the outline (red) with its on-curve points, the centerline (blue).
pub fn panel(items: &[(&Outcome, &Expanded)], caption: String, flagged: bool) -> Panel {
    let bounds = items.iter().map(|(_, e)| e.reference.bounding_box().union(e.outline.bounding_box())).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    let view = bounds.inflate(0.1, 0.1);
    let side = view.width().max(view.height());
    let mut svg = Svg::new(Rect::from_center_size(view.center(), (side, side)), 420.0);
    for (o, e) in items {
        svg.path(&e.reference, color::ORIGINAL, "none", 0.0);
        svg.path(&e.outline, color::REGENERATED_FILL, color::REGENERATED, 1.0);
        for el in e.outline.elements() {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::CurveTo(_, _, p) = el {
                svg.dot(*p, 1.5, color::REGENERATED);
            }
        }
        for piece in &o.plan.pieces {
            svg.polyline(&piece.dense.iter().map(|d| d.p).collect::<Vec<_>>(), color::SPINE, 0.8);
        }
    }
    Panel { caption, svg: svg.render(), flagged }
}

pub fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}
