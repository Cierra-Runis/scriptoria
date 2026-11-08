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
