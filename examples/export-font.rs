//! Makes a variable font (`wght` 100–400, the strokes as written at 400) from the sample
//! characters of `test/test.pdf` and a page to try it in a browser:
//! `target/scriptoria-font/ScriptoriaSample.ttf` and `index.html` (the font embedded, a weight
//! slider). With a weight given (`wght=250`), only that weight is shown and the slider is left
//! out, for screenshots.
//!
//! `cargo run --release --example export-font [-- wght=250]`

mod common;

use std::path::PathBuf;

use scriptoria::font::{AXIS, INSTANCES, Layout, glyph, preview_page};
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
        instances: INSTANCES.iter().map(|&(name, w)| (name.to_string(), w)).collect(),
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
    std::fs::write(dir.join("index.html"), preview_page(&bytes, &source.family, &text, fixed)).unwrap();
    println!("{}", dir.join("index.html").display());
}
