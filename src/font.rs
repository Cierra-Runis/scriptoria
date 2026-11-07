//! From handwritten strokes to font glyphs: the weight axis, and each glyph's masters.

use scriptoria_font::{Axis, Glyph};
use scriptoria_ink::Stroke;
use scriptoria_ink::kurbo::{Affine, BezPath, Rect};
use scriptoria_outline::{PlanParams, WeightParams, expand, masters, plan};
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// The weight axis: the strokes as written are the boldest weight and the default, Regular (400);
/// the axis runs from there down to Thin (100). Strokes thin out cleanly; made bolder, they run
/// into each other and lose the hand's shapes.
pub const AXIS: Axis = Axis { min: 100.0, default: 400.0, max: 400.0 };

/// The radius scale of the thinnest weight (`wght` 100) by default.
pub const THINNEST: f64 = 0.3;

/// The named weights.
pub const INSTANCES: [(&str, f64); 4] = [("Thin", 100.0), ("ExtraLight", 200.0), ("Light", 300.0), ("Regular", 400.0)];

/// The `wght` value of a radius scale, from `thinnest` (at the axis minimum) to 1 (the strokes as
/// written, at the default): linear, so that the font's interpolation (linear in the normalized
/// axis) is linear in the scale, as [`masters`] assumes.
pub fn wght(scale: f64, thinnest: f64) -> f64 {
    AXIS.min + (scale - thinnest) / (1.0 - thinnest) * (AXIS.default - AXIS.min)
}

/// The radius scale at a `wght` value: the inverse of [`wght`].
pub fn scale_at(wght: f64, thinnest: f64) -> f64 {
    thinnest + (wght.clamp(AXIS.min, AXIS.default) - AXIS.min) / (AXIS.default - AXIS.min) * (1.0 - thinnest)
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
    /// The radius scale of the thinnest weight.
    pub thinnest: f64,
}

impl Default for Layout {
    fn default() -> Self {
        // 4.5 pt per em: handwritten CJK characters in the sample are 3 to 4.5 pt tall. The
        // precision is the outline crate's default (0.004 pt), the tolerance twice that.
        let (em, units) = (4.5, 1000u16);
        let precision = PlanParams::default().precision * units as f64 / em;
        Layout { em, units_per_em: units, descender: -120, precision, tolerance: 2.0 * precision, thinnest: THINNEST }
    }
}

/// A glyph's name: the character itself for ASCII letters and digits, `uniXXXX` otherwise.
pub fn glyph_name(c: char) -> String {
    if c.is_ascii_alphanumeric() { c.to_string() } else { format!("uni{:04X}", c as u32) }
}

/// The glyph a character's strokes make, written inside `rect` (page coordinates, y down): the
/// box's left edge at x = 0 and its bottom at the layout's descender, its width the advance. One
/// contour per stroke piece, strokes in the order given (the writing order). Its masters are the
/// weights any of its strokes needs ([`masters`]).
pub fn glyph(c: char, strokes: &[&Stroke], rect: Rect, layout: &Layout) -> Glyph {
    let k = layout.units_per_em as f64 / layout.em;
    // Page (y down) to font units (y up): the skeleton is moved before outlines are made, so that
    // "left" in the outline crate is the writer's left and contours run clockwise with y up.
    let to_font = Affine::new([k, 0.0, 0.0, -k, -rect.x0 * k, rect.y1 * k + layout.descender as f64]);
    let params = PlanParams { precision: layout.precision, weights: (layout.thinnest, 1.0) };
    let plans: Vec<_> = strokes.iter().map(|s| plan(&transform(&crate::skeleton(s), to_font, k), &params)).collect();
    let mut scales: Vec<f64> = plans.iter().flat_map(|p| masters(p, params.weights, layout.tolerance)).collect();
    scales.sort_by(f64::total_cmp);
    scales.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let masters = scales
        .iter()
        .map(|&s| {
            let mut outline = BezPath::new();
            for p in &plans {
                outline.extend(expand(p, &WeightParams::scaled(s)).elements().iter().copied());
            }
            (wght(s, layout.thinnest), outline)
        })
        .collect();
    Glyph { name: glyph_name(c), chars: vec![c], advance: rect.width() * k, masters }
}

/// A web page to try a font in: `text` in it at each named weight and under a weight slider, the
/// font embedded. With a weight given, only that weight is shown, large and without the slider
/// (for screenshots).
pub fn preview_page(font: &[u8], family: &str, text: &str, fixed: Option<f64>) -> String {
    let text = escape(text);
    let rows: String = match fixed {
        Some(w) => format!("<p class=\"big\" style=\"font-variation-settings: 'wght' {w}\">{text}</p>"),
        None => INSTANCES
            .iter()
            .map(|(name, w)| format!("<p style=\"font-variation-settings: 'wght' {w}\"><span>{w} {name}</span>{text}</p>\n"))
            .collect(),
    };
    let slider = if fixed.is_some() {
        String::new()
    } else {
        format!(
            "<label>wght <input id=\"w\" type=\"range\" min=\"{min}\" max=\"{max}\" value=\"{default}\"> <output id=\"v\">{default}</output></label>\n<p id=\"live\" class=\"big\">{text}</p>\n<script>const w=document.getElementById('w'),v=document.getElementById('v'),l=document.getElementById('live');w.oninput=()=>{{v.textContent=w.value;l.style.fontVariationSettings=`'wght' ${{w.value}}`}};</script>",
            min = AXIS.min,
            max = AXIS.max,
            default = AXIS.default,
        )
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>
<style>
@font-face {{ font-family: 'Preview'; src: url(data:font/ttf;base64,{data}) format('truetype'); font-weight: {min} {max}; }}
body {{ font-family: system-ui, sans-serif; margin: 24px; background: #fff; color: #111; }}
p {{ font-family: 'Preview'; font-size: 56px; margin: 8px 0; line-height: 1.3; }}
p span {{ font-family: system-ui, sans-serif; font-size: 14px; color: #888; display: inline-block; width: 120px; }}
.big {{ font-size: 96px; }}
</style>
<h1 style=\"font-size: 18px; font-weight: 600\">{title}</h1>
{slider}
{rows}",
        title = escape(family),
        data = base64(font),
        min = AXIS.min,
        max = AXIS.max,
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
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
        assert_eq!(wght(THINNEST, THINNEST), AXIS.min);
        assert_eq!(wght(1.0, THINNEST), AXIS.default);
        assert!((wght(0.65, THINNEST) - 250.0).abs() < 1e-9);
        for s in [0.3, 0.5, 0.65, 1.0] {
            assert!((scale_at(wght(s, THINNEST), THINNEST) - s).abs() < 1e-9, "{s}");
        }
        // Nothing bolder than the strokes as written.
        assert_eq!(scale_at(900.0, THINNEST), 1.0);
    }

    #[test]
    fn fonts_are_embedded_in_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        let page = preview_page(b"foo", "A <b>", "x&y", None);
        assert!(page.contains("base64,Zm9v)") && page.contains("A &lt;b&gt;") && page.contains("x&amp;y"));
    }
}
