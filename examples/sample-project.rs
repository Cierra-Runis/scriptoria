//! Makes a project from the sample characters of `test/test.pdf`: each character's strokes
//! assigned to its glyph, as the app does (placed as the installed reference font places the
//! character, if one is found). For trying the app and for screenshots. With `--parts`, 你's 亻
//! is kept as a part and put into 他, 们 and 休 (added, empty before).
//!
//! `cargo run --release --example sample-project -- <folder> [--parts]`

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
    if std::env::args().any(|a| a == "--parts") {
        let table = scriptoria_ids::Table::builtin();
        let ni = project.glyph_for('你').unwrap();
        let part = project.make_part(ni, &[0, 1], "亻").unwrap();
        let region = scriptoria::components::region_in(table, &project, &['你'], part);
        project.part_mut(part).unwrap().region = region;
        project.add_chars("他们休");
        for c in ['他', '们', '休'] {
            let g = project.glyph_for(c).unwrap();
            scriptoria::components::put_into(table, &mut project, part, g);
        }
        println!("part 亻 (region {region:?}) used in {} glyphs", project.glyphs_using(part).len());
    }
    project.save(&folder).expect("save");
    println!("{} glyphs → {}", project.glyphs.len(), folder.display());
}
