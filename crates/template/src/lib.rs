//! Practice sheets: pages of cells to write characters in, one character to a cell, printed or
//! opened in Goodnotes as a notebook template; and, on the pages written, finding the sheet again.
//!
//! A cell is a writing square (the em, descender to ascender) under a strip showing the character
//! to write. The squares have guides: for CJK characters the middle lines (田), for others the
//! baseline. Each page carries a code at its foot, a row of small squares, saying which sheet and
//! which page it is, so that the strokes written on it can be put into their characters' glyphs
//! by the cells they lie in.
//!
//! Pages are vector drawings in colors of their own ([`is_sheet_color`]), which nothing written on
//! them is expected to use: a reader tells the sheet's drawing from the handwriting by its color.
//! Page coordinates here are as in the ink crate: points, the origin at the top left, y down.

use kurbo::{BezPath, PathEl, Point, Rect};
pub use pdf_writer;
use pdf_writer::{Content, Pdf, Rect as PdfRect, Ref};
use serde::{Deserialize, Serialize};

/// The colors of the sheet's drawing (RGB): the squares, their guides, the characters to write,
/// the code.
pub const FRAME: [u8; 3] = [150, 184, 226];
pub const GUIDE: [u8; 3] = [198, 216, 240];
pub const LABEL: [u8; 3] = [140, 146, 158];
pub const CODE: [u8; 3] = [38, 58, 104];

/// Whether something drawn in this color is the sheet's own.
pub fn is_sheet_color([r, g, b]: [u8; 3]) -> bool {
    [FRAME, GUIDE, LABEL, CODE].contains(&[r, g, b])
}

/// The code at a page's foot: a start square, the sheet's id (12 bits), the page (10 bits), an
/// end square.
const ID_BITS: usize = 12;
const PAGE_BITS: usize = 10;
const CODE_SQUARES: usize = 1 + ID_BITS + PAGE_BITS + 1;
const CODE_SIZE: f64 = 5.0;
const CODE_STEP: f64 = 8.0;

/// The largest sheet id and page a code can hold.
pub const MAX_ID: u32 = (1 << ID_BITS) - 1;
pub const MAX_PAGE: usize = (1 << PAGE_BITS) - 1;

/// How a sheet's pages are laid out (points).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub width: f64,
    pub height: f64,
    /// Space around the cells; the code sits in the bottom one.
    pub margin: f64,
    /// A writing square's side: the em, descender to ascender.
    pub cell: f64,
    /// The strip above a square showing its character.
    pub label: f64,
    /// Space between cells.
    pub gap: f64,
    /// Where the baseline is in a square, as a fraction of its side from its bottom (the
    /// descender's share of the em).
    pub baseline: f64,
}

impl Default for Grid {
    /// A4, eight cells across.
    fn default() -> Self {
        Grid { width: 595.276, height: 841.89, margin: 36.0, cell: 54.0, label: 16.0, gap: 8.0, baseline: 0.12 }
    }
}

/// One cell of a sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub page: usize,
    /// Which character of the sheet it holds.
    pub index: usize,
    /// Where to write.
    pub square: Rect,
    /// Where the character to write is shown.
    pub label: Rect,
}

impl Grid {
    pub fn columns(&self) -> usize {
        (((self.width - 2.0 * self.margin + self.gap) / (self.cell + self.gap)).floor() as usize).max(1)
    }

    pub fn rows(&self) -> usize {
        let room = self.height - 2.0 * self.margin + self.gap;
        ((room / (self.label + self.cell + self.gap)).floor() as usize).max(1)
    }

    pub fn per_page(&self) -> usize {
        self.columns() * self.rows()
    }

    /// The pages `n` characters take.
    pub fn pages(&self, n: usize) -> usize {
        n.div_ceil(self.per_page()).max(1)
    }

    /// The cell of the sheet's `index`-th character.
    pub fn cell(&self, index: usize) -> Cell {
        let (page, at) = (index / self.per_page(), index % self.per_page());
        let (row, column) = (at / self.columns(), at % self.columns());
        // The cells centered across the page.
        let used = self.columns() as f64 * (self.cell + self.gap) - self.gap;
        let x = (self.width - used) / 2.0 + column as f64 * (self.cell + self.gap);
        let y = self.margin + row as f64 * (self.label + self.cell + self.gap);
        let label = Rect::new(x, y, x + self.cell, y + self.label);
        let square = Rect::new(x, y + self.label, x + self.cell, y + self.label + self.cell);
        Cell { page, index, square, label }
    }

    /// The cell whose square holds `p` on `page`, if any (of `n` characters). A little ink over a
    /// square's edge, into the gap around it, still counts.
    pub fn cell_at(&self, page: usize, n: usize, p: Point) -> Option<Cell> {
        let first = page * self.per_page();
        (first..n.min(first + self.per_page())).map(|i| self.cell(i)).find(|c| c.square.inflate(self.gap / 2.0, self.gap / 2.0).contains(p))
    }

    /// The code's squares, left to right.
    fn code_squares(&self) -> Vec<Rect> {
        let y = self.height - self.margin / 2.0 - CODE_SIZE / 2.0;
        (0..CODE_SQUARES)
            .map(|i| {
                let x = self.margin + i as f64 * CODE_STEP;
                Rect::new(x, y, x + CODE_SIZE, y + CODE_SIZE)
            })
            .collect()
    }
}

/// Which squares of the code are drawn for a sheet's page.
fn code_bits(id: u32, page: usize) -> Vec<bool> {
    let mut bits = vec![true];
    bits.extend((0..ID_BITS).map(|i| id >> i & 1 == 1));
    bits.extend((0..PAGE_BITS).map(|i| page >> i & 1 == 1));
    bits.push(true);
    bits
}

/// A page found to be a sheet's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub id: u32,
    pub page: usize,
}

/// Reads a page's code from the boxes of what is drawn on it in the code's color (page
/// coordinates; a page of another size than the grid's is taken as the sheet scaled to it).
pub fn read_code(grid: &Grid, page_size: (f64, f64), marks: &[Rect]) -> Option<Found> {
    let (sx, sy) = (grid.width / page_size.0, grid.height / page_size.1);
    let centers: Vec<Point> = marks.iter().map(|r| Point::new(r.center().x * sx, r.center().y * sy)).collect();
    let bits: Vec<bool> = grid.code_squares().iter().map(|sq| centers.iter().any(|c| sq.inflate(1.5, 1.5).contains(*c))).collect();
    if !bits[0] || !bits[CODE_SQUARES - 1] {
        return None;
    }
    let number = |bits: &[bool]| bits.iter().enumerate().fold(0usize, |n, (i, &b)| n | (b as usize) << i);
    Some(Found { id: number(&bits[1..1 + ID_BITS]) as u32, page: number(&bits[1 + ID_BITS..1 + ID_BITS + PAGE_BITS]) })
}

/// Whether a character is written in a square of its own (CJK and the like): such cells get
/// middle lines, others a baseline.
pub fn is_square(c: char) -> bool {
    matches!(c as u32, 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFF60 | 0x20000..=0x3FFFF)
}

/// The sheet as a PDF: `chars` in cells, page after page, the code of sheet `id` on each. `label`
/// gives a character's outline to show above its square (font units of an em of 1000, y up, the
/// baseline at 0, the em from `-baseline × 1000` up); characters without one get an empty strip.
pub fn write_pdf(grid: &Grid, id: u32, chars: &[char], label: impl Fn(char) -> Option<BezPath>) -> Vec<u8> {
    assert!(id <= MAX_ID, "sheet id {id} too large");
    let pages = grid.pages(chars.len());
    assert!(pages <= MAX_PAGE + 1, "too many pages");
    let mut pdf = Pdf::new();
    let (catalog, tree) = (Ref::new(1), Ref::new(2));
    let page_ref = |p: usize| Ref::new(3 + 2 * p as i32);
    let content_ref = |p: usize| Ref::new(4 + 2 * p as i32);
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids((0..pages).map(page_ref)).count(pages as i32);
    for page in 0..pages {
        let mut c = Content::new();
        draw_page(&mut c, grid, id, chars, page, &label);
        pdf.page(page_ref(page))
            .parent(tree)
            .media_box(PdfRect::new(0.0, 0.0, grid.width as f32, grid.height as f32))
            .contents(content_ref(page))
            .resources();
        pdf.stream(content_ref(page), &c.finish());
    }
    pdf.finish()
}

/// Draws page `page` of the sheet (see [`write_pdf`]) into a content stream: for a PDF of one's
/// own, or to write on.
pub fn draw_page(c: &mut Content, grid: &Grid, id: u32, chars: &[char], page: usize, label: &impl Fn(char) -> Option<BezPath>) {
    let rgb = |[r, g, b]: [u8; 3]| (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    // Page coordinates (y down) to PDF (y up).
    let h = grid.height;
    let at = |p: Point| (p.x as f32, (h - p.y) as f32);
    for index in page * grid.per_page()..chars.len().min((page + 1) * grid.per_page()) {
        let cell = grid.cell(index);
        let ch = chars[index];
        let sq = cell.square;
        // The square.
        let (r, g, b) = rgb(FRAME);
        c.set_stroke_rgb(r, g, b);
        c.set_line_width(0.75);
        let (x0, y0) = at(Point::new(sq.x0, sq.y1));
        c.rect(x0, y0, sq.width() as f32, sq.height() as f32);
        c.stroke();
        // Its guides, dashed.
        let (r, g, b) = rgb(GUIDE);
        c.set_stroke_rgb(r, g, b);
        c.set_line_width(0.5);
        c.set_dash_pattern([2.0, 2.0], 0.0);
        let lines: Vec<(Point, Point)> = if is_square(ch) {
            let m = sq.center();
            vec![(Point::new(sq.x0, m.y), Point::new(sq.x1, m.y)), (Point::new(m.x, sq.y0), Point::new(m.x, sq.y1))]
        } else {
            let base = sq.y1 - grid.baseline * sq.height();
            // The baseline, and halfway from it to the top (about the lowercase letters' height).
            let mid = base - (base - sq.y0) * 0.55;
            vec![(Point::new(sq.x0, base), Point::new(sq.x1, base)), (Point::new(sq.x0, mid), Point::new(sq.x1, mid))]
        };
        for (a, b) in lines {
            let (a, b) = (at(a), at(b));
            c.move_to(a.0, a.1);
            c.line_to(b.0, b.1);
        }
        c.stroke();
        c.set_dash_pattern([], 0.0);
        // The character to write, at the strip's left.
        if let Some(outline) = label(ch) {
            let (r, g, b) = rgb(LABEL);
            c.set_fill_rgb(r, g, b);
            let k = cell.label.height() * 0.9 / 1000.0;
            let origin = Point::new(cell.label.x0 + 1.0, cell.label.y1 - grid.baseline * 1000.0 * k);
            let map = |p: Point| at(Point::new(origin.x + p.x * k, origin.y - p.y * k));
            let mut last = Point::ZERO;
            for el in outline.elements() {
                match *el {
                    PathEl::MoveTo(p) => {
                        let q = map(p);
                        c.move_to(q.0, q.1);
                        last = p;
                    }
                    PathEl::LineTo(p) => {
                        let q = map(p);
                        c.line_to(q.0, q.1);
                        last = p;
                    }
                    PathEl::QuadTo(q, p) => {
                        // As a cubic: PDF has no quadratic segments.
                        let (c1, c2) = (last + (q - last) * (2.0 / 3.0), p + (q - p) * (2.0 / 3.0));
                        let (c1, c2, e) = (map(c1), map(c2), map(p));
                        c.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
                        last = p;
                    }
                    PathEl::CurveTo(c1, c2, p) => {
                        let (c1, c2, e) = (map(c1), map(c2), map(p));
                        c.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
                        last = p;
                    }
                    PathEl::ClosePath => {
                        c.close_path();
                    }
                }
            }
            c.fill_nonzero();
        }
    }
    // The code.
    let (r, g, b) = rgb(CODE);
    c.set_fill_rgb(r, g, b);
    for (sq, on) in grid.code_squares().into_iter().zip(code_bits(id, page)) {
        if on {
            let (x0, y0) = at(Point::new(sq.x0, sq.y1));
            c.rect(x0, y0, sq.width() as f32, sq.height() as f32);
            // Each square a shape of its own, so that a reader sees them apart.
            c.fill_nonzero();
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a4_holds_eight_by_nine() {
        let g = Grid::default();
        assert_eq!((g.columns(), g.rows()), (8, 9));
        assert_eq!(g.pages(72), 1);
        assert_eq!(g.pages(73), 2);
        let c = g.cell(73);
        assert_eq!((c.page, c.index), (1, 73));
        assert_eq!(c.square.width(), 54.0);
        // Within the page, clear of the code.
        let last = g.cell(71);
        assert!(last.square.y1 <= g.height - g.margin + 1e-9, "{last:?}");
    }

    #[test]
    fn codes_read_back() {
        let g = Grid::default();
        for (id, page) in [(0, 0), (1, 0), (MAX_ID, MAX_PAGE), (1234, 56)] {
            let marks: Vec<Rect> = g.code_squares().into_iter().zip(code_bits(id, page)).filter(|(_, on)| *on).map(|(r, _)| r).collect();
            assert_eq!(read_code(&g, (g.width, g.height), &marks), Some(Found { id, page }));
            // The same page scaled up.
            let scaled: Vec<Rect> = marks.iter().map(|r| Rect::new(r.x0 * 2.0, r.y0 * 2.0, r.x1 * 2.0, r.y1 * 2.0)).collect();
            assert_eq!(read_code(&g, (g.width * 2.0, g.height * 2.0), &scaled), Some(Found { id, page }));
        }
        assert_eq!(read_code(&g, (g.width, g.height), &[]), None);
    }

    #[test]
    fn strokes_go_to_the_cell_they_lie_in() {
        let g = Grid::default();
        let c = g.cell(10);
        assert_eq!(g.cell_at(0, 20, c.square.center()).map(|c| c.index), Some(10));
        // A little over the edge still counts; the label strip does not.
        assert_eq!(g.cell_at(0, 20, Point::new(c.square.x1 + 2.0, c.square.center().y)).map(|c| c.index), Some(10));
        assert_eq!(g.cell_at(0, 20, c.label.center()), None);
        // Not on another page, nor past the characters.
        assert_eq!(g.cell_at(1, 20, c.square.center()), None);
        assert_eq!(g.cell_at(0, 10, c.square.center()), None);
    }
}
