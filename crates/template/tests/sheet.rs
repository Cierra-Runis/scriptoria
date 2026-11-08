//! A sheet written as a PDF and read back as a handwriting app's page would be (with the ink
//! crate): everything drawn is in the sheet's colors, and each page's code says which page it is.

use kurbo::{BezPath, Rect};
use scriptoria_ink::InkDocument;
use scriptoria_template::{CODE, FRAME, Grid, LABEL, is_sheet_color, read_code, write_pdf};

/// A box for every character, as its label.
fn label(_: char) -> Option<BezPath> {
    let mut p = BezPath::new();
    p.move_to((100.0, 0.0));
    p.line_to((100.0, 700.0));
    p.quad_to((500.0, 900.0), (900.0, 700.0));
    p.line_to((900.0, 0.0));
    p.close_path();
    Some(p)
}

#[test]
fn sheets_read_back_page_by_page() {
    let grid = Grid::default();
    let chars: Vec<char> = "天地玄黄宇宙洪荒".chars().cycle().take(grid.per_page() + 5).collect();
    let pdf = write_pdf(&grid, 42, &chars, label);
    let doc = InkDocument::open(pdf).expect("a PDF");
    assert_eq!(doc.page_count(), 2);
    for page in 0..2 {
        let ink = doc.page(page).unwrap();
        assert!((ink.width - grid.width).abs() < 0.01 && (ink.height - grid.height).abs() < 0.01);
        let rgb = |s: &scriptoria_ink::Stroke| [s.rgba[0], s.rgba[1], s.rgba[2]];
        assert!(ink.strokes.iter().all(|s| is_sheet_color(rgb(s))), "only the sheet's colors");
        let marks: Vec<Rect> = ink.strokes.iter().filter(|s| rgb(s) == CODE).map(|s| s.bbox).collect();
        assert_eq!(read_code(&grid, (ink.width, ink.height), &marks).map(|f| (f.id, f.page)), Some((42, page)));
        let cells = if page == 0 { grid.per_page() } else { 5 };
        assert_eq!(ink.strokes.iter().filter(|s| rgb(s) == FRAME).count(), cells);
        assert_eq!(ink.strokes.iter().filter(|s| rgb(s) == LABEL).count(), cells);
        // Each square is where the grid says.
        let first = grid.cell(page * grid.per_page()).square;
        assert!(ink.strokes.iter().any(|s| rgb(s) == FRAME && (s.bbox.center() - first.center()).hypot() < 0.5));
    }
}
