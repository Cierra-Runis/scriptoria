//! Glyph files: a glyph's strokes as SVG.
//!
//! ```text
//! <svg viewBox="…" …>                          the em (font units, y flipped for viewing)
//!   <g transform="scale(1 -1)">                font units, y up
//!     <g class="stroke" data-origin="s1 2 1617" data-pen="222.2" transform="matrix(a b c d e f)">
//!                                              one per stroke, in writing order; the matrix is its
//!                                              placement, page coordinates to font units; data-pen
//!                                              its pen scale (font units per page unit for widths)
//!       <path class="outline" data-kind="filled" d="…"/>               the stroke as written
//!       <path class="outline" data-kind="line" data-width="0.25" d="…"/>
//!       <path class="piece" data-r="r₀ r₁ …" d="M … C …"/>            its skeleton, a piece each:
//!                                              the centerline, one cubic between consecutive circle
//!                                              centers (a lone `M x y` for a single circle), and
//!                                              their radii
//!     </g>
//!   </g>
//! </svg>
//! ```
//!
//! Numbers are written in full (`f64` round trips exactly); everything else (titles, colors,
//! the viewing transform) is for people and ignored when reading.

use std::fmt::Write as _;

use kurbo::{Affine, BezPath, CubicBez, PathEl, Point};

use crate::{FontInfo, Glyph, GlyphStroke, Outline, OutlineKind, SkeletonPiece, StrokeOrigin};

const INK: &str = "#1f2937";
const SKELETON: &str = "#2563eb";

pub(crate) fn write_strokes(glyph: &Glyph, font: &FontInfo) -> String {
    let (top, bottom) = (font.ascender as f64, font.descender as f64);
    let left = glyph.bounds().map_or(0.0, |b| b.x0.min(0.0));
    let right = glyph.bounds().map_or(glyph.advance, |b| b.x1.max(glyph.advance));
    let chars: String = glyph.chars.iter().collect();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{left} {} {} {}\" width=\"{:.0}\" height=\"400\">",
        -top,
        right - left,
        top - bottom,
        400.0 * (right - left) / (top - bottom)
    );
    let _ = writeln!(s, "<title>{} {}</title>", escape(&glyph.name), escape(&chars));
    let _ = writeln!(s, "<rect x=\"0\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#f9fafb\"/>", -top, glyph.advance, top - bottom);
    s += "<g transform=\"scale(1 -1)\">\n";
    for stroke in &glyph.strokes {
        let [a, b, c, d, e, f] = stroke.placement.as_coeffs();
        let origin = stroke.origin.as_ref().map_or(String::new(), |o| format!(" data-origin=\"{} {} {}\"", escape(&o.source), o.page, o.index));
        let _ = writeln!(s, "<g class=\"stroke\"{origin} data-pen=\"{}\" transform=\"matrix({a} {b} {c} {d} {e} {f})\">", stroke.pen);
        match stroke.outline.kind {
            OutlineKind::Filled => {
                let _ = writeln!(s, "<path class=\"outline\" data-kind=\"filled\" fill=\"{INK}\" d=\"{}\"/>", stroke.outline.path.to_svg());
            }
            OutlineKind::Line { width } => {
                let _ = writeln!(
                    s,
                    "<path class=\"outline\" data-kind=\"line\" data-width=\"{width}\" fill=\"none\" stroke=\"{INK}\" stroke-width=\"{width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" d=\"{}\"/>",
                    stroke.outline.path.to_svg()
                );
            }
        }
        // About 4 font units wide, whatever the placement's scale.
        let width = 4.0 / stroke.placement.determinant().abs().sqrt().max(1e-12);
        for piece in &stroke.skeleton {
            let radii: Vec<String> = piece.circles.iter().map(|(_, r)| r.to_string()).collect();
            let _ = writeln!(
                s,
                "<path class=\"piece\" data-r=\"{}\" fill=\"none\" stroke=\"{SKELETON}\" stroke-width=\"{width}\" d=\"{}\"/>",
                radii.join(" "),
                centerline(piece).to_svg()
            );
        }
        s += "</g>\n";
    }
    s += "</g>\n</svg>\n";
    s
}

fn centerline(piece: &SkeletonPiece) -> BezPath {
    let mut path = BezPath::new();
    if let Some(&(c, _)) = piece.circles.first() {
        path.move_to(c);
    }
    for seg in &piece.spine {
        path.curve_to(seg.p1, seg.p2, seg.p3);
    }
    path
}

/// `pen`: the pen scale of strokes that do not give theirs (files from before it was kept).
pub(crate) fn read_strokes(text: &str, pen: f64) -> Result<Vec<GlyphStroke>, String> {
    let mut strokes: Vec<GlyphStroke> = Vec::new();
    let mut in_stroke = false;
    for tag in tags(text)? {
        match (tag.name.as_str(), tag.close) {
            ("g", false) if tag.get("class").as_deref() == Some("stroke") => {
                let placement = matrix(&tag.get("transform").ok_or("a stroke without a transform")?)?;
                let origin = match tag.get("data-origin") {
                    Some(o) => {
                        let parts: Vec<&str> = o.split_whitespace().collect();
                        let [source, page, index] = parts[..] else { return Err(format!("bad data-origin {o:?}")) };
                        Some(StrokeOrigin { source: source.into(), page: number(page)?, index: number(index)? })
                    }
                    None => None,
                };
                let pen = match tag.get("data-pen") {
                    Some(p) => number(&p)?,
                    None => pen,
                };
                strokes.push(GlyphStroke { origin, outline: Outline { kind: OutlineKind::Filled, path: BezPath::new() }, skeleton: Vec::new(), placement, pen });
                in_stroke = true;
            }
            ("g", true) => in_stroke = false,
            ("path", false) if in_stroke => {
                let stroke = strokes.last_mut().unwrap();
                match tag.get("class").as_deref() {
                    Some("outline") => {
                        let kind = match tag.get("data-kind").as_deref() {
                            Some("filled") => OutlineKind::Filled,
                            Some("line") => OutlineKind::Line { width: number(&tag.get("data-width").ok_or("a line without data-width")?)? },
                            other => return Err(format!("unknown outline kind {other:?}")),
                        };
                        stroke.outline = Outline { kind, path: tag.path()? };
                    }
                    Some("piece") => stroke.skeleton.push(piece(&tag)?),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    Ok(strokes)
}

fn piece(tag: &Tag) -> Result<SkeletonPiece, String> {
    let radii = tag.get("data-r").unwrap_or_default().split_whitespace().map(number).collect::<Result<Vec<f64>, _>>()?;
    let mut piece = SkeletonPiece::default();
    let mut last = Point::ORIGIN;
    for el in tag.path()?.elements() {
        match *el {
            PathEl::MoveTo(p) => last = p,
            PathEl::CurveTo(c1, c2, p) => {
                piece.spine.push(CubicBez::new(last, c1, c2, p));
                last = p;
            }
            ref other => return Err(format!("unexpected centerline element {other:?}")),
        }
    }
    let centers = std::iter::once(piece.spine.first().map_or(last, |s| s.p0)).chain(piece.spine.iter().map(|s| s.p3));
    piece.circles = centers.zip(radii.iter().copied()).collect();
    if piece.circles.len() != radii.len() {
        return Err("a skeleton piece's radii and centerline do not match".into());
    }
    Ok(piece)
}

fn number<T: std::str::FromStr>(s: &str) -> Result<T, String> {
    s.trim().parse().map_err(|_| format!("bad number {s:?}"))
}

/// `matrix(a b c d e f)`, separated by spaces or commas.
fn matrix(s: &str) -> Result<Affine, String> {
    let inner = s.trim().strip_prefix("matrix(").and_then(|r| r.strip_suffix(')')).ok_or_else(|| format!("bad transform {s:?}"))?;
    let v = inner.split(|c: char| c == ',' || c.is_whitespace()).filter(|p| !p.is_empty()).map(number).collect::<Result<Vec<f64>, _>>()?;
    let [a, b, c, d, e, f] = v[..] else { return Err(format!("bad transform {s:?}")) };
    Ok(Affine::new([a, b, c, d, e, f]))
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

struct Tag {
    name: String,
    close: bool,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn get(&self, key: &str) -> Option<String> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn path(&self) -> Result<BezPath, String> {
        let d = self.get("d").unwrap_or_default();
        BezPath::from_svg(&d).map_err(|e| format!("bad path data: {e:?}"))
    }
}

/// The tags of an XML text, in order (text, comments and declarations skipped).
fn tags(text: &str) -> Result<Vec<Tag>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        let end = after.find('>').ok_or("an unterminated tag")?;
        let inner = after[..end].trim();
        rest = &after[end + 1..];
        if inner.starts_with('?') || inner.starts_with('!') {
            continue;
        }
        let close = inner.starts_with('/');
        let inner = inner.trim_start_matches('/').trim_end_matches('/').trim();
        let name_end = inner.find(char::is_whitespace).unwrap_or(inner.len());
        out.push(Tag { name: inner[..name_end].to_string(), close, attrs: attributes(&inner[name_end..])? });
    }
    Ok(out)
}

fn attributes(mut s: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    while let Some(eq) = s.find('=') {
        let key = s[..eq].trim().to_string();
        let after = s[eq + 1..].trim_start();
        let Some(value) = after.strip_prefix('"') else { return Err(format!("attribute {key} without quotes")) };
        let end = value.find('"').ok_or("an unterminated attribute")?;
        out.push((key, unescape(&value[..end])));
        s = &value[end + 1..];
    }
    Ok(out)
}
