//! Handwritten characters (`tests/data/glyphs.svg`: each glyph's outline at the masters it needs,
//! in font units, `wght` values) made into a font and read back: the outlines at and between the
//! masters, contour by contour, with their start points and directions. Writes the `font-glyphs`
//! report (the glyphs drawn from the font at several weights).

mod common;

use std::path::PathBuf;

use common::*;
use kurbo::{Rect, Shape};
use scriptoria_font::{Axis, FontSource, Glyph, build};
use scriptoria_testkit::data;
use scriptoria_testkit::report::{Panel, Report, Svg};

/// The strokes as written are the boldest weight (900).
const AXIS: Axis = Axis { min: 100.0, default: 400.0, max: 900.0 };
/// Largest distance of the quadratic splines from the cubics (font units).
const ACCURACY: f64 = 0.5;
/// Allowed on top of that: rounding to whole units (up to 0.71 at a master, as much between
/// them) and the deltas left for the renderer to infer (IUP, within half a unit).
const ROUNDING: f64 = 1.5;
/// How far a contour may stray from a master before it keeps its own deltas there: what the
/// masters were chosen for (twice the outlines' precision, 0.004 pt at 4.5 pt per em).
const TOLERANCE: f64 = 1.8;

fn source() -> FontSource {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/glyphs.svg");
    let glyphs = data::read_glyphs(&path).into_iter().map(|g| Glyph { name: g.name, chars: g.chars, advance: g.advance, masters: g.masters }).collect();
    FontSource {
        family: "Scriptoria Glyphs".into(),
        units_per_em: 1000,
        ascender: 880,
        descender: -120,
        axis: AXIS,
        instances: vec![("Thin".into(), 100.0), ("Regular".into(), 400.0), ("Bold".into(), 700.0), ("Black".into(), 900.0)],
        glyphs,
        accuracy: ACCURACY,
        tolerance: TOLERANCE,
    }
}

#[test]
fn glyphs_come_back_from_the_font() {
    let source = source();
    let bytes = build(&source).expect("build");
    let font = font(&bytes);
    let mut failures = Vec::new();
    let (mut worst, mut masters, mut contours_total) = (0.0f64, 0usize, 0usize);
    for g in &source.glyphs {
        let id = gid(&font, g.chars[0]);
        masters += g.masters.len();
        if !overlap_flag(&font, id) {
            failures.push(format!("{}: no OVERLAP_SIMPLE", g.name));
        }
        let mut weights: Vec<f64> = g.masters.iter().map(|m| m.0).collect();
        weights.extend(g.masters.windows(2).map(|m| 0.5 * (m[0].0 + m[1].0)));
        for wght in weights {
            let want = contours(&expected(&g.masters, wght));
            let got = contours(&outline(&font, id, wght));
            if got.len() != want.len() {
                failures.push(format!("{} at {wght}: {} contours instead of {}", g.name, got.len(), want.len()));
                continue;
            }
            for (i, (a, b)) in want.iter().zip(&got).enumerate() {
                let d = distance(a, b);
                worst = worst.max(d);
                if d > ACCURACY + ROUNDING + TOLERANCE {
                    failures.push(format!("{} at {wght}, contour {i}: {d:.2} units off", g.name));
                }
                if (start(a) - start(b)).hypot() > ROUNDING + TOLERANCE {
                    failures.push(format!("{} at {wght}, contour {i}: starts elsewhere", g.name));
                }
                if (area(a) < 0.0) != (area(b) < 0.0) {
                    failures.push(format!("{} at {wght}, contour {i}: direction changed", g.name));
                }
            }
        }
        // TrueType's direction for outer contours: clockwise with y up (negative kurbo area).
        let default = &g.masters.iter().find(|m| m.0 == AXIS.default).unwrap().1;
        for (i, c) in contours(default).iter().enumerate() {
            contours_total += 1;
            if area(c) >= 0.0 {
                failures.push(format!("{}: contour {i} runs counter-clockwise", g.name));
            }
        }
    }
    let n = source.glyphs.len();
    let line = format!(
        "{n} glyphs, {contours_total} contours · {:.1} masters per glyph · {} bytes ({:.0} per glyph) · worst distance {worst:.2} units",
        masters as f64 / n as f64,
        bytes.len(),
        bytes.len() as f64 / n as f64
    );
    eprintln!("{line}");
    for tag in [b"glyf", b"gvar", b"loca", b"hmtx", b"cmap", b"name"] {
        let len = skrifa::raw::TableProvider::data_for_tag(&font, skrifa::Tag::new(tag)).map_or(0, |d| d.len());
        eprintln!("  {}: {len} bytes", String::from_utf8_lossy(tag));
    }
    write_report(&source, &bytes, line);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The glyphs as the font draws them, at several weights.
fn write_report(source: &FontSource, bytes: &[u8], summary: String) {
    let font = font(bytes);
    let mut report = Report::new("font-glyphs", "font · glyphs read back from the font");
    report.summary(summary);
    for wght in [100.0, 250.0, 400.0, 650.0, 900.0] {
        let section = report.section(format!("wght {wght}"), "black: the glyph drawn from the font (non-zero fill) · gray: the em from descender to ascender");
        for g in &source.glyphs {
            let path = outline(&font, gid(&font, g.chars[0]), wght);
            // y up in the font, y down in SVG.
            let flipped = kurbo::Affine::scale_non_uniform(1.0, -1.0) * path;
            let em = Rect::new(0.0, -(source.ascender as f64), g.advance, -(source.descender as f64));
            let view = em.union(flipped.bounding_box()).inflate(20.0, 20.0);
            let side = view.width().max(view.height());
            let mut svg = Svg::new(Rect::from_center_size(view.center(), (side, side)), 160.0);
            svg.path(&em.to_path(0.1), "#f3f4f6", "none", 0.0);
            svg.path(&flipped, "#111827", "none", 0.0);
            section.panels.push(Panel { caption: format!("{} {wght}", g.chars.iter().collect::<String>()), svg: svg.render(), flagged: false });
        }
    }
    report.write();
}
