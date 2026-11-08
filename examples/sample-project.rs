//! Makes a project from the sample characters of `test/test.pdf`: each character's strokes
//! assigned to its glyph, as the app does (placed as the installed reference font places the
//! character, if one is found). For trying the app and for screenshots.
//!
//! `cargo run --release --example sample-project -- <folder>`

mod common;

use std::path::PathBuf;

use scriptoria_project::Project;

fn main() {
    let folder = PathBuf::from(std::env::args().nth(1).expect("a folder to write the project to"));
    let doc = common::test_pdf();
    let pages: Vec<_> = (0..doc.page_count()).map(|i| doc.page(i).unwrap()).collect();
    let mut project = Project::new("Scriptoria Sample");
    let reference = scriptoria::assign::system_reference();
    if let Some((r, font)) = &reference {
        println!("reference: {} ({} {})", r.file.display(), font.face().family, font.face().style);
        project.font.reference = Some(r.clone());
    }
    project.add_chars(&common::CHARACTERS.iter().map(|c| c.0).collect::<String>());
    for c in common::CHARACTERS {
        let glyph = project.glyph_for(c.char()).unwrap();
        let strokes = common::strokes_in(&pages[c.1], c.rect());
        scriptoria::assign::assign(&mut project, glyph, &strokes, reference.as_ref().map(|(_, f)| f));
    }
    project.save(&folder).expect("save");
    println!("{} glyphs → {}", project.glyphs.len(), folder.display());
}
