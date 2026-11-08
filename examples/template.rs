//! Writes a practice sheet for some characters, as the app does (the characters shown in the
//! installed reference font), to look at or print: `target/scriptoria-template.pdf`.
//!
//! `cargo run --example template [-- 天地玄黄宇宙洪荒AaBb]`

use std::path::PathBuf;

use scriptoria_project::Project;

fn main() {
    let text = std::env::args().nth(1).unwrap_or_else(|| "天地玄黄宇宙洪荒日月盈昃辰宿列张あいうえおAaBbGgQq0123".into());
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut project = Project::new("Sample");
    let template = project.new_template(&chars).clone();
    let reference = scriptoria::assign::system_reference().map(|(_, f)| f);
    let pdf = scriptoria::template::sheet_pdf(&project.font, &template, reference.as_ref());
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/scriptoria-template.pdf");
    std::fs::write(&path, pdf).expect("write");
    println!("{} characters, {} page(s) → {}", chars.len(), template.grid.pages(chars.len()), path.display());
}
