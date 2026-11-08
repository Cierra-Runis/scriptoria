//! How large the sample's handwriting is, as the reference font measures it: for each character
//! of the sample, the em (in page points) at which the handwritten character would be as large as
//! the reference font's (by the square root of the ink boxes' areas, and by their heights). The
//! medians are what a project's points per em (and so its practice sheets' squares) should be for
//! this hand.
//!
//! `cargo run --example handwriting-size`

#[path = "common/mod.rs"]
mod common;

use common::{CHARACTERS, strokes_in, test_pdf};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let (_, reference) = scriptoria::assign::system_reference().expect("a reference font installed (Source Han Sans)");
    let doc = test_pdf();
    let pages: Vec<_> = (0..doc.page_count()).map(|i| doc.page(i).unwrap()).collect();
    let mut groups: Vec<(&str, Vec<f64>, Vec<f64>)> = vec![("汉字", vec![], vec![]), ("假名", vec![], vec![]), ("拉丁", vec![], vec![])];
    println!("字  手写 宽×高 (pt)   参考 宽×高 (em/1000)   每 em 磅数：按面积  按高度");
    for ch in CHARACTERS {
        let c = ch.char();
        let strokes = strokes_in(&pages[ch.1], ch.rect());
        let Some(hand) = strokes.iter().map(|s| s.bbox).reduce(|a, b| a.union(b)) else { continue };
        let Some(model) = reference.glyph(c, 1000.0) else { continue };
        let r = model.bounds;
        let by_area = (hand.area() / r.area()).sqrt() * 1000.0;
        let by_height = hand.height() / r.height() * 1000.0;
        println!("{c}  {:5.2}×{:5.2}         {:4.0}×{:4.0}                {by_area:5.2}   {by_height:5.2}", hand.width(), hand.height(), r.width(), r.height());
        let g = if scriptoria_template::is_square(c) { if (c as u32) < 0x3100 { 1 } else { 0 } } else { 2 };
        groups[g].1.push(by_area);
        groups[g].2.push(by_height);
    }
    println!();
    let all: Vec<f64> = groups.iter().flat_map(|g| g.1.clone()).collect();
    for (name, area, height) in groups {
        if !area.is_empty() {
            println!("{name}：按面积 {:.2} pt/em，按高度 {:.2} pt/em（{} 个字）", median(area.clone()), median(height), area.len());
        }
    }
    println!("全部：按面积 {:.2} pt/em", median(all));
}
