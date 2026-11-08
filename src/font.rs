//! From handwritten strokes to font glyphs: the weight axis, each glyph's masters, a project as a
//! font ([`export`]) and a page to try a font in ([`preview_page`]).

use scriptoria_font::{Axis, FontSource, Glyph, LineMetrics, Names};
use scriptoria_ink::Stroke;
use scriptoria_ink::kurbo::{Affine, BezPath, Rect};
use scriptoria_outline::{PlanParams, WeightParams, expand, masters, plan};
use scriptoria_project::{FontInfo, Glyph as ProjectGlyph, Project};
use scriptoria_skeleton::{Piece, Sample, Skeleton};

/// The weight axis: `wght` from Thin (100) to Black (900), Regular (400) by default. The strokes as
/// written are the boldest weight, 900: strokes thin out cleanly, while made bolder they run into
/// each other and lose the hand's shapes.
pub const AXIS: Axis = Axis { min: 100.0, default: 400.0, max: 900.0 };

/// How close the font's quadratic splines are to the cubic curves made from skeletons (font units).
pub const QUADRATIC_ACCURACY: f64 = 0.5;

/// The radius scale of the thinnest weight (`wght` 100) by default.
pub const THINNEST: f64 = 0.3;

/// The named weights.
pub const INSTANCES: [(&str, f64); 9] = [
    ("Thin", 100.0),
    ("ExtraLight", 200.0),
    ("Light", 300.0),
    ("Regular", 400.0),
    ("Medium", 500.0),
    ("SemiBold", 600.0),
    ("Bold", 700.0),
    ("ExtraBold", 800.0),
    ("Black", 900.0),
];

/// The weight the strokes as written are by default: the boldest.
pub const WRITTEN: f64 = 900.0;

/// The lowest weight the strokes as written may be given: below it, the boldest weight would need
/// more than three times their radius.
pub const WRITTEN_MIN: f64 = 300.0;

/// How stroke radii follow the weight axis: linear in `wght`, from `thinnest` at the axis minimum
/// to the strokes as written (×1) at `written`, and on beyond it to the maximum. Linear, so that
/// the font's interpolation (linear in the normalized axis on either side of the default) is linear
/// in the scale, as [`masters`] assumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weights {
    pub thinnest: f64,
    pub written: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Weights { thinnest: THINNEST, written: WRITTEN }
    }
}

impl Weights {
    /// A project's weights.
    pub fn of(info: &FontInfo) -> Self {
        Weights { thinnest: info.thinnest, written: info.written_weight.clamp(WRITTEN_MIN, AXIS.max) }
    }

    /// The radius scale at a `wght` value (clamped to the axis).
    pub fn scale(&self, wght: f64) -> f64 {
        self.thinnest + (wght.clamp(AXIS.min, AXIS.max) - AXIS.min) / (self.written - AXIS.min) * (1.0 - self.thinnest)
    }

    /// The `wght` value of a radius scale: the inverse of [`Weights::scale`].
    pub fn wght(&self, scale: f64) -> f64 {
        AXIS.min + (scale - self.thinnest) / (1.0 - self.thinnest) * (self.written - AXIS.min)
    }

    /// The radius scales at the axis' minimum, default and maximum.
    pub fn scales(&self) -> (f64, f64, f64) {
        (self.scale(AXIS.min), self.scale(AXIS.default), self.scale(AXIS.max))
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
    pub weights: Weights,
}

impl Default for Layout {
    fn default() -> Self {
        // 4.5 pt per em: handwritten CJK characters in the sample are 3 to 4.5 pt tall. The
        // precision is the outline crate's default (0.004 pt), the tolerance twice that.
        let (em, units) = (4.5, 1000u16);
        let precision = PlanParams::default().precision * units as f64 / em;
        Layout { em, units_per_em: units, descender: -120, precision, tolerance: 2.0 * precision, weights: Weights::default() }
    }
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
    let skeletons: Vec<Skeleton> = strokes.iter().map(|s| transform(&crate::skeleton(s), to_font, k)).collect();
    let mut g = glyph_from_skeletons(&skeletons, layout.precision, layout.tolerance, layout.weights);
    g.name = scriptoria_project::glyph_name(c);
    g.chars = vec![c];
    g.advance = rect.width() * k;
    g
}

/// A glyph (without name, characters or advance) from its strokes' skeletons in font units (y up),
/// in writing order: one contour per stroke piece, at the masters any stroke needs.
fn glyph_from_skeletons(skeletons: &[Skeleton], precision: f64, tolerance: f64, weights: Weights) -> Glyph {
    let (thinnest, regular, boldest) = weights.scales();
    let params = PlanParams { precision, weights: (thinnest, boldest) };
    let plans: Vec<_> = skeletons.iter().map(|sk| plan(sk, &params)).collect();
    // The font interpolates on either side of the default on its own: masters for each side.
    let mut scales: Vec<f64> = plans
        .iter()
        .flat_map(|p| masters(p, (thinnest, regular), tolerance).into_iter().chain(masters(p, (regular, boldest), tolerance)))
        .collect();
    // Every glyph has the default and the axis' ends (also one without strokes).
    scales.extend([thinnest, regular, boldest]);
    scales.sort_by(f64::total_cmp);
    scales.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let masters = scales
        .iter()
        .map(|&s| {
            let mut outline = BezPath::new();
            for p in &plans {
                outline.extend(expand(p, &WeightParams::scaled(s)).elements().iter().copied());
            }
            // Rounded, so that the default and the ends come out exactly.
            ((weights.wght(s) * 1e6).round() / 1e6, outline)
        })
        .collect();
    Glyph { name: String::new(), chars: Vec::new(), advance: 0.0, masters }
}

/// A project's font, ready to write.
pub struct Export {
    pub source: FontSource,
    /// Glyphs left out for having no strokes yet.
    pub empty: Vec<String>,
}

impl Export {
    /// The font file.
    pub fn build(&self) -> Result<Vec<u8>, scriptoria_font::Error> {
        scriptoria_font::build(&self.source)
    }

    pub fn masters_per_glyph(&self) -> f64 {
        self.source.glyphs.iter().map(|g| g.masters.len()).sum::<usize>() as f64 / self.source.glyphs.len().max(1) as f64
    }
}

/// A project as a font: every glyph with strokes, its strokes where they are placed, on all cores.
/// Outlines follow the strokes within the outline crate's default precision (0.004 pt, in font
/// units at the project's points per em), and masters within twice that.
pub fn export(project: &Project) -> Export {
    let info = &project.font;
    let precision = PlanParams::default().precision * info.units_per_em as f64 / info.pt_per_em;
    let (with, without): (Vec<&ProjectGlyph>, Vec<&ProjectGlyph>) = project.glyphs.iter().partition(|g| !g.strokes.is_empty());
    let glyphs = crate::par_map(&with, |g| {
        let skeletons: Vec<Skeleton> = g.strokes.iter().map(crate::assign::placed_skeleton).collect();
        let mut glyph = glyph_from_skeletons(&skeletons, precision, 2.0 * precision, Weights::of(info));
        glyph.name = g.name.clone();
        glyph.chars = g.chars.clone();
        glyph.advance = g.advance;
        glyph
    });
    let source = FontSource {
        family: info.family.clone(),
        units_per_em: info.units_per_em,
        ascender: info.ascender,
        descender: info.descender,
        axis: AXIS,
        instances: INSTANCES.iter().map(|&(name, w)| (name.to_string(), w)).collect(),
        glyphs,
        accuracy: QUADRATIC_ACCURACY,
        tolerance: 2.0 * precision,
        names: Names {
            english: info.english_name.clone(),
            version: info.version,
            copyright: info.copyright.clone(),
            designer: info.designer.clone(),
            license: info.license.clone(),
        },
        lines: LineMetrics { line_gap: info.line_gap, x_height: info.x_height, cap_height: info.cap_height },
    };
    Export { source, empty: without.iter().map(|g| g.name.clone()).collect() }
}

/// What [`write_font`] wrote.
#[derive(Debug)]
pub struct Written {
    pub glyphs: usize,
    /// Glyphs left out for having no strokes yet.
    pub empty: Vec<String>,
    pub bytes: usize,
    pub masters_per_glyph: f64,
    /// The page to try the font in, next to it.
    pub preview: std::path::PathBuf,
}

/// Writes a project's font to `path`, and next to it a page to try it in (same name, `.html`).
pub fn write_font(project: &Project, path: &std::path::Path) -> anyhow::Result<Written> {
    let export = export(project);
    if export.source.glyphs.is_empty() {
        anyhow::bail!("还没有写好的字：先把笔画分配给字");
    }
    let bytes = export.build()?;
    std::fs::write(path, &bytes).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let text: String = export.source.glyphs.iter().flat_map(|g| g.chars.iter()).collect();
    let preview = path.with_extension("html");
    std::fs::write(&preview, preview_page(&bytes, &export.source.family, &text, None)).map_err(|e| anyhow::anyhow!("{}: {e}", preview.display()))?;
    Ok(Written {
        glyphs: export.source.glyphs.len(),
        masters_per_glyph: export.masters_per_glyph(),
        empty: export.empty,
        bytes: bytes.len(),
        preview,
    })
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
        // By default the strokes as written are the boldest.
        let w = Weights::default();
        assert_eq!(w.wght(THINNEST), AXIS.min);
        assert_eq!(w.wght(1.0), AXIS.max);
        assert!((w.wght(0.65) - 500.0).abs() < 1e-9);
        assert!((w.scale(AXIS.default) - 0.5625).abs() < 1e-9);
        for s in [0.3, 0.5, 0.65, 1.0] {
            assert!((w.scale(w.wght(s)) - s).abs() < 1e-9, "{s}");
        }
        assert_eq!(w.scale(1000.0), 1.0);
        // Written at 500, they are bolder beyond.
        let w = Weights { thinnest: 0.3, written: 500.0 };
        assert!((w.scale(500.0) - 1.0).abs() < 1e-9);
        assert!((w.scales().2 - 1.7).abs() < 1e-9);
        assert!((w.wght(1.7) - 900.0).abs() < 1e-9);
        // Project settings out of range are brought in.
        let info = FontInfo { written_weight: 100.0, ..Default::default() };
        assert_eq!(Weights::of(&info).written, WRITTEN_MIN);
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
