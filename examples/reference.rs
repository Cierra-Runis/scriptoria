//! Shows what the reference font says about some characters: the face chosen (of a collection) and
//! the ideographic face its BASE table gives, and each character's box and advance at 1000 units
//! per em.
//!
//! `cargo run --example reference [-- <font file>] [-- 世一口国A]`

use std::path::PathBuf;

use scriptoria_reference::{ReferenceFont, faces, system_default};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = args.iter().find(|a| a.contains('.')).map(PathBuf::from).or_else(system_default).expect("no reference font found");
    let text = args.iter().find(|a| !a.contains('.')).cloned().unwrap_or_else(|| "世一口国小のあAgx".into());
    let all = faces(&file).expect("faces");
    println!("{}: {} faces", file.display(), all.len());
    let font = ReferenceFont::open(&file, None).expect("font");
    println!("using face {} ({} {})", font.face().index, font.face().family, font.face().style);
    match font.ideographic_face(1000.0) {
        Some((bottom, top)) => println!("ideographic face (BASE icfb..icft): {bottom:.0}..{top:.0}"),
        None => println!("no ideographic face in BASE"),
    }
    for c in text.chars() {
        match font.glyph(c, 1000.0) {
            Some(g) => {
                let b = g.bounds;
                println!("{c}  box x {:4.0}..{:4.0}  y {:4.0}..{:4.0}  ({:3.0} × {:3.0})  advance {:4.0}", b.x0, b.x1, b.y0, b.y1, b.width(), b.height(), g.advance);
            }
            None => println!("{c}  —"),
        }
    }
}
