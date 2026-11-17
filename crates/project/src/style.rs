//! Making a font's glyphs alike, its writer's style kept: strokes' widths drawn towards the font's
//! typical width.

use crate::{Glyph, GlyphStroke, Project};

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

impl GlyphStroke {
    /// Its typical width: its circles' mean radius at its pen (font units).
    pub fn width(&self) -> Option<f64> {
        let radii: Vec<f64> = self.skeleton.iter().flat_map(|p| p.circles.iter().map(|c| c.1)).collect();
        (!radii.is_empty()).then(|| radii.iter().sum::<f64>() / radii.len() as f64 * self.pen)
    }
}

impl Glyph {
    /// Draws each stroke's width towards `target` (font units), as far as `strength` (0 to 1)
    /// goes: only its pen changes.
    pub fn even_widths(&mut self, target: f64, strength: f64) {
        for s in &mut self.strokes {
            if let Some(w) = s.width().filter(|&w| w > 0.0) {
                s.pen *= (target / w).powf(strength.clamp(0.0, 1.0));
            }
        }
    }
}

impl Project {
    /// The typical width of the strokes of the glyphs written (font units).
    pub fn typical_width(&self) -> Option<f64> {
        median(self.glyphs.iter().flat_map(|g| g.strokes.iter().filter_map(GlyphStroke::width)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Outline, OutlineKind, SkeletonPiece};
    use kurbo::{Affine, BezPath, Point};

    /// A stroke from `a` to `b` (font units), radius `r`, pen 1.
    fn stroke(a: (f64, f64), b: (f64, f64), r: f64) -> GlyphStroke {
        let mut piece = SkeletonPiece { circles: vec![(Point::from(a), r), (Point::from(b), r)], spine: Vec::new() };
        piece.respine();
        GlyphStroke { outline: Outline { kind: OutlineKind::Filled, path: BezPath::new() }, skeleton: vec![piece], placement: Affine::IDENTITY, pen: 1.0, link: None }
    }

    fn glyph(strokes: Vec<GlyphStroke>) -> Glyph {
        Glyph { name: "x".into(), chars: vec!['三'], advance: 1000.0, strokes }
    }

    #[test]
    fn widths_are_evened_out() {
        let mut g = glyph(vec![stroke((0.0, 0.0), (500.0, 0.0), 20.0), stroke((0.0, 100.0), (500.0, 100.0), 80.0)]);
        g.even_widths(40.0, 1.0);
        assert!(g.strokes.iter().all(|s| (s.width().unwrap() - 40.0).abs() < 1e-9));
        // Half way: the geometric middle.
        let mut g = glyph(vec![stroke((0.0, 0.0), (500.0, 0.0), 10.0)]);
        g.even_widths(40.0, 0.5);
        assert!((g.strokes[0].width().unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(g.strokes[0].skeleton[0].circles[0].1, 10.0, "only the pen changes");
    }

    #[test]
    fn the_fonts_typical_width() {
        let mut p = Project::new("x");
        p.add_chars("一二");
        p.glyphs[0].strokes = vec![stroke((0.0, 0.0), (500.0, 50.0), 30.0)];
        p.glyphs[1].strokes = vec![stroke((0.0, 0.0), (500.0, 0.0), 40.0), stroke((0.0, 300.0), (500.0, 300.0), 50.0)];
        assert_eq!(p.typical_width(), Some(40.0));
    }
}
