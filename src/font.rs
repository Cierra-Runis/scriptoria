//! From handwritten strokes to font glyphs: the weight axis, and each glyph's masters.

use scriptoria_font::{Axis, Glyph};
use scriptoria_ink::Stroke;
use scriptoria_ink::kurbo::{Affine, BezPath, Rect};
use scriptoria_outline::{PlanParams, WeightParams, expand, masters, plan};
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// The weight axis: `wght` from 100 to 900, the writer's own (400) by default.
pub const AXIS: Axis = Axis { min: 100.0, default: 400.0, max: 900.0 };

/// The radius scales at the axis' minimum, default and maximum.
pub const SCALES: (f64, f64, f64) = (0.3, 1.0, 3.0);

/// The `wght` value of a radius scale: linear from each end of the axis to the default, so that
/// the font's interpolation (linear in the normalized axis on either side of the default) is
/// linear in the scale, as [`masters`] assumes.
pub fn wght(scale: f64) -> f64 {
    let (s0, s1, s2) = SCALES;
    if scale <= s1 {
        AXIS.default - (s1 - scale) / (s1 - s0) * (AXIS.default - AXIS.min)
    } else {
        AXIS.default + (scale - s1) / (s2 - s1) * (AXIS.max - AXIS.default)
    }
}

/// The radius scale at a `wght` value, for the scales at the axis' minimum, default and maximum:
/// linear on either side of the default (the inverse of [`wght`] for [`SCALES`]).
pub fn scale_at(wght: f64, [s0, s1, s2]: [f64; 3]) -> f64 {
    if wght <= AXIS.default {
        s1 - (AXIS.default - wght) / (AXIS.default - AXIS.min) * (s1 - s0)
    } else {
        s1 + (wght - AXIS.default) / (AXIS.max - AXIS.default) * (s2 - s1)
    }
}

/// How handwriting is placed in the em.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    /// Page units (pt) per em.
    pub em: f64,
    pub units_per_em: u16,
    /// Where the bottom of a character's box goes (font units, below the baseline).
    pub descender: i16,
    /// Largest distance of an outline from the strokes' exact shape (font units).
    pub precision: f64,
    /// How far interpolating between masters may stray (font units).
    pub tolerance: f64,
}

impl Default for Layout {
    fn default() -> Self {
        // 4.5 pt per em: handwritten CJK characters in the sample are 3 to 4.5 pt tall. The
        // precision is the outline crate's default (0.004 pt), the tolerance twice that.
        let (em, units) = (4.5, 1000u16);
        let precision = PlanParams::default().precision * units as f64 / em;
        Layout { em, units_per_em: units, descender: -120, precision, tolerance: 2.0 * precision }
    }
}

/// A glyph's name: the character itself for ASCII letters and digits, `uniXXXX` otherwise.
pub fn glyph_name(c: char) -> String {
    if c.is_ascii_alphanumeric() { c.to_string() } else { format!("uni{:04X}", c as u32) }
}

/// The glyph a character's strokes make, written inside `rect` (page coordinates, y down): the
/// box's left edge at x = 0 and its bottom at the layout's descender, its width the advance. One
/// contour per stroke piece, strokes in the order given (the writing order). Its masters are the
/// weights any of its strokes needs ([`masters`], on either side of the default).
pub fn glyph(c: char, strokes: &[&Stroke], rect: Rect, layout: &Layout) -> Glyph {
    let k = layout.units_per_em as f64 / layout.em;
    // Page (y down) to font units (y up): the skeleton is moved before outlines are made, so that
    // "left" in the outline crate is the writer's left and contours run clockwise with y up.
    let to_font = Affine::new([k, 0.0, 0.0, -k, -rect.x0 * k, rect.y1 * k + layout.descender as f64]);
    let params = PlanParams { precision: layout.precision, weights: (SCALES.0, SCALES.2) };
    let plans: Vec<_> = strokes.iter().map(|s| plan(&transform(&crate::skeleton(s), to_font, k), &params)).collect();
    let mut scales: Vec<f64> = plans
        .iter()
        .flat_map(|p| masters(p, (SCALES.0, SCALES.1), layout.tolerance).into_iter().chain(masters(p, (SCALES.1, SCALES.2), layout.tolerance)))
        .collect();
    scales.sort_by(f64::total_cmp);
    scales.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let masters = scales
        .iter()
        .map(|&s| {
            let mut outline = BezPath::new();
            for p in &plans {
                outline.extend(expand(p, &WeightParams::scaled(s)).elements().iter().copied());
            }
            (wght(s), outline)
        })
        .collect();
    Glyph { name: glyph_name(c), chars: vec![c], advance: rect.width() * k, masters }
}

/// A skeleton moved by a similarity transform scaling by `k`.
fn transform(sk: &Skeleton, t: Affine, k: f64) -> Skeleton {
    Skeleton {
        pieces: sk
            .pieces
            .iter()
            .map(|p| Piece {
                samples: p.samples.iter().map(|s| Sample { p: t * s.p, r: s.r * k }).collect(),
                spine: p.spine.iter().map(|c| t * *c).collect(),
            })
            .collect(),
        quality: sk.quality.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_map_to_the_axis() {
        assert_eq!(wght(SCALES.0), AXIS.min);
        assert_eq!(wght(SCALES.1), AXIS.default);
        assert_eq!(wght(SCALES.2), AXIS.max);
        assert!((wght(2.0) - 650.0).abs() < 1e-9);
        let scales = [SCALES.0, SCALES.1, SCALES.2];
        for s in [0.3, 0.65, 1.0, 2.0, 3.0] {
            assert!((scale_at(wght(s), scales) - s).abs() < 1e-9, "{s}");
        }
    }
}
