//! Practice sheets for a project's characters (see the template crate), and the pages written on
//! them put into the glyphs cell by cell.
//!
//! On a written sheet every character's cell maps to the em the same way (the square is the em,
//! descender to ascender), so the characters keep the sizes and places the writer gave them in
//! their squares, and their strokes keep the pen's width.

use std::collections::BTreeMap;

use scriptoria_ink::kurbo::{Affine, Point, Rect};
use scriptoria_ink::{InkPage, Stroke};
use scriptoria_project::{FontInfo, Glyph, GlyphStroke, Project, StrokeOrigin, Template};
use scriptoria_reference::ReferenceFont;
use scriptoria_template::{CODE, is_sheet_color, is_square, read_code, write_pdf};

/// A sheet's PDF, each character shown above its square in the reference font (left blank
/// without one).
pub fn sheet_pdf(template: &Template, reference: Option<&ReferenceFont>) -> Vec<u8> {
    let chars: Vec<char> = template.chars.chars().collect();
    write_pdf(&template.grid, template.id, &chars, |c| reference.and_then(|r| r.glyph(c, 1000.0)).map(|g| g.outline))
}

fn rgb(s: &Stroke) -> [u8; 3] {
    [s.rgba[0], s.rgba[1], s.rgba[2]]
}

/// Whether a stroke of a page is a sheet's own drawing rather than handwriting.
pub fn is_sheet_stroke(s: &Stroke) -> bool {
    is_sheet_color(rgb(s))
}

/// Which of the project's sheets a written page is: (the sheet's index in the project, its page).
pub fn recognize(project: &Project, page: &InkPage) -> Option<(usize, usize)> {
    let marks: Vec<Rect> = page.strokes.iter().filter(|s| rgb(s) == CODE).map(|s| s.bbox).collect();
    if marks.is_empty() {
        return None;
    }
    project.templates.iter().enumerate().find_map(|(i, t)| {
        let found = read_code(&t.grid, (page.width, page.height), &marks)?;
        (found.id == t.id && found.page < t.grid.pages(t.chars.chars().count())).then_some((i, found.page))
    })
}

/// A character written in a sheet's cell, ready to become its glyph.
#[derive(Clone, Debug)]
pub struct Written {
    pub c: char,
    pub strokes: Vec<GlyphStroke>,
    pub advance: f64,
}

/// The characters written on a sheet's page (`sheet_page` of `template`), which is page
/// `page_index` of `source`: the handwriting in each cell (a stroke belongs to the cell its box's
/// center lies in), in writing order, its square made the em. Characters written in squares (CJK)
/// are an em wide; others keep their height and take the reference's side bearings (a tenth of the
/// em each without a reference). Skeletons are recovered on all cores.
pub fn read_sheet_page(
    info: &FontInfo,
    template: &Template,
    sheet_page: usize,
    source: &str,
    page_index: usize,
    page: &InkPage,
    reference: Option<&ReferenceFont>,
) -> Vec<Written> {
    let grid = &template.grid;
    let chars: Vec<char> = template.chars.chars().collect();
    // A page of another size is the sheet scaled to it.
    let (sx, sy) = (page.width / grid.width, page.height / grid.height);
    let mut cells: BTreeMap<usize, Vec<&Stroke>> = BTreeMap::new();
    for s in page.strokes.iter().filter(|s| !is_sheet_stroke(s)) {
        let c = s.bbox.center();
        if let Some(cell) = grid.cell_at(sheet_page, chars.len(), Point::new(c.x / sx, c.y / sy)) {
            cells.entry(cell.index).or_default().push(s);
        }
    }
    let cells: Vec<(usize, Vec<&Stroke>)> = cells.into_iter().collect();
    let em = (info.ascender - info.descender) as f64;
    let upm = info.units_per_em as f64;
    crate::par_map(&cells, |(index, strokes)| {
        let c = chars[*index];
        let sq = grid.cell(*index).square;
        let square = Rect::new(sq.x0 * sx, sq.y0 * sy, sq.x1 * sx, sq.y1 * sy);
        let k = em / square.height();
        let placement = Affine::new([k, 0.0, 0.0, -k, -square.x0 * k, square.y1 * k + info.descender as f64]);
        let strokes = strokes
            .iter()
            .map(|s| crate::assign::glyph_stroke(s, Some(StrokeOrigin { source: source.to_string(), page: page_index, index: s.order }), placement, k))
            .collect();
        let mut glyph = Glyph { name: String::new(), chars: vec![c], advance: upm, strokes };
        if !is_square(c) {
            let frame = reference.and_then(|r| crate::assign::reference_frame(r, &glyph, info));
            let (left, right) = frame.map_or((0.1 * upm, 0.1 * upm), |f| (f.bounds.x0, f.advance - f.bounds.x1));
            glyph.set_side_bearings(left, right);
        }
        Written { c, strokes: glyph.strokes, advance: glyph.advance }
    })
}
