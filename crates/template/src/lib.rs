//! Practice sheets: pages of cells to write characters in, one character to a cell, opened in
//! Goodnotes (or printed) as a notebook; and, on the pages written, finding the sheet again.
//!
//! A cell is a writing square (the em, descender to ascender) under a strip showing the character
//! to write. In the square, the reference font's character is drawn faintly ([`MODEL_ALPHA`]) to
//! write over, so that each character comes out as large as the reference's and where it is; and
//! the reference font's design lines ([`Lines`]): for CJK characters the ideographic face and the
//! middle lines (田), for others the baseline, ascender, cap height, x-height and descender. Each
//! page carries a code at its foot, a row of small squares, saying which sheet and which page it
//! is, so that the strokes written on it can be put into their characters' glyphs by the cells
//! they lie in.
//!
//! A sheet is sized by its squares, made as large as the handwriting is written (the em in page
//! points), everything else in proportion; a page holds many squares across, each a small part of
//! it as handwriting is of a notebook's page, and is shaped as A4 ([`Grid::portrait`]).
//!
//! Pages are vector drawings in colors of their own ([`is_sheet_color`]), which nothing written on
//! them is expected to use: a reader tells the sheet's drawing from the handwriting by its color.
//! Page coordinates here are as in the ink crate: points, the origin at the top left, y down.

use kurbo::{BezPath, PathEl, Point, Rect};
pub use pdf_writer;
use pdf_writer::{Content, Name, Pdf, Rect as PdfRect, Ref};
use serde::{Deserialize, Serialize};

/// The colors of the sheet's drawing (RGB): the squares, their guides, the characters to write
/// (above the squares, and faintly in them), the code.
pub const FRAME: [u8; 3] = [150, 184, 226];
pub const GUIDE: [u8; 3] = [198, 216, 240];
pub const LABEL: [u8; 3] = [140, 146, 158];
pub const MODEL: [u8; 3] = [52, 60, 84];
pub const CODE: [u8; 3] = [38, 58, 104];

/// How opaque the characters in the squares are.
pub const MODEL_ALPHA: f32 = 0.1;

/// The characters in the squares as they look on white paper: what an app that flattens the
/// page's transparency would write instead of [`MODEL`].
fn model_on_paper() -> [f64; 3] {
    MODEL.map(|c| 255.0 - (255.0 - c as f64) * MODEL_ALPHA as f64)
}

/// Whether something drawn in this color is the sheet's own.
pub fn is_sheet_color(rgb: [u8; 3]) -> bool {
    let flat = model_on_paper();
    [FRAME, GUIDE, LABEL, MODEL, CODE].contains(&rgb) || rgb.iter().zip(flat).all(|(&c, f)| (c as f64 - f).abs() <= 2.0)
}

/// The code at a page's foot: a start square, the sheet's id (12 bits), the page (10 bits), an
/// end square.
const ID_BITS: usize = 12;
const PAGE_BITS: usize = 10;
const CODE_SQUARES: usize = 1 + ID_BITS + PAGE_BITS + 1;

/// The largest sheet id and page a code can hold.
pub const MAX_ID: u32 = (1 << ID_BITS) - 1;
pub const MAX_PAGE: usize = (1 << PAGE_BITS) - 1;

/// The parts of a sheet as shares of a square's side: the strip above it, the space between
/// cells, the space around them (the code sits in the bottom one).
const LABEL_SHARE: f64 = 0.3;
const GAP_SHARE: f64 = 0.15;
const MARGIN_SHARE: f64 = 0.6;

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
    /// Squares of 54 pt, eight by nine to a page.
    fn default() -> Self {
        Grid::new(54.0, 8, 9, 0.12)
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
    /// Pages `columns` squares of side `cell` wide (the rest in proportion), with as many rows as
    /// fit in the shape of an ISO page (A4) standing up.
    pub fn portrait(cell: f64, columns: usize, baseline: f64) -> Grid {
        let width = Grid::new(cell, columns, 1, baseline).width;
        let (margin, row) = (MARGIN_SHARE * cell, (LABEL_SHARE + 1.0 + GAP_SHARE) * cell);
        let rows = ((width * std::f64::consts::SQRT_2 - 2.0 * margin + GAP_SHARE * cell) / row).floor() as usize;
        Grid::new(cell, columns, rows.max(1), baseline)
    }

    /// Pages of `columns` × `rows` squares of side `cell`, the rest in proportion (a page is made
    /// wide enough for its code, which may give it more columns).
    pub fn new(cell: f64, columns: usize, rows: usize, baseline: f64) -> Grid {
        let (label, gap, margin) = (LABEL_SHARE * cell, GAP_SHARE * cell, MARGIN_SHARE * cell);
        let mut grid = Grid { width: 0.0, height: 0.0, margin, cell, label, gap, baseline };
        let code = CODE_SQUARES as f64 * grid.code_step();
        grid.width = (2.0 * margin + columns.max(1) as f64 * (cell + gap) - gap).max(2.0 * margin + code);
        grid.height = 2.0 * margin + rows.max(1) as f64 * (label + cell + gap) - gap;
        grid
    }

    pub fn columns(&self) -> usize {
        (((self.width - 2.0 * self.margin + self.gap) / (self.cell + self.gap) + 1e-9).floor() as usize).max(1)
    }

    pub fn rows(&self) -> usize {
        let room = self.height - 2.0 * self.margin + self.gap;
        ((room / (self.label + self.cell + self.gap) + 1e-9).floor() as usize).max(1)
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

    /// The side of a code square, and the distance from one to the next: in proportion to the
    /// squares, up to 5 and 8 pt.
    fn code_size(&self) -> f64 {
        (0.2 * self.cell).min(5.0)
    }

    fn code_step(&self) -> f64 {
        (0.32 * self.cell).min(8.0)
    }

    /// The code's squares, left to right.
    fn code_squares(&self) -> Vec<Rect> {
        let (size, step) = (self.code_size(), self.code_step());
        let y = self.height - self.margin / 2.0 - size / 2.0;
        (0..CODE_SQUARES)
            .map(|i| {
                let x = self.margin + i as f64 * step;
                Rect::new(x, y, x + size, y + size)
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
    let slack = 0.3 * grid.code_step();
    let bits: Vec<bool> = grid.code_squares().iter().map(|sq| centers.iter().any(|c| sq.inflate(slack, slack).contains(*c))).collect();
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

/// A character as the reference font draws it, in units of an em of 1000: y up, the baseline at
/// 0, the em (the square) from `−baseline × 1000` up.
#[derive(Clone, Debug)]
pub struct Model {
    pub outline: BezPath,
    pub advance: f64,
}

/// The reference font's design lines, drawn in the squares (heights as [`Model`]'s y): for letters
/// and the like the ascender, cap height, x-height and descender (with the baseline); for CJK
/// characters the ideographic face, the box they keep within (its bottom and top; its sides as far
/// in from the square's).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lines {
    pub ascender: Option<f64>,
    pub cap_height: Option<f64>,
    pub x_height: Option<f64>,
    pub descender: Option<f64>,
    pub face: Option<(f64, f64)>,
}

/// The name a page's resources give the faint drawing state (see [`write_faint_state`]).
pub const FAINT: Name<'static> = Name(b"Faint");

/// Writes the graphics state the characters in the squares are drawn in: a page drawn with
/// [`Sheet::draw_page`] lists it in its resources as [`FAINT`].
pub fn write_faint_state(pdf: &mut Pdf, id: Ref) {
    pdf.ext_graphics(id).non_stroking_alpha(MODEL_ALPHA);
}

/// A sheet to write: characters in cells, page after page, the code of sheet `id` on each.
pub struct Sheet<'a> {
    pub grid: Grid,
    pub id: u32,
    pub chars: &'a [char],
    /// How the reference font draws a character; characters without one get an empty square.
    pub models: &'a dyn Fn(char) -> Option<Model>,
    pub lines: Lines,
}

impl Sheet<'_> {
    /// The sheet as a PDF.
    pub fn pdf(&self) -> Vec<u8> {
        assert!(self.id <= MAX_ID, "sheet id {} too large", self.id);
        let pages = self.grid.pages(self.chars.len());
        assert!(pages <= MAX_PAGE + 1, "too many pages");
        let mut pdf = Pdf::new();
        let (catalog, tree, faint) = (Ref::new(1), Ref::new(2), Ref::new(3));
        let page_ref = |p: usize| Ref::new(4 + 2 * p as i32);
        let content_ref = |p: usize| Ref::new(5 + 2 * p as i32);
        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids((0..pages).map(page_ref)).count(pages as i32);
        write_faint_state(&mut pdf, faint);
        for page in 0..pages {
            let mut c = Content::new();
            self.draw_page(&mut c, page);
            {
                let mut p = pdf.page(page_ref(page));
                p.parent(tree)
                    .media_box(PdfRect::new(0.0, 0.0, self.grid.width as f32, self.grid.height as f32))
                    .contents(content_ref(page));
                p.resources().ext_g_states().pair(FAINT, faint);
            }
            pdf.stream(content_ref(page), &c.finish());
        }
        pdf.finish()
    }

    /// Draws page `page` into a content stream (whose page lists [`FAINT`]): for a PDF of one's
    /// own, or to write on.
    pub fn draw_page(&self, c: &mut Content, page: usize) {
        let grid = &self.grid;
        let rgb = |[r, g, b]: [u8; 3]| (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        // Page coordinates (y down) to PDF (y up).
        let h = grid.height;
        let at = |p: Point| (p.x as f32, (h - p.y) as f32);
        let line = |c: &mut Content, a: Point, b: Point| {
            let (a, b) = (at(a), at(b));
            c.move_to(a.0, a.1);
            c.line_to(b.0, b.1);
        };
        // Lines and dashes in proportion to the squares (as on a sheet of 54 pt squares).
        let k = (grid.cell / 54.0) as f32;
        for index in page * grid.per_page()..self.chars.len().min((page + 1) * grid.per_page()) {
            let cell = grid.cell(index);
            let ch = self.chars[index];
            let sq = cell.square;
            let model = (self.models)(ch);
            // The em of 1000 (y up, baseline 0) to the square: the advance centered.
            let unit = sq.width() / 1000.0;
            let base = sq.y1 - grid.baseline * sq.height();
            let x0 = model.as_ref().map_or(sq.x0, |m| sq.center().x - m.advance / 2.0 * unit);
            let in_square = |p: Point| Point::new(x0 + p.x * unit, base - p.y * unit);
            if let Some(m) = &model {
                // The character to write over, faint.
                let (r, g, b) = rgb(MODEL);
                c.save_state();
                c.set_parameters(FAINT);
                c.set_fill_rgb(r, g, b);
                if path(c, &m.outline, |p| at(in_square(p))) {
                    c.fill_nonzero();
                }
                c.restore_state();
            }
            // The reference font's lines. CJK: the ideographic face (solid), the middle lines
            // (dashed). Others: the baseline (solid); the ascender, cap height, x-height and
            // descender (dashed; a descender may reach into the gap below the square).
            let (r, g, b) = rgb(GUIDE);
            c.set_stroke_rgb(r, g, b);
            let across = |c: &mut Content, height: f64| {
                let y = base - height * unit;
                line(c, Point::new(sq.x0, y), Point::new(sq.x1, y));
            };
            c.set_line_width(0.5 * k);
            if is_square(ch) {
                if let Some((bottom, top)) = self.lines.face {
                    // As far in from the square's sides as from its top and bottom.
                    let side = (1000.0 - (top - bottom)) / 2.0;
                    let (a, b) = (Point::new(sq.x0 + side * unit, base - top * unit), Point::new(sq.x1 - side * unit, base - bottom * unit));
                    let (x, y) = at(Point::new(a.x, b.y));
                    c.rect(x, y, (b.x - a.x) as f32, (b.y - a.y) as f32);
                    c.stroke();
                }
                c.set_line_width(0.4 * k);
                c.set_dash_pattern([2.0 * k, 2.0 * k], 0.0);
                let m = sq.center();
                line(c, Point::new(sq.x0, m.y), Point::new(sq.x1, m.y));
                line(c, Point::new(m.x, sq.y0), Point::new(m.x, sq.y1));
            } else {
                across(c, 0.0);
                c.stroke();
                c.set_line_width(0.4 * k);
                c.set_dash_pattern([2.0 * k, 2.0 * k], 0.0);
                let l = &self.lines;
                // Without the reference's x-height: about the lowercase letters'.
                let x_height = l.x_height.unwrap_or(0.55 * (1.0 - grid.baseline) * 1000.0);
                for height in [l.ascender, l.cap_height, Some(x_height), l.descender].into_iter().flatten() {
                    across(c, height);
                }
            }
            c.stroke();
            c.set_dash_pattern([], 0.0);
            // The square.
            let (r, g, b) = rgb(FRAME);
            c.set_stroke_rgb(r, g, b);
            c.set_line_width(0.75 * k);
            let (x, y) = at(Point::new(sq.x0, sq.y1));
            c.rect(x, y, sq.width() as f32, sq.height() as f32);
            c.stroke();
            // The character to write, at the strip's left.
            if let Some(m) = &model {
                let (r, g, b) = rgb(LABEL);
                c.set_fill_rgb(r, g, b);
                let s = cell.label.height() * 0.9 / 1000.0;
                let origin = Point::new(cell.label.x0 + cell.label.height() * 0.06, cell.label.y1 - grid.baseline * 1000.0 * s);
                if path(c, &m.outline, |p| at(Point::new(origin.x + p.x * s, origin.y - p.y * s))) {
                    c.fill_nonzero();
                }
            }
        }
        // The code.
        let (r, g, b) = rgb(CODE);
        c.set_fill_rgb(r, g, b);
        for (sq, on) in grid.code_squares().into_iter().zip(code_bits(self.id, page)) {
            if on {
                let (x0, y0) = at(Point::new(sq.x0, sq.y1));
                c.rect(x0, y0, sq.width() as f32, sq.height() as f32);
                // Each square a shape of its own, so that a reader sees them apart.
                c.fill_nonzero();
            }
        }
    }
}

/// Adds an outline to the content's current path through `map` (quadratic segments as cubic:
/// PDF has none); false if it is empty.
fn path(c: &mut Content, outline: &BezPath, map: impl Fn(Point) -> (f32, f32)) -> bool {
    let mut last = Point::ZERO;
    let mut any = false;
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
                any = true;
            }
            PathEl::QuadTo(q, p) => {
                let (c1, c2) = (last + (q - last) * (2.0 / 3.0), p + (q - p) * (2.0 / 3.0));
                let (c1, c2, e) = (map(c1), map(c2), map(p));
                c.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
                last = p;
                any = true;
            }
            PathEl::CurveTo(c1, c2, p) => {
                let (c1, c2, e) = (map(c1), map(c2), map(p));
                c.cubic_to(c1.0, c1.1, c2.0, c2.1, e.0, e.1);
                last = p;
                any = true;
            }
            PathEl::ClosePath => {
                c.close_path();
            }
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grid_holds_the_cells_asked_for() {
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
        assert!(g.code_squares().iter().all(|s| s.y0 > last.square.y1 && s.x1 <= g.width - g.margin + 1e-9));
        // Squares as small as handwriting, the rest in proportion.
        let small = Grid::new(4.5, 8, 10, 0.12);
        assert_eq!((small.columns(), small.rows()), (8, 10));
        assert!((small.width / g.width - 4.5 / 54.0).abs() < 0.01, "{small:?}");
        // Too narrow for the code: wider, so more columns.
        let narrow = Grid::new(10.0, 2, 3, 0.12);
        assert!(narrow.columns() > 2 && narrow.code_squares().last().unwrap().x1 <= narrow.width - narrow.margin + 1e-9);
        // Many squares to a row, on a page shaped as A4: within a row of it.
        let page = Grid::portrait(3.1, 24, 0.12);
        assert_eq!(page.columns(), 24);
        let row = page.label + page.cell + page.gap;
        assert!(page.height <= page.width * std::f64::consts::SQRT_2 + 1e-9 && page.height > page.width * std::f64::consts::SQRT_2 - row, "{page:?}");
    }

    #[test]
    fn codes_read_back() {
        for g in [Grid::default(), Grid::new(4.5, 8, 10, 0.12)] {
            for (id, page) in [(0, 0), (1, 0), (MAX_ID, MAX_PAGE), (1234, 56)] {
                let marks: Vec<Rect> = g.code_squares().into_iter().zip(code_bits(id, page)).filter(|(_, on)| *on).map(|(r, _)| r).collect();
                assert_eq!(read_code(&g, (g.width, g.height), &marks), Some(Found { id, page }));
                // The same page scaled up.
                let scaled: Vec<Rect> = marks.iter().map(|r| Rect::new(r.x0 * 2.0, r.y0 * 2.0, r.x1 * 2.0, r.y1 * 2.0)).collect();
                assert_eq!(read_code(&g, (g.width * 2.0, g.height * 2.0), &scaled), Some(Found { id, page }));
            }
            assert_eq!(read_code(&g, (g.width, g.height), &[]), None);
        }
    }

    #[test]
    fn sheets_made_before_still_read() {
        // A sheet as the first version wrote it: A4, squares of 54 pt, code squares of 5 pt.
        let old = Grid { width: 595.276, height: 841.89, margin: 36.0, cell: 54.0, label: 16.0, gap: 8.0, baseline: 0.12 };
        assert_eq!((old.columns(), old.rows()), (8, 9));
        let marks: Vec<Rect> = code_bits(7, 1)
            .into_iter()
            .enumerate()
            .filter(|(_, on)| *on)
            .map(|(i, _)| {
                let (x, y) = (36.0 + i as f64 * 8.0, 841.89 - 18.0 - 2.5);
                Rect::new(x, y, x + 5.0, y + 5.0)
            })
            .collect();
        assert_eq!(read_code(&old, (old.width, old.height), &marks), Some(Found { id: 7, page: 1 }));
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

    #[test]
    fn the_faint_model_is_the_sheets_on_paper_too() {
        assert!(is_sheet_color(MODEL));
        assert!(is_sheet_color([234, 235, 238]));
        assert!(!is_sheet_color([0, 0, 0]));
        assert!(!is_sheet_color([255, 255, 255]));
    }
}
