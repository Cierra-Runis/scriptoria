#![allow(dead_code)] // each test binary uses a different part of this module

//! Shared by the data tests: reading `tests/data/`, recovering skeletons, measuring them against
//! the strokes they came from, drawing report panels.

use std::path::PathBuf;

use kurbo::{BezPath, Point, Rect, Shape};
use scriptoria_skeleton::{Skeleton, recover_line, recover_outline, split_segments};
use scriptoria_testkit::data::{self, Group, StrokeKind};
use scriptoria_testkit::metrics::compare_at;
use scriptoria_testkit::report::{Panel, Svg, color};

pub fn read(name: &str) -> Vec<Group<data::Stroke>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name);
    data::read_strokes(&path)
}

pub fn recover(stroke: &data::Stroke) -> Skeleton {
    match stroke.kind {
        StrokeKind::Outline => recover_outline(&stroke.path),
        StrokeKind::Line { width } => recover_line(&stroke.path, width / 2.0),
    }
}

/// The area the stroke covers.
pub fn covered(stroke: &data::Stroke) -> BezPath {
    match stroke.kind {
        StrokeKind::Outline => stroke.path.clone(),
        StrokeKind::Line { width } => scriptoria_testkit::stroked(&stroke.path, width),
    }
}

/// The skeleton in the test-data form (for its swept area).
pub fn as_data(sk: &Skeleton) -> data::Skeleton {
    data::Skeleton {
        id: String::new(),
        tags: Vec::new(),
        source: None,
        pieces: sk
            .pieces
            .iter()
            .map(|p| data::SkeletonPiece { circles: p.samples.iter().map(|s| (s.p, s.r)).collect(), spine: p.spine.clone() })
            .collect(),
    }
}

/// How well a skeleton explains its stroke.
pub struct Fit {
    pub skeleton: Skeleton,
    /// Overlap of the area the skeleton sweeps with the stroke.
    pub iou: f64,
    /// Hausdorff distance between the two, relative to the largest radius.
    pub hausdorff_r: f64,
    /// Largest deviation of a subpath from its own stretch of skeleton, relative to the radius.
    pub worst_deviation_r: f64,
}

pub fn fit(stroke: &data::Stroke) -> Fit {
    let skeleton = recover(stroke);
    let (reference, swept) = (covered(stroke), as_data(&skeleton).swept(1.0));
    // A raster relative to the stroke (a twentieth of its radius, within reason), so the measure
    // does not depend on the drawing's scale either.
    let extent = reference.bounding_box().union(swept.bounding_box());
    let step = (skeleton.max_radius() / 20.0).max(extent.width().max(extent.height()) / 3000.0);
    let cmp = compare_at(&reference, &swept, step);
    let rmax = skeleton.max_radius().max(1e-9);
    let worst_deviation_r = skeleton.quality.deviations.iter().map(|(d, r)| d / r.max(1e-9)).fold(0.0, f64::max);
    Fit { iou: cmp.iou, hausdorff_r: cmp.hausdorff / rmax, worst_deviation_r, skeleton }
}

/// The stroke (gray), its subpaths (thin colors), the skeleton's circles and centerline (blue).
pub fn panel(strokes: &[(&data::Stroke, &Fit)], caption: String, flagged: bool) -> Panel {
    const HUES: [&str; 6] = ["#f59e0b", "#10b981", "#8b5cf6", "#ef4444", "#06b6d4", "#84cc16"];
    let rmax = strokes.iter().map(|(_, f)| f.skeleton.max_radius()).fold(0.0, f64::max);
    let bounds = strokes.iter().map(|(s, _)| covered(s).bounding_box()).reduce(|a, b| a.union(b)).unwrap_or(Rect::ZERO);
    let view = bounds.inflate(rmax + 0.1, rmax + 0.1);
    let side = view.width().max(view.height());
    let mut svg = Svg::new(Rect::from_center_size(view.center(), (side, side)), 420.0);
    for (stroke, fit) in strokes {
        svg.path(&covered(stroke), color::ORIGINAL, "none", 0.0);
        if strokes.len() == 1 && stroke.kind == StrokeKind::Outline {
            for (k, segs) in split_segments(&stroke.path).iter().enumerate() {
                let mut sub = BezPath::new();
                for (i, seg) in segs.iter().enumerate() {
                    if i == 0 {
                        sub.move_to(kurbo::ParamCurve::start(seg));
                    }
                    sub.push(seg.as_path_el());
                }
                svg.path(&sub, "none", HUES[k % HUES.len()], 0.8);
            }
        }
        for s in fit.skeleton.samples() {
            svg.circle(s.p, s.r, color::SPINE, 0.6);
        }
        for piece in &fit.skeleton.pieces {
            let spine: Vec<Point> = piece.densify(8).iter().map(|s| s.p).collect();
            svg.polyline(&spine, color::SPINE, 1.2);
        }
    }
    Panel { caption, svg: svg.render(), flagged }
}

pub fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}
