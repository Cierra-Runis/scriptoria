//! Makes a variable font (`wght` 100–900) from the sample characters of `test/test.pdf` and a page
//! to try it in a browser: `target/scriptoria-font/ScriptoriaSample.ttf` and `index.html` (the
//! font embedded, a weight slider). With a weight given (`wght=650`), only that weight is shown
//! and the slider is left out, for screenshots.
//!
//! `cargo run --release --example export-font [-- wght=650]`

mod common;

use std::path::PathBuf;

use scriptoria::font::{AXIS, Layout, glyph};
use scriptoria_font::{FontSource, build};

fn main() {
    let fixed: Option<f64> = std::env::args().skip(1).find_map(|a| a.strip_prefix("wght=").map(|w| w.parse().expect("weight")));
    let doc = common::test_pdf();
    let pages: Vec<_> = (0..doc.page_count()).map(|i| doc.page(i).unwrap()).collect();
    let layout = Layout::default();
    let glyphs: Vec<_> = scriptoria::par_map(common::CHARACTERS, |c| glyph(c.char(), &common::strokes_in(&pages[c.1], c.rect()), c.rect(), &layout));
    let source = FontSource {
        family: "Scriptoria Sample".into(),
        units_per_em: layout.units_per_em,
        ascender: 880,
        descender: layout.descender,
        axis: AXIS,
        instances: vec![("Thin".into(), 100.0), ("Light".into(), 250.0), ("Regular".into(), 400.0), ("Bold".into(), 650.0), ("Black".into(), 900.0)],
        glyphs,
        accuracy: 0.5,
        tolerance: layout.tolerance,
    };
    let bytes = build(&source).expect("font");
    let masters: usize = source.glyphs.iter().map(|g| g.masters.len()).sum();
    println!("{} glyphs, {:.1} masters per glyph, {} bytes", source.glyphs.len(), masters as f64 / source.glyphs.len() as f64, bytes.len());

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/scriptoria-font");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ScriptoriaSample.ttf"), &bytes).unwrap();
    let text: String = common::CHARACTERS.iter().map(|c| c.0).collect();
    std::fs::write(dir.join("index.html"), page(&bytes, &text, fixed)).unwrap();
    println!("{}", dir.join("index.html").display());
}

fn page(font: &[u8], text: &str, fixed: Option<f64>) -> String {
    let weights = [100, 250, 400, 650, 900];
    let rows: String = match fixed {
        Some(w) => format!("<p class=\"big\" style=\"font-variation-settings: 'wght' {w}\">{text}</p>"),
        None => weights.iter().map(|w| format!("<p style=\"font-variation-settings: 'wght' {w}\"><span>{w}</span>{text}</p>\n")).collect(),
    };
    let slider = if fixed.is_some() {
        String::new()
    } else {
        format!(
            "<label>wght <input id=\"w\" type=\"range\" min=\"100\" max=\"900\" value=\"400\"> <output id=\"v\">400</output></label>\n<p id=\"live\" class=\"big\">{text}</p>\n<script>const w=document.getElementById('w'),v=document.getElementById('v'),l=document.getElementById('live');w.oninput=()=>{{v.textContent=w.value;l.style.fontVariationSettings=`'wght' ${{w.value}}`}};</script>"
        )
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><title>Scriptoria sample font</title>
<style>
@font-face {{ font-family: 'Scriptoria Sample'; src: url(data:font/ttf;base64,{}) format('truetype'); font-weight: 100 900; }}
body {{ font-family: system-ui, sans-serif; margin: 24px; background: #fff; color: #111; }}
p {{ font-family: 'Scriptoria Sample'; font-size: 56px; margin: 8px 0; line-height: 1.3; }}
p span {{ font-family: system-ui, sans-serif; font-size: 14px; color: #888; display: inline-block; width: 40px; }}
.big {{ font-size: 96px; }}
</style>
{slider}
{rows}",
        base64(font)
    )
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
