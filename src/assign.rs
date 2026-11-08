//! Putting written strokes into a project's glyphs, and placing glyphs as the reference font does.

use scriptoria_ink::kurbo::{Affine, Rect};
use scriptoria_ink::{Stroke, StrokeShape};
use scriptoria_project::{
    FontInfo, Frame, Glyph, GlyphStroke, Outline, OutlineKind, Project, Reference, SkeletonPiece, StrokeOrigin, initial_placement,
};
use scriptoria_reference::ReferenceFont;
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// A written stroke as a glyph's stroke: its outline, its skeleton (recovered now) and where it
/// came from, placed by `placement`, its width at `pen` (see [`GlyphStroke::pen`]).
pub fn glyph_stroke(stroke: &Stroke, origin: Option<StrokeOrigin>, placement: Affine, pen: f64) -> GlyphStroke {
    let kind = match stroke.shape {
        StrokeShape::Outline { .. } => OutlineKind::Filled,
        StrokeShape::Line { width } => OutlineKind::Line { width },
    };
    let skeleton = crate::skeleton(stroke)
        .pieces
        .iter()
        .map(|p| SkeletonPiece { circles: p.samples.iter().map(|s| (s.p, s.r)).collect(), spine: p.spine.clone() })
        .collect();
    GlyphStroke { origin, outline: Outline { kind, path: stroke.path.clone() }, skeleton, placement, pen }
}

/// Adds strokes (with where they came from) to the end of a glyph, in the order given, all
/// placed alike.
///
/// Strokes from the page the glyph's last stroke came from join it where they were written
/// (they take its placement: a stroke forgotten in the first go). Otherwise the box around them
/// is scaled by the project's page units per em, its bottom on the descender, its left edge at
/// x = 0; and a glyph that had no strokes is then placed as the reference font places its
/// character ([`fit`]), or takes the box's width as its advance without a reference.
pub fn assign(project: &mut Project, glyph: usize, strokes: &[(&Stroke, StrokeOrigin)], reference: Option<&ReferenceFont>) {
    let Some(bounds) = strokes.iter().map(|(s, _)| s.bbox).reduce(|a: Rect, b| a.union(b)) else { return };
    let info = project.font.clone();
    let g = &mut project.glyphs[glyph];
    let same_page = |s: &GlyphStroke| {
        let (a, b) = (s.origin.as_ref(), strokes.first().map(|(_, o)| o));
        a.zip(b).is_some_and(|(a, b)| a.source == b.source && a.page == b.page)
    };
    let joining = g.strokes.last().filter(|s| same_page(s)).map(|s| (s.placement, s.pen));
    let was_empty = g.strokes.is_empty();
    let (placement, pen) = joining.unwrap_or_else(|| (initial_placement(bounds, &info), info.pen_scale()));
    g.strokes.extend(strokes.iter().map(|(s, origin)| glyph_stroke(s, Some(origin.clone()), placement, pen)));
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
/// written in one place, that the reference has); with how many glyphs that is. None without any.
pub fn estimate_pt_per_em(project: &Project, reference: &ReferenceFont) -> Option<(f64, usize)> {
    use scriptoria_ink::kurbo::Shape as _;
    let upm = project.font.units_per_em as f64;
    let mut sizes: Vec<f64> = project
        .glyphs
        .iter()
        .filter_map(|g| {
            let c = *g.chars.first()?;
            // Outlines are in their page's coordinates: all from one page, or the box means nothing.
            let page = |s: &GlyphStroke| s.origin.as_ref().map(|o| (o.source.clone(), o.page));
            let first = page(g.strokes.first()?);
            if g.strokes.iter().any(|s| page(s) != first) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_font::{Axis, FontSource, build};
    use scriptoria_ink::kurbo::Shape as _;

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
        };
        ReferenceFont::from_bytes(build(&source).unwrap(), None).unwrap()
    }

    fn stroke(page: usize, r: Rect) -> GlyphStroke {
        GlyphStroke {
            origin: Some(StrokeOrigin { source: "s1".into(), page, index: 0 }),
            outline: Outline { kind: OutlineKind::Filled, path: r.to_path(0.1) },
            skeleton: Vec::new(),
            placement: Affine::IDENTITY,
            pen: 1.0,
        }
    }

    #[test]
    fn the_hands_size_is_measured_by_the_reference() {
        let mut p = Project::new("x");
        assert_eq!(estimate_pt_per_em(&p, &reference()), None);
        p.add_chars("口一");
        // 口 written 2.4 pt square (two strokes), as large as the reference's 800 units at 3 pt per em.
        p.glyphs[0].strokes = vec![stroke(0, Rect::new(10.0, 10.0, 12.4, 11.0)), stroke(0, Rect::new(10.0, 11.0, 11.0, 12.4))];
        // 一 is not in the reference.
        p.glyphs[1].strokes = vec![stroke(0, Rect::new(0.0, 0.0, 3.0, 0.3))];
        let (pt, n) = estimate_pt_per_em(&p, &reference()).unwrap();
        assert!((pt - 3.0).abs() < 1e-6 && n == 1, "{pt}");
        // Strokes from two pages do not make one box.
        p.glyphs[0].strokes[1] = stroke(1, Rect::new(10.0, 11.0, 11.0, 12.4));
        assert_eq!(estimate_pt_per_em(&p, &reference()), None);
    }
}
