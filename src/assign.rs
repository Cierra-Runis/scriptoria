//! Putting written strokes into a project's glyphs.

use scriptoria_ink::kurbo::{Affine, Rect};
use scriptoria_ink::{Stroke, StrokeShape};
use scriptoria_project::{GlyphStroke, Outline, OutlineKind, Project, SkeletonPiece, StrokeOrigin, initial_placement};
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// A written stroke as a glyph's stroke: its outline, its skeleton (recovered now) and where it
/// came from, placed by `placement`.
pub fn glyph_stroke(stroke: &Stroke, origin: Option<StrokeOrigin>, placement: Affine) -> GlyphStroke {
    let kind = match stroke.shape {
        StrokeShape::Outline { .. } => OutlineKind::Filled,
        StrokeShape::Line { width } => OutlineKind::Line { width },
    };
    let skeleton = crate::skeleton(stroke)
        .pieces
        .iter()
        .map(|p| SkeletonPiece { circles: p.samples.iter().map(|s| (s.p, s.r)).collect(), spine: p.spine.clone() })
        .collect();
    GlyphStroke { origin, outline: Outline { kind, path: stroke.path.clone() }, skeleton, placement }
}

/// Adds strokes (with where they came from) to the end of a glyph, in the order given, all
/// placed alike: the box around them scaled by the project's page units per em, its bottom on the
/// descender, its left edge at x = 0. A glyph without strokes takes the box's width as its advance.
pub fn assign(project: &mut Project, glyph: usize, strokes: &[(&Stroke, StrokeOrigin)]) {
    let Some(bounds) = strokes.iter().map(|(s, _)| s.bbox).reduce(|a: Rect, b| a.union(b)) else { return };
    let placement = initial_placement(bounds, &project.font);
    let k = project.font.units_per_em as f64 / project.font.pt_per_em;
    let g = &mut project.glyphs[glyph];
    if g.strokes.is_empty() {
        g.advance = (bounds.width() * k).round();
    }
    g.strokes.extend(strokes.iter().map(|(s, origin)| glyph_stroke(s, Some(origin.clone()), placement)));
}

/// A glyph stroke's skeleton in font units: moved by its placement.
pub fn placed_skeleton(stroke: &GlyphStroke) -> Skeleton {
    skeleton_of(&stroke.skeleton, stroke.placement)
}

/// A stored skeleton moved by `t` (a similarity: radii scale with it).
pub fn skeleton_of(pieces: &[SkeletonPiece], t: Affine) -> Skeleton {
    let k = t.determinant().abs().sqrt();
    Skeleton {
        pieces: pieces
            .iter()
            .map(|p| Piece {
                samples: p.circles.iter().map(|&(c, r)| Sample { p: t * c, r: r * k }).collect(),
                spine: p.spine.iter().map(|c| t * *c).collect(),
            })
            .collect(),
        quality: Default::default(),
    }
}
