//! A sheet written as a PDF and read back as a handwriting app's page would be (with the ink
//! crate): everything drawn is in the sheet's colors, the characters in the squares faint, and
//! each page's code says which page it is.

use kurbo::{BezPath, Rect};
use scriptoria_ink::InkDocument;
use scriptoria_template::{CODE, FRAME, Grid, Heights, LABEL, MODEL, Model, Sheet, is_sheet_color, read_code};

/// A box with an arched top for every character: 800 wide on an advance of 1000, from the
/// baseline up to 700 (800 at the top of the arch).
fn model(_: char) -> Option<Model> {
    let mut p = BezPath::new();
    p.move_to((100.0, 0.0));
    p.line_to((100.0, 700.0));
    p.quad_to((500.0, 900.0), (900.0, 700.0));
    p.line_to((900.0, 0.0));
    p.close_path();
    Some(Model { outline: p, advance: 1000.0 })
}

fn rgb(s: &scriptoria_ink::Stroke) -> [u8; 3] {
    [s.rgba[0], s.rgba[1], s.rgba[2]]
}

#[test]
fn sheets_read_back_page_by_page() {
    // Squares as large as handwriting is written.
    for grid in [Grid::default(), Grid::new(4.5, 8, 10, 0.12)] {
        let chars: Vec<char> = "天地玄黄宇宙洪荒".chars().cycle().take(grid.per_page() + 5).collect();
        let sheet = Sheet { grid, id: 42, chars: &chars, models: &model, heights: Heights::default() };
        let doc = InkDocument::open(sheet.pdf()).expect("a PDF");
        assert_eq!(doc.page_count(), 2);
        for page in 0..2 {
            let ink = doc.page(page).unwrap();
            assert!((ink.width - grid.width).abs() < 0.01 && (ink.height - grid.height).abs() < 0.01);
            assert!(ink.strokes.iter().all(|s| is_sheet_color(rgb(s))), "only the sheet's colors");
            let marks: Vec<Rect> = ink.strokes.iter().filter(|s| rgb(s) == CODE).map(|s| s.bbox).collect();
            assert_eq!(read_code(&grid, (ink.width, ink.height), &marks).map(|f| (f.id, f.page)), Some((42, page)));
            let cells = if page == 0 { grid.per_page() } else { 5 };
            for color in [FRAME, LABEL, MODEL] {
                assert_eq!(ink.strokes.iter().filter(|s| rgb(s) == color).count(), cells, "{color:?}");
            }
            // Each square is where the grid says, its character in it, faint.
            let first = grid.cell(page * grid.per_page()).square;
            assert!(ink.strokes.iter().any(|s| rgb(s) == FRAME && (s.bbox.center() - first.center()).hypot() < 0.01 * grid.cell));
            let shown = ink.strokes.iter().find(|s| rgb(s) == MODEL && first.contains(s.bbox.center())).expect("the character in its square");
            assert_eq!(shown.rgba[3], 26, "drawn at a tenth");
            // From the baseline (0.12 of the square up) to 900 units of the em above it.
            let unit = grid.cell / 1000.0;
            let base = first.y1 - 0.12 * grid.cell;
            assert!((shown.bbox.y1 - base).abs() < 0.02 * grid.cell && (shown.bbox.y0 - (base - 800.0 * unit)).abs() < 0.02 * grid.cell, "{shown:?}");
            assert!((shown.bbox.x0 - (first.x0 + 100.0 * unit)).abs() < 0.02 * grid.cell);
        }
    }
}
