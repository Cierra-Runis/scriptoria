//! Small made-up glyphs: two overlapping strokes whose shape changes with the weight in ways a
//! straight interpolation between the outer masters would miss, so the intermediate masters (and
//! their regions) are needed.

mod common;

use common::*;
use kurbo::BezPath;
use scriptoria_font::{Axis, FontSource, Glyph, build};

const AXIS: Axis = Axis { min: 100.0, default: 400.0, max: 900.0 };
const ACCURACY: f64 = 0.5;

/// A stroke-like contour around a centerline, `w` thick, its sides bowed by `bend`, clockwise
/// (y up): left side forwards, the end, right side backwards, the start.
fn stroke(x0: f64, y0: f64, x1: f64, y1: f64, w: f64, bend: f64) -> BezPath {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = dx.hypot(dy);
    let (tx, ty) = (dx / len, dy / len);
    let (nx, ny) = (-ty * w, tx * w);
    let (bx, by) = (-ty * bend, tx * bend);
    let cap = 1.3 * w;
    let mut p = BezPath::new();
    p.move_to((x0 + nx, y0 + ny));
    p.curve_to((x0 + dx / 3.0 + nx + bx, y0 + dy / 3.0 + ny + by), (x0 + 2.0 * dx / 3.0 + nx + bx, y0 + 2.0 * dy / 3.0 + ny + by), (x1 + nx, y1 + ny));
    p.curve_to((x1 + nx + tx * cap, y1 + ny + ty * cap), (x1 - nx + tx * cap, y1 - ny + ty * cap), (x1 - nx, y1 - ny));
    p.curve_to((x0 + 2.0 * dx / 3.0 - nx + bx, y0 + 2.0 * dy / 3.0 - ny + by), (x0 + dx / 3.0 - nx + bx, y0 + dy / 3.0 - ny + by), (x0 - nx, y0 - ny));
    p.curve_to((x0 - nx - tx * cap, y0 - ny - ty * cap), (x0 + nx - tx * cap, y0 + ny - ty * cap), (x0 + nx, y0 + ny));
    p.close_path();
    p
}

fn glyph_at(w: f64, bend: f64) -> BezPath {
    let mut p = stroke(100.0, 100.0, 500.0, 600.0, w, bend);
    p.extend(stroke(80.0, 400.0, 560.0, 300.0, w * 0.8, -bend));
    p
}

fn source() -> FontSource {
    let glyphs = vec![
        Glyph { name: "a".into(), chars: vec!['a'], advance: 650.0, masters: vec![(100.0, glyph_at(10.0, 0.0)), (400.0, glyph_at(40.0, 60.0)), (900.0, glyph_at(120.0, 0.0))] },
        Glyph {
            name: "b".into(),
            chars: vec!['b', 'B'],
            advance: 700.0,
            masters: vec![
                (100.0, glyph_at(10.0, 0.0)),
                (250.0, glyph_at(20.0, 150.0)),
                (400.0, glyph_at(40.0, 0.0)),
                (650.0, glyph_at(70.0, -120.0)),
                (900.0, glyph_at(120.0, 0.0)),
            ],
        },
    ];
    FontSource {
        family: "Scriptoria Test".into(),
        units_per_em: 1000,
        ascender: 880,
        descender: -120,
        axis: AXIS,
        instances: vec![("Thin".into(), 100.0), ("Regular".into(), 400.0), ("Black".into(), 900.0)],
        glyphs,
        accuracy: ACCURACY,
        tolerance: 0.0,
    }
}

/// The same glyphs on an axis that ends at its default: the outlines as given are the boldest, the
/// others thinner.
fn source_ending_at_the_default() -> FontSource {
    let mut source = source();
    source.axis = Axis { min: 100.0, default: 400.0, max: 400.0 };
    source.instances = vec![("Thin".into(), 100.0), ("Light".into(), 300.0), ("Regular".into(), 400.0)];
    for g in &mut source.glyphs {
        g.masters.retain(|m| m.0 <= 400.0);
    }
    source
}

#[test]
fn outlines_come_back_at_and_between_masters() {
    outlines_come_back(source());
}

#[test]
fn the_default_may_end_the_axis() {
    outlines_come_back(source_ending_at_the_default());
}

fn outlines_come_back(source: FontSource) {
    let bytes = build(&source).expect("build");
    let font = font(&bytes);
    for g in &source.glyphs {
        let id = gid(&font, g.chars[0]);
        for &c in &g.chars {
            assert_eq!(gid(&font, c), id, "{c} maps to {}", g.name);
        }
        assert!(overlap_flag(&font, id), "{} lacks OVERLAP_SIMPLE", g.name);
        let mut weights: Vec<f64> = g.masters.iter().map(|m| m.0).collect();
        weights.extend(g.masters.windows(2).flat_map(|m| [0.75 * m[0].0 + 0.25 * m[1].0, 0.5 * (m[0].0 + m[1].0)]));
        for wght in weights {
            let want = contours(&expected(&g.masters, wght));
            let got = contours(&outline(&font, id, wght));
            assert_eq!(got.len(), want.len(), "{} at {wght}: contour count", g.name);
            for (i, (a, b)) in want.iter().zip(&got).enumerate() {
                // Rounding to font units (half a unit each way, at both masters) on top of the
                // conversion to quadratics.
                let d = distance(a, b);
                assert!(d <= ACCURACY + 1.5, "{} at {wght}, contour {i}: {d:.2} units off", g.name);
                assert!((start(a) - start(b)).hypot() <= 1.5, "{} at {wght}, contour {i}: starts elsewhere", g.name);
                assert_eq!(area(a) < 0.0, area(b) < 0.0, "{} at {wght}, contour {i}: direction changed", g.name);
            }
        }
    }
}

#[test]
fn masters_must_include_the_default() {
    let mut source = source();
    source.glyphs[0].masters.remove(1);
    assert!(build(&source).is_err());
}

#[test]
fn outlines_show_the_points_the_font_gets() {
    use kurbo::BezPath;
    use scriptoria_font::glyf_points;
    // A square of lines, then a contour with one cubic.
    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((0.0, 100.0));
    path.line_to((100.0, 100.0));
    path.line_to((100.0, 0.0));
    path.close_path();
    path.move_to((200.0, 0.0));
    path.curve_to((200.0, 300.0), (500.0, 300.0), (500.0, 0.0));
    path.close_path();
    let contours = glyf_points(&path, 0.5);
    assert_eq!(contours.len(), 2);
    assert_eq!(contours[0], [(0.0, 0.0, true), (0.0, 100.0, true), (100.0, 100.0, true), (100.0, 0.0, true)]);
    // The cubic: off-curve points between its ends, which stay on the curve.
    let curve = &contours[1];
    assert_eq!((curve[0], *curve.last().unwrap()), ((200.0, 0.0, true), (500.0, 0.0, true)));
    assert!(curve[1..curve.len() - 1].iter().all(|p| !p.2) && curve.len() > 3, "{curve:?}");
}
