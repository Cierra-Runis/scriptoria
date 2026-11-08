//! Putting written strokes into a project's glyphs, and placing glyphs as the reference font does.

use scriptoria_ink::kurbo::{Affine, Rect, Shape as _};
use scriptoria_ink::{Stroke, StrokeShape};
use scriptoria_project::{
    FontInfo, Frame, Glyph, GlyphStroke, Outline, OutlineKind, Project, Reference, SkeletonPiece, initial_placement,
};
use scriptoria_reference::ReferenceFont;
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// A written stroke as a glyph's stroke: its outline and its skeleton (recovered now), placed by
/// `placement`, its width at `pen` (see [`GlyphStroke::pen`]).
pub fn glyph_stroke(stroke: &Stroke, placement: Affine, pen: f64) -> GlyphStroke {
    let kind = match stroke.shape {
        StrokeShape::Outline { .. } => OutlineKind::Filled,
        StrokeShape::Line { width } => OutlineKind::Line { width },
    };
    let skeleton = crate::skeleton(stroke)
        .pieces
        .iter()
        .map(|p| SkeletonPiece { circles: p.samples.iter().map(|s| (s.p, s.r)).collect(), spine: p.spine.clone() })
        .collect();
    GlyphStroke { outline: Outline { kind, path: stroke.path.clone() }, skeleton, placement, pen }
}

/// Adds written strokes to the end of a glyph, in the order given, all placed alike.
///
/// Strokes written next to the glyph's last stroke (within an em of it on the page) join it where
/// they were written: they take its placement (a stroke forgotten in the first go). Otherwise the box around them
/// is scaled by the project's page units per em, its bottom on the descender, its left edge at
/// x = 0; and a glyph that had no strokes is then placed as the reference font places its
/// character ([`fit`]), or takes the box's width as its advance without a reference.
pub fn assign(project: &mut Project, glyph: usize, strokes: &[&Stroke], reference: Option<&ReferenceFont>) {
    let Some(bounds) = strokes.iter().map(|s| s.bbox).reduce(|a: Rect, b| a.union(b)) else { return };
    let info = project.font.clone();
    let g = &mut project.glyphs[glyph];
    let em = info.pt_per_em;
    let near = |s: &GlyphStroke| {
        let b = s.outline.path.bounding_box().inflate(em, em);
        b.x0 <= bounds.x1 && bounds.x0 <= b.x1 && b.y0 <= bounds.y1 && bounds.y0 <= b.y1
    };
    let joining = g.strokes.last().filter(|s| near(s)).map(|s| (s.placement, s.pen));
    let was_empty = g.strokes.is_empty();
    let (placement, pen) = joining.unwrap_or_else(|| (initial_placement(bounds, &info), info.pen_scale()));
    g.strokes.extend(strokes.iter().map(|s| glyph_stroke(s, placement, pen)));
    if was_empty {
        g.advance = (bounds.width() * info.pen_scale()).round();
        if let Some(reference) = reference {
            fit(g, &info, reference);
        }
    }
}

/// The reference font installed on this system, if any (see
/// [`scriptoria_reference::system_default`]), with the project setting naming it.
pub fn system_reference() -> Option<(Reference, ReferenceFont)> {
    let file = scriptoria_reference::system_default()?;
    let font = ReferenceFont::open(&file, None).ok()?;
    Some((Reference { file, face: font.face().index }, font))
}

/// A project's reference font.
pub fn open_reference(reference: &Reference) -> Result<ReferenceFont, scriptoria_reference::Error> {
    ReferenceFont::open(&reference.file, Some(reference.face))
}

/// Where the reference font puts a glyph's (first) character, in the project's units; none if
/// it lacks the character.
pub fn reference_frame(reference: &ReferenceFont, glyph: &Glyph, info: &FontInfo) -> Option<Frame> {
    let c = *glyph.chars.first()?;
    let g = reference.glyph(c, info.units_per_em as f64)?;
    Some(Frame { bounds: g.bounds, advance: g.advance })
}

/// Places a glyph as the reference font places its character (see [`Glyph::fit`]), as closely
/// as the project asks; returns whether the reference has the character.
pub fn fit(glyph: &mut Glyph, info: &FontInfo, reference: &ReferenceFont) -> bool {
    match reference_frame(reference, glyph, info) {
        Some(frame) => {
            glyph.fit(&frame, info, info.fit_strength);
            true
        }
        None => false,
    }
}

/// A glyph stroke's skeleton in font units: moved by its placement, its radii at its pen's width
/// (see [`GlyphStroke::pen`]).
pub fn placed_skeleton(stroke: &GlyphStroke) -> Skeleton {
    skeleton_of(&stroke.skeleton, stroke.placement, stroke.pen)
}

/// A stored skeleton moved by `t`, its radii scaled by `radius`.
pub fn skeleton_of(pieces: &[SkeletonPiece], t: Affine, radius: f64) -> Skeleton {
    Skeleton {
        pieces: pieces
            .iter()
            .map(|p| Piece {
                samples: p.circles.iter().map(|&(c, r)| Sample { p: t * c, r: r * radius }).collect(),
                spine: p.spine.iter().map(|c| t * *c).collect(),
            })
            .collect(),
        quality: Default::default(),
    }
}

/// How large a project's hand writes, as the reference font measures it: the points per em (see
/// [`FontInfo::pt_per_em`]) at which its written glyphs are as large as the reference's
/// characters, by the square root of their ink boxes' areas, the median over the glyphs (those
/// whose strokes are placed alike, that the reference has); with how many glyphs that is. None without any.
pub fn estimate_pt_per_em(project: &Project, reference: &ReferenceFont) -> Option<(f64, usize)> {
    use scriptoria_ink::kurbo::Shape as _;
    let upm = project.font.units_per_em as f64;
    let mut sizes: Vec<f64> = project
        .glyphs
        .iter()
        .filter_map(|g| {
            let c = *g.chars.first()?;
            // Outlines are in their page's coordinates: only strokes placed together (written in one
            // go, on one page) make a box that means something.
            let first = g.strokes.first()?.placement;
            if g.strokes.iter().any(|s| s.placement != first) {
                return None;
            }
            let hand = g.strokes.iter().map(|s| s.outline.path.bounding_box()).reduce(|a, b| a.union(b))?;
            let model = reference.glyph(c, upm)?.bounds;
            (hand.area() > 0.0 && model.area() > 0.0).then(|| (hand.area() / model.area()).sqrt() * upm)
        })
        .collect();
    if sizes.is_empty() {
        return None;
    }
    sizes.sort_by(f64::total_cmp);
    Some((sizes[sizes.len() / 2], sizes.len()))
}

/// How much wider than the reference the hand writes, all over: the median, over the glyphs
/// written in one go that the reference has, of their width-to-height against the reference's
/// (see [`FontInfo::aspect_bias`]); with how many glyphs that is. None without any.
pub fn estimate_aspect_bias(project: &Project, reference: &ReferenceFont) -> Option<(f64, usize)> {
    use scriptoria_ink::kurbo::Shape as _;
    let upm = project.font.units_per_em as f64;
    let mut ratios: Vec<f64> = project
        .glyphs
        .iter()
        .filter_map(|g| {
            let c = *g.chars.first()?;
            let first = g.strokes.first()?.placement;
            if g.strokes.iter().any(|s| s.placement != first) {
                return None;
            }
            let hand = g.strokes.iter().map(|s| s.outline.path.bounding_box()).reduce(|a, b| a.union(b))?;
            let model = reference.glyph(c, upm)?.bounds;
            let tall = |w: f64, h: f64| w.max(h) > 2.5 * w.min(h);
            (hand.area() > 0.0 && model.area() > 0.0 && !tall(hand.width(), hand.height()) && !tall(model.width(), model.height()))
                .then(|| (hand.width() / hand.height()) / (model.width() / model.height()))
        })
        .collect();
    if ratios.is_empty() {
        return None;
    }
    ratios.sort_by(f64::total_cmp);
    Some((ratios[ratios.len() / 2], ratios.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_font::{Axis, FontSource, build};

    /// A reference font with 口 an 800 × 800 square.
    fn reference() -> ReferenceFont {
        let square = Rect::new(100.0, -50.0, 900.0, 750.0).to_path(0.1);
        let glyph = scriptoria_font::Glyph {
            name: "uni53E3".into(),
            chars: vec!['口'],
            advance: 1000.0,
            masters: vec![(100.0, square.clone()), (400.0, square.clone()), (900.0, square)],
        };
        let source = FontSource {
            family: "Measure".into(),
            units_per_em: 1000,
            ascender: 880,
            descender: -120,
            axis: Axis { min: 100.0, default: 400.0, max: 900.0 },
            instances: vec![("Regular".into(), 400.0)],
            glyphs: vec![glyph],
            accuracy: 0.5,
            tolerance: 0.0,
            names: Default::default(),
            lines: Default::default(),
        };
        ReferenceFont::from_bytes(build(&source).unwrap(), None).unwrap()
    }

    /// A stroke of box `r`, placed by a translation of `dx`.
    fn stroke(dx: f64, r: Rect) -> GlyphStroke {
        GlyphStroke {
            outline: Outline { kind: OutlineKind::Filled, path: r.to_path(0.1) },
            skeleton: Vec::new(),
            placement: Affine::translate((dx, 0.0)),
            pen: 1.0,
        }
    }

    #[test]
    fn the_hands_size_is_measured_by_the_reference() {
        let mut p = Project::new("x");
        assert_eq!(estimate_pt_per_em(&p, &reference()), None);
        p.add_chars("口一");
        // 口 written 2.4 pt square (two strokes), as large as the reference's 800 units at 3 pt per em.
        p.glyphs[0].strokes = vec![stroke(0.0, Rect::new(10.0, 10.0, 12.4, 11.0)), stroke(0.0, Rect::new(10.0, 11.0, 11.0, 12.4))];
        // 一 is not in the reference.
        p.glyphs[1].strokes = vec![stroke(0.0, Rect::new(0.0, 0.0, 3.0, 0.3))];
        let (pt, n) = estimate_pt_per_em(&p, &reference()).unwrap();
        assert!((pt - 3.0).abs() < 1e-6 && n == 1, "{pt}");
        // Written twice as wide as high, against a square 口: twice as wide, all over.
        p.glyphs[0].strokes = vec![stroke(0.0, Rect::new(10.0, 10.0, 14.8, 12.4))];
        let (bias, n) = estimate_aspect_bias(&p, &reference()).unwrap();
        assert!((bias - 2.0).abs() < 1e-6 && n == 1, "{bias}");
        p.glyphs[0].strokes = vec![stroke(0.0, Rect::new(10.0, 10.0, 12.4, 11.0)), stroke(0.0, Rect::new(10.0, 11.0, 11.0, 12.4))];
        // Strokes placed apart (one moved on its own) do not make one box.
        p.glyphs[0].strokes[1] = stroke(1.0, Rect::new(10.0, 11.0, 11.0, 12.4));
        assert_eq!(estimate_pt_per_em(&p, &reference()), None);
    }
}
