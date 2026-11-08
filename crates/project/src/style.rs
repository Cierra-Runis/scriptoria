//! Making a font's glyphs alike, its writer's style kept: strokes' widths drawn towards the font's
//! typical width, and glyphs sheared so that their horizontal strokes slope as the font's
//! typically do.

use kurbo::{Affine, Vec2};

use crate::{Glyph, GlyphStroke, Project};

/// How far from level a stroke may run and still count as horizontal (radians).
const LEVEL: f64 = 25.0 * std::f64::consts::PI / 180.0;

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

    /// From its first circle to its last, placed (font units).
    fn course(&self) -> Option<Vec2> {
        let first = self.skeleton.first()?.circles.first()?.0;
        let last = self.skeleton.last()?.circles.last()?.0;
        Some(self.placement * last - self.placement * first)
    }

    /// Its slope (radians, up to the right positive) if it is a horizontal stroke at least
    /// `min_length` long, written left to right.
    fn horizontal_slope(&self, min_length: f64) -> Option<f64> {
        let d = self.course()?;
        let angle = d.y.atan2(d.x);
        (d.hypot() >= min_length && angle.abs() <= LEVEL).then_some(angle)
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

    /// The slope of its horizontal strokes: the median of those at least `min_length` long
    /// (radians); none without any.
    pub fn horizontal_slope(&self, min_length: f64) -> Option<f64> {
        median(self.strokes.iter().filter_map(|s| s.horizontal_slope(min_length)).collect())
    }

    /// Shears the glyph up and down about the middle of its ink (its upright strokes stay upright)
    /// so that its horizontal strokes slope at `target`; false (nothing changed) if it has none.
    pub fn level_to(&mut self, target: f64, min_length: f64) -> bool {
        let (Some(slope), Some(ink)) = (self.horizontal_slope(min_length), self.ink_bounds()) else { return false };
        let k = target.tan() - slope.tan();
        let c = ink.center().to_vec2();
        let shear = Affine::translate(c) * Affine::new([1.0, k, 0.0, 1.0, 0.0, 0.0]) * Affine::translate(-c);
        let all: Vec<usize> = (0..self.strokes.len()).collect();
        self.transform_strokes(&all, shear);
        true
    }
}

impl Project {
    /// The typical width of the strokes of the glyphs written (font units).
    pub fn typical_width(&self) -> Option<f64> {
        median(self.glyphs.iter().flat_map(|g| g.strokes.iter().filter_map(GlyphStroke::width)).collect())
    }

    /// The typical slope of the glyphs' horizontal strokes (radians): the median over the glyphs
    /// that have some at least `min_length` long.
    pub fn typical_slope(&self, min_length: f64) -> Option<f64> {
        median(self.glyphs.iter().filter_map(|g| g.horizontal_slope(min_length)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Outline, OutlineKind, SkeletonPiece};
    use kurbo::{BezPath, Point};

    /// A stroke from `a` to `b` (font units), radius `r`, pen 1.
    fn stroke(a: (f64, f64), b: (f64, f64), r: f64) -> GlyphStroke {
        let mut piece = SkeletonPiece { circles: vec![(Point::from(a), r), (Point::from(b), r)], spine: Vec::new() };
        piece.respine();
        GlyphStroke { outline: Outline { kind: OutlineKind::Filled, path: BezPath::new() }, skeleton: vec![piece], placement: Affine::IDENTITY, pen: 1.0 }
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
    fn horizontals_are_levelled_and_uprights_stay() {
        // 三 written rising 10° to the right, with an upright stroke.
        let rise = 10f64.to_radians();
        let h = |y: f64| stroke((100.0, y), (100.0 + 600.0 * rise.cos(), y + 600.0 * rise.sin()), 20.0);
        let mut g = glyph(vec![h(100.0), h(400.0), h(700.0), stroke((400.0, 0.0), (400.0, 800.0), 20.0)]);
        assert!((g.horizontal_slope(200.0).unwrap() - rise).abs() < 1e-9);
        assert!(g.level_to(0.0, 200.0));
        assert!(g.horizontal_slope(200.0).unwrap().abs() < 1e-9);
        let upright = g.strokes[3].course().unwrap();
        assert!(upright.x.abs() < 1e-9, "{upright:?}");
        // Short strokes and steep ones are not horizontal; without horizontals nothing changes.
        let mut dots = glyph(vec![stroke((0.0, 0.0), (50.0, 5.0), 20.0), stroke((0.0, 0.0), (100.0, 300.0), 20.0)]);
        assert_eq!(dots.horizontal_slope(200.0), None);
        assert!(!dots.level_to(0.0, 200.0));
    }

    #[test]
    fn the_fonts_typical_width_and_slope() {
        let mut p = Project::new("x");
        p.add_chars("一二");
        p.glyphs[0].strokes = vec![stroke((0.0, 0.0), (500.0, 50.0), 30.0)];
        p.glyphs[1].strokes = vec![stroke((0.0, 0.0), (500.0, 0.0), 40.0), stroke((0.0, 300.0), (500.0, 300.0), 50.0)];
        assert_eq!(p.typical_width(), Some(40.0));
        let slope = p.typical_slope(200.0).unwrap();
        assert!(slope == 0.0 || (slope - 0.1f64.atan()).abs() < 1e-9, "{slope}");
    }
}
