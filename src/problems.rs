//! Problems a glyph may have (as FontForge's Find Problems, for fonts made of strokes): things
//! that make it come out wrong in the font, or out of keeping with the others. Each is a guess to
//! look at, not a rule.

use scriptoria_ink::kurbo::Rect;
use scriptoria_project::{FontInfo, Glyph, fingerprint};

/// A problem of a glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Problem {
    /// A stroke has no skeleton: the font gets nothing for it.
    NoSkeleton,
    /// The same stroke twice, in the same place.
    Duplicate,
    /// Ink well beyond the em (above the ascender, below the descender, or beyond the sides).
    OutsideEm,
    /// No advance, or more than two ems.
    OddAdvance,
    /// A stroke much thicker or thinner than the glyph's others.
    UnevenWeight,
    /// A stroke with very many circles in its skeleton (slow, and more points in the font).
    ManyCircles,
    /// Much larger or smaller than the reference font's character.
    OffReference,
}

impl Problem {
    pub const ALL: [Problem; 7] =
        [Problem::NoSkeleton, Problem::Duplicate, Problem::OutsideEm, Problem::OddAdvance, Problem::UnevenWeight, Problem::ManyCircles, Problem::OffReference];

    pub fn name(self) -> &'static str {
        match self {
            Problem::NoSkeleton => "笔画没有骨架",
            Problem::Duplicate => "重复的笔画",
            Problem::OutsideEm => "超出字身框",
            Problem::OddAdvance => "字宽异常",
            Problem::UnevenWeight => "笔画粗细不一",
            Problem::ManyCircles => "骨架圆太多",
            Problem::OffReference => "大小与参考字差得多",
        }
    }

    /// What it means and what to do.
    pub fn advice(self) -> &'static str {
        match self {
            Problem::NoSkeleton => "导出时这一笔没有轮廓：重新分配这一笔",
            Problem::Duplicate => "同一笔在同一处有两份（多粘贴了一次？）：删掉一份",
            Problem::OutsideEm => "墨迹超出字身框一成半以上：缩小或移进来",
            Problem::OddAdvance => "字宽为 0 或超过两个 em：在单字视图里改字宽",
            Problem::UnevenWeight => "有一笔比别的笔粗或细六成以上：在单字视图里用“变细 / 变粗”",
            Problem::ManyCircles => "某一笔骨架上的圆超过 120 个：在“改骨架”里精简",
            Problem::OffReference => "墨迹比参考字体的这个字大或小三成以上：按参考摆放，或手动缩放",
        }
    }
}

/// How far ink may reach beyond the em, as a share of it.
const OUTSIDE: f64 = 0.15;
/// How much thicker (or thinner) than the glyph's typical stroke a stroke may be.
const UNEVEN: f64 = 1.6;
/// How many circles a stroke's skeleton may have.
const MANY_CIRCLES: usize = 120;
/// How much larger (or smaller) than the reference's character a glyph may be, by the square root
/// of the areas of their boxes.
const OFF_REFERENCE: f64 = 1.3;

/// A glyph's problems, given the box of its character in the reference font (font units), if
/// any.
pub fn check(glyph: &Glyph, info: &FontInfo, reference: Option<Rect>) -> Vec<Problem> {
    let mut found = Vec::new();
    let upm = info.units_per_em as f64;
    if glyph.advance <= 0.0 || glyph.advance > 2.0 * upm {
        found.push(Problem::OddAdvance);
    }
    if glyph.strokes.is_empty() {
        return found;
    }
    if glyph.strokes.iter().any(|s| s.skeleton.iter().all(|p| p.circles.is_empty())) {
        found.push(Problem::NoSkeleton);
    }
    let mut seen = std::collections::HashSet::new();
    if glyph.strokes.iter().any(|s| !seen.insert((fingerprint(&s.outline.path), s.placement.as_coeffs().map(f64::to_bits)))) {
        found.push(Problem::Duplicate);
    }
    if let Some(ink) = glyph.ink_bounds() {
        let margin = OUTSIDE * (info.ascender - info.descender) as f64;
        if ink.y1 > info.ascender as f64 + margin || ink.y0 < info.descender as f64 - margin || ink.x0 < -margin || ink.x1 > glyph.advance + margin {
            found.push(Problem::OutsideEm);
        }
        if let Some(r) = reference.filter(|r| r.area() > 0.0 && ink.area() > 0.0) {
            let k = (ink.area() / r.area()).sqrt();
            if !(1.0 / OFF_REFERENCE..=OFF_REFERENCE).contains(&k) {
                found.push(Problem::OffReference);
            }
        }
    }
    // Each stroke's typical width: its mean radius at its pen.
    let widths: Vec<f64> = glyph
        .strokes
        .iter()
        .filter_map(|s| {
            let radii: Vec<f64> = s.skeleton.iter().flat_map(|p| p.circles.iter().map(|c| c.1)).collect();
            (!radii.is_empty()).then(|| radii.iter().sum::<f64>() / radii.len() as f64 * s.pen)
        })
        .collect();
    if widths.len() >= 2 {
        let mut sorted = widths.clone();
        sorted.sort_by(f64::total_cmp);
        let typical = sorted[sorted.len() / 2];
        if widths.iter().any(|&w| w > typical * UNEVEN || w < typical / UNEVEN) {
            found.push(Problem::UnevenWeight);
        }
    }
    if glyph.strokes.iter().any(|s| s.skeleton.iter().map(|p| p.circles.len()).sum::<usize>() > MANY_CIRCLES) {
        found.push(Problem::ManyCircles);
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_ink::kurbo::{Affine, BezPath, Point};
    use scriptoria_project::{GlyphStroke, Outline, OutlineKind, SkeletonPiece};

    /// A stroke from (x0, y) to (x1, y) in font units, radius `r`, `n` circles.
    fn stroke(x0: f64, x1: f64, y: f64, r: f64, n: usize) -> GlyphStroke {
        let circles = (0..n).map(|i| (Point::new(x0 + (x1 - x0) * i as f64 / (n - 1).max(1) as f64, y), r)).collect();
        let mut piece = SkeletonPiece { circles, spine: Vec::new() };
        piece.respine();
        let mut path = BezPath::new();
        path.move_to((x0, y));
        path.line_to((x1, y + r));
        GlyphStroke { outline: Outline { kind: OutlineKind::Filled, path }, skeleton: vec![piece], placement: Affine::IDENTITY, pen: 1.0 }
    }

    fn glyph(strokes: Vec<GlyphStroke>) -> Glyph {
        Glyph { name: "x".into(), chars: vec!['口'], advance: 1000.0, strokes }
    }

    #[test]
    fn good_glyphs_have_no_problems() {
        let info = FontInfo::default();
        let g = glyph(vec![stroke(100.0, 900.0, 700.0, 30.0, 5), stroke(100.0, 900.0, 0.0, 34.0, 5)]);
        assert_eq!(check(&g, &info, Some(Rect::new(100.0, -20.0, 900.0, 730.0))), []);
        assert_eq!(check(&glyph(Vec::new()), &info, None), []);
    }

    #[test]
    fn problems_are_found() {
        let info = FontInfo::default();
        let ok = || stroke(100.0, 900.0, 300.0, 30.0, 5);
        // Pasted twice; much thicker; beyond the em; very many circles.
        let twice = glyph(vec![ok(), ok()]);
        assert_eq!(check(&twice, &info, None), [Problem::Duplicate]);
        let thick = glyph(vec![ok(), stroke(100.0, 900.0, 500.0, 90.0, 5)]);
        assert_eq!(check(&thick, &info, None), [Problem::UnevenWeight]);
        let high = glyph(vec![ok(), stroke(100.0, 900.0, 1200.0, 30.0, 5)]);
        assert_eq!(check(&high, &info, None), [Problem::OutsideEm]);
        let busy = glyph(vec![stroke(100.0, 900.0, 300.0, 30.0, 200)]);
        assert_eq!(check(&busy, &info, None), [Problem::ManyCircles]);
        // Half the reference's size; no advance; a stroke without skeleton.
        let small = glyph(vec![stroke(300.0, 700.0, 300.0, 30.0, 5)]);
        assert_eq!(check(&small, &info, Some(Rect::new(100.0, -50.0, 900.0, 750.0))), [Problem::OffReference]);
        let mut narrow = glyph(vec![ok()]);
        narrow.advance = 0.0;
        assert!(check(&narrow, &info, None).contains(&Problem::OddAdvance));
        let mut bare = ok();
        bare.skeleton.clear();
        assert!(check(&glyph(vec![ok(), bare]), &info, None).contains(&Problem::NoSkeleton));
    }
}
