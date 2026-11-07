//! Tessellates strokes into triangle meshes once, in page coordinates.
//!
//! gpui's `PathBuilder` tessellates in window coordinates, so every zoom step would redo it. Here
//! each stroke is tessellated once when a page loads; zooming and panning only transform vertices.

use gpui::{Path, Pixels, Point, point, px};
use lyon::math::point as lp;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, LineCap, LineJoin, StrokeOptions,
    StrokeTessellator, StrokeVertex, VertexBuffers,
};
use scriptoria_ink::kurbo::{BezPath, PathEl};
use scriptoria_ink::{Stroke, StrokeShape};

/// Tessellation tolerance (pt): about 0.4 px of error at 20× zoom.
const TOLERANCE: f32 = 0.02;
/// Minimum width of stroked lines (pt), so hairlines stay visible when zoomed out.
const MIN_LINE_WIDTH: f32 = 0.5;

/// A triangle mesh in page coordinates.
pub struct Mesh {
    vertices: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Mesh {
    pub fn tessellate(stroke: &Stroke) -> Option<Self> {
        Self::from_path(&stroke.path, stroke.shape, stroke.rgba[3] == 255)
    }

    /// Tessellates any path. With `opaque`, every subpath is tessellated on its own (see below).
    pub fn from_path(path: &BezPath, shape: StrokeShape, opaque: bool) -> Option<Self> {
        Self::from_path_with_tolerance(path, shape, opaque, TOLERANCE)
    }

    /// As [`Mesh::from_path`], within `tolerance` of the curves (in the path's units).
    pub fn from_path_with_tolerance(path: &BezPath, shape: StrokeShape, opaque: bool, tolerance: f32) -> Option<Self> {
        let mut buf: VertexBuffers<lyon::math::Point, u32> = VertexBuffers::new();
        match shape {
            StrokeShape::Outline { non_zero } => {
                let rule = if non_zero { FillRule::NonZero } else { FillRule::EvenOdd };
                let options = FillOptions::tolerance(tolerance).with_fill_rule(rule);
                let mut tess = FillTessellator::new();
                // Opaque strokes: each subpath (small outline) on its own. Same-color overlaps look
                // identical, and skipping the intersection work avoids a flood of tiny triangles.
                // Translucent strokes (highlighters) must be tessellated whole, or overlaps darken.
                let groups = if opaque { split_subpaths(path) } else { vec![to_lyon(path.elements())] };
                for path in &groups {
                    tess.tessellate_path(path, &options, &mut BuffersBuilder::new(&mut buf, |v: FillVertex| v.position()))
                        .ok()?;
                }
            }
            StrokeShape::Line { width } => {
                let path = to_lyon(path.elements());
                let options = StrokeOptions::tolerance(tolerance)
                    .with_line_width((width as f32).max(MIN_LINE_WIDTH))
                    .with_line_cap(LineCap::Round)
                    .with_line_join(LineJoin::Round);
                StrokeTessellator::new()
                    .tessellate_path(&path, &options, &mut BuffersBuilder::new(&mut buf, |v: StrokeVertex| v.position()))
                    .ok()?;
            }
        }
        if buf.indices.is_empty() {
            return None;
        }
        Some(Self { vertices: buf.vertices.iter().map(|v| [v.x, v.y]).collect(), indices: buf.indices })
    }

    #[cfg(test)]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The mesh in window coordinates: `origin + p * scale`.
    pub fn to_path(&self, origin: Point<Pixels>, scale: f32) -> Path<Pixels> {
        let (ox, oy) = (f32::from(origin.x), f32::from(origin.y));
        let map = |i: u32| {
            let [x, y] = self.vertices[i as usize];
            point(px(ox + x * scale), px(oy + y * scale))
        };
        let mut path = Path::new(map(self.indices[0]));
        // As in gpui's `PathBuilder::build_path`: solid fills use st = (0, 1).
        let st = (point(0., 1.), point(0., 1.), point(0., 1.));
        for tri in self.indices.chunks_exact(3) {
            path.push_triangle((map(tri[0]), map(tri[1]), map(tri[2])), st);
        }
        path
    }
}

fn to_lyon(elements: &[PathEl]) -> lyon::path::Path {
    let mut builder = lyon::path::Path::builder().with_svg();
    let p = |q: scriptoria_ink::kurbo::Point| lp(q.x as f32, q.y as f32);
    for el in elements {
        match *el {
            PathEl::MoveTo(a) => {
                builder.move_to(p(a));
            }
            PathEl::LineTo(a) => {
                builder.line_to(p(a));
            }
            PathEl::QuadTo(c, a) => {
                builder.quadratic_bezier_to(p(c), p(a));
            }
            PathEl::CurveTo(c1, c2, a) => {
                builder.cubic_bezier_to(p(c1), p(c2), p(a));
            }
            PathEl::ClosePath => builder.close(),
        }
    }
    builder.build()
}

/// Splits a path at every `MoveTo`.
fn split_subpaths(path: &BezPath) -> Vec<lyon::path::Path> {
    let els = path.elements();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=els.len() {
        if i == els.len() || matches!(els[i], PathEl::MoveTo(_)) {
            out.push(to_lyon(&els[start..i]));
            start = i;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_ink::kurbo::{Rect, Shape};

    #[test]
    fn opaque_subpaths_are_tessellated_separately() {
        // Two overlapping squares: tessellated whole, the overlap needs extra triangles.
        let mut path = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-3);
        path.extend(Rect::new(0.5, 0.5, 1.5, 1.5).to_path(1e-3));
        let shape = StrokeShape::Outline { non_zero: true };
        let separate = Mesh::from_path(&path, shape, true).unwrap();
        let whole = Mesh::from_path(&path, shape, false).unwrap();
        assert_eq!(separate.triangle_count(), 4);
        assert!(whole.triangle_count() > 4);
    }

    #[test]
    fn lines_get_a_minimum_width() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        let mesh = Mesh::from_path(&path, StrokeShape::Line { width: 0.01 }, true).unwrap();
        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v[1]).collect();
        let spread = ys.iter().copied().fold(f32::MIN, f32::max) - ys.iter().copied().fold(f32::MAX, f32::min);
        assert!((spread - MIN_LINE_WIDTH).abs() < 1e-3, "{spread}");
    }
}
