#![allow(dead_code)] // each test binary uses a different part of this module

//! Shared by the tests: reading a built font back with skrifa, comparing outlines.

use kurbo::{BezPath, PathEl, Point, Shape};
use skrifa::instance::Size;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::raw::tables::glyf::Glyph as RawGlyph;
use skrifa::{FontRef, GlyphId, MetadataProvider};

/// Collects a drawn glyph as a kurbo path.
#[derive(Default)]
struct Pen(BezPath);

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((x as f64, y as f64));
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.0.quad_to((cx0 as f64, cy0 as f64), (x as f64, y as f64));
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.0.curve_to((cx0 as f64, cy0 as f64), (cx1 as f64, cy1 as f64), (x as f64, y as f64));
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// The outline of a glyph at a weight, in font units.
pub fn outline(font: &FontRef, gid: GlyphId, wght: f64) -> BezPath {
    let location = font.axes().location([("wght", wght as f32)]);
    let glyph = font.outline_glyphs().get(gid).expect("glyph");
    let mut pen = Pen::default();
    glyph.draw(DrawSettings::unhinted(Size::unscaled(), &location), &mut pen).expect("draw");
    pen.0
}

/// Whether the glyph's OVERLAP_SIMPLE flag is set.
pub fn overlap_flag(font: &FontRef, gid: GlyphId) -> bool {
    let (loca, glyf) = (font.loca(None).unwrap(), font.glyf().unwrap());
    match loca.get_glyf(gid, &glyf).unwrap() {
        Some(RawGlyph::Simple(g)) => g.has_overlapping_contours(),
        _ => false,
    }
}

/// The contours of a path, each as its own path.
pub fn contours(path: &BezPath) -> Vec<BezPath> {
    let mut out: Vec<BezPath> = Vec::new();
    for el in path.elements() {
        if let PathEl::MoveTo(_) = el {
            out.push(BezPath::new());
        }
        out.last_mut().unwrap().push(*el);
    }
    out
}

pub fn start(contour: &BezPath) -> Point {
    match contour.elements()[0] {
        PathEl::MoveTo(p) => p,
        _ => panic!("contour without a move"),
    }
}

/// Signed area (kurbo's sign: negative for clockwise with y up).
pub fn area(contour: &BezPath) -> f64 {
    contour.area()
}

/// Largest distance between two contours' curves, either way (both flattened finely).
pub fn distance(a: &BezPath, b: &BezPath) -> f64 {
    let flat = |p: &BezPath| {
        let mut pts = Vec::new();
        kurbo::flatten(p, 0.02, |el| match el {
            PathEl::MoveTo(q) | PathEl::LineTo(q) => pts.push(q),
            _ => {}
        });
        pts
    };
    let (fa, fb) = (flat(a), flat(b));
    let directed = |from: &[Point], to: &[Point]| {
        from.iter()
            .map(|&p| {
                to.windows(2)
                    .map(|s| {
                        let d = s[1] - s[0];
                        let t = if d.hypot2() > 0.0 { ((p - s[0]).dot(d) / d.hypot2()).clamp(0.0, 1.0) } else { 0.0 };
                        (p - s[0].lerp(s[1], t)).hypot()
                    })
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    };
    directed(&fa, &fb).max(directed(&fb, &fa))
}

/// The point-by-point linear interpolation of two compatible paths.
pub fn lerp_path(a: &BezPath, b: &BezPath, t: f64) -> BezPath {
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
            _ => panic!("paths are not compatible"),
        })
        .collect()
}

/// The outline a glyph's masters give at a weight: the two masters around it, interpolated.
pub fn expected(masters: &[(f64, BezPath)], wght: f64) -> BezPath {
    let k = masters.windows(2).position(|m| wght >= m[0].0 && wght <= m[1].0).expect("weight within the masters");
    let (a, b) = (&masters[k], &masters[k + 1]);
    lerp_path(&a.1, &b.1, (wght - a.0) / (b.0 - a.0))
}

pub fn font(bytes: &[u8]) -> FontRef<'_> {
    FontRef::new(bytes).expect("font")
}

pub fn gid(font: &FontRef, c: char) -> GlyphId {
    font.charmap().map(c).expect("mapped")
}
