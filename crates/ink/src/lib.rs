//! Extracts handwritten strokes from PDF pages (vector ink exported by Goodnotes and similar apps).
//!
//! Goodnotes does not use `/Ink` annotations; every stroke is drawn straight into the page's
//! content stream:
//! - ordinary strokes: one `f` fill made of many consecutive small outlines (subpaths), which is
//!   how the varying width (pen pressure) is drawn;
//! - very short or thin strokes: one `S` stroke, i.e. the centerline itself.
//!
//! The content stream is painted in order and Goodnotes writes its strokes in their internal
//! order, so **drawing order ≈ writing order**.

use hayro_interpret::font::GlyphRun;
use hayro_interpret::kurbo::{BezPath, ParamCurveNearest, PathEl, Point, Rect, Shape};
use hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, FillRule, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, Paint, SoftMask, TransformExt, interpret_page,
};
use hayro_syntax::Pdf;

/// Re-exported so downstream crates use the very same geometry types.
pub use hayro_interpret::kurbo;

/// How a stroke is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrokeShape {
    /// A filled outline (an ordinary stroke with varying width).
    Outline { non_zero: bool },
    /// A stroked centerline; `width` is in page points.
    Line { width: f64 },
}

/// One handwritten stroke. Coordinates are page points with the origin at the top-left, y down.
#[derive(Clone, Debug)]
pub struct Stroke {
    /// Writing order, from 0.
    pub order: usize,
    pub path: BezPath,
    pub shape: StrokeShape,
    pub rgba: [u8; 4],
    pub bbox: Rect,
    /// Pen-down point.
    pub start: Point,
    /// Pen-up point (best effort).
    pub end: Point,
    /// Number of subpaths.
    pub subpaths: usize,
}

/// The strokes of one page.
#[derive(Clone, Debug)]
pub struct InkPage {
    pub width: f64,
    pub height: f64,
    /// Paper color (a fill covering the page), if any.
    pub paper: Option<[u8; 4]>,
    pub strokes: Vec<Stroke>,
}

pub struct InkDocument {
    pdf: Pdf,
}

impl InkDocument {
    pub fn open(data: Vec<u8>) -> anyhow::Result<Self> {
        let pdf = Pdf::new(data).map_err(|e| anyhow::anyhow!("cannot parse PDF: {e:?}"))?;
        Ok(Self { pdf })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Extracts the strokes of page `index` (from 0).
    pub fn page(&self, index: usize) -> Option<InkPage> {
        let page = self.pdf.pages().get(index)?;
        let (w, h) = page.render_dimensions();
        let (w, h) = (w as f64, h as f64);
        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            page.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, w, h),
            &cache,
            self.pdf.xref(),
            InterpreterSettings::default(),
        );
        let mut collector = Collector { page_area: w * h, paper: None, strokes: Vec::new() };
        interpret_page(page, &mut ctx, &mut collector);
        Some(InkPage { width: w, height: h, paper: collector.paper, strokes: collector.strokes })
    }
}

impl InkPage {
    /// The stroke nearest to page point `p`, at most `tolerance` away, among the first `visible`
    /// strokes. Where strokes overlap, the later (upper) one wins.
    pub fn hit_test(&self, p: Point, tolerance: f64, visible: usize) -> Option<usize> {
        let tol_sq = tolerance * tolerance;
        let mut best: Option<(usize, f64)> = None;
        for stroke in self.strokes[..visible.min(self.strokes.len())].iter().rev() {
            if !stroke.bbox.inflate(tolerance, tolerance).contains(p) {
                continue;
            }
            let d = if matches!(stroke.shape, StrokeShape::Outline { .. }) && stroke.path.contains(p) {
                0.0
            } else {
                stroke.path.segments().map(|seg| seg.nearest(p, 1e-3).distance_sq).fold(f64::INFINITY, f64::min)
            };
            if d <= tol_sq && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((stroke.order, d));
                if d == 0.0 {
                    break;
                }
            }
        }
        best.map(|(i, _)| i)
    }
}

struct Collector {
    page_area: f64,
    paper: Option<[u8; 4]>,
    strokes: Vec<Stroke>,
}

impl Collector {
    fn push(&mut self, path: BezPath, shape: StrokeShape, rgba: [u8; 4]) {
        let bbox = path.bounding_box();
        // A first fill covering the page is the paper, not a stroke.
        if self.strokes.is_empty()
            && self.paper.is_none()
            && matches!(shape, StrokeShape::Outline { .. })
            && bbox.area() >= self.page_area * 0.9
        {
            self.paper = Some(rgba);
            return;
        }
        let (start, end, subpaths) = endpoints(&path, shape);
        self.strokes.push(Stroke { order: self.strokes.len(), path, shape, rgba, bbox, start, end, subpaths });
    }
}

/// Pen-down and pen-up points.
///
/// - Lines: the path's first and last points.
/// - Outlines: Goodnotes splits a stroke into consecutive small outlines, each starting along one
///   side in the writing direction. So the first subpath's start is the pen-down point, and the
///   end of the last subpath's first segment is the pen-up point.
fn endpoints(path: &BezPath, shape: StrokeShape) -> (Point, Point, usize) {
    let mut start = None;
    let mut last_end = Point::ORIGIN;
    let mut sub_first_seg_end: Option<Point> = None;
    let mut awaiting_first_seg = false;
    let mut subpaths = 0;
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                start.get_or_insert(p);
                subpaths += 1;
                awaiting_first_seg = true;
                last_end = p;
            }
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => {
                if awaiting_first_seg {
                    sub_first_seg_end = Some(p);
                    awaiting_first_seg = false;
                }
                last_end = p;
            }
            PathEl::ClosePath => {}
        }
    }
    let start = start.unwrap_or(Point::ORIGIN);
    let end = match shape {
        StrokeShape::Line { .. } => last_end,
        StrokeShape::Outline { .. } => sub_first_seg_end.unwrap_or(last_end),
    };
    (start, end, subpaths)
}

impl<'a> Device<'a> for Collector {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) {
        let Paint::Color(color) = &props.paint else { return };
        let rgba = color.to_rgba().to_rgba8();
        let mut p = path.clone();
        p.apply_affine(props.transform);
        match mode {
            DrawMode::Fill(rule) | DrawMode::FillAndStroke(rule, _) => {
                let non_zero = matches!(rule, FillRule::NonZero);
                self.push(p, StrokeShape::Outline { non_zero }, rgba);
            }
            DrawMode::Stroke(s) => {
                // The line width is in user space: scale it by the transform to page points.
                let scale = props.transform.determinant().abs().sqrt();
                let width = s.line_width as f64 * scale;
                self.push(p, StrokeShape::Line { width }, rgba);
            }
            // Invisible paths (`n`, text render mode 3) are not strokes.
            DrawMode::Invisible => {}
        }
    }
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph_run(&mut self, _: &GlyphRun<'_, 'a>, _: DrawProps<'a>, _: &DrawMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_writer::{Content, Pdf as PdfWriter, Rect as PdfRect, Ref};

    /// A 100 × 50 pt page: paper fill, one filled stroke (two subpaths), one stroked line.
    fn synthetic_pdf() -> Vec<u8> {
        let mut c = Content::new();
        c.set_fill_rgb(0.97, 0.96, 0.91);
        c.rect(0.0, 0.0, 100.0, 50.0);
        c.fill_nonzero();
        // A fill drawn in two pieces, moving right (PDF y up: y = 40 is near the top).
        c.set_fill_rgb(0.1, 0.1, 0.1);
        c.move_to(10.0, 41.0);
        c.line_to(20.0, 41.0);
        c.line_to(20.0, 39.0);
        c.line_to(10.0, 39.0);
        c.close_path();
        c.move_to(20.0, 41.0);
        c.line_to(30.0, 41.0);
        c.line_to(30.0, 39.0);
        c.line_to(20.0, 39.0);
        c.close_path();
        c.fill_nonzero();
        c.set_stroke_rgb(0.2, 0.2, 0.2);
        c.set_line_width(0.5);
        c.move_to(50.0, 10.0);
        c.line_to(60.0, 20.0);
        c.stroke();

        let (catalog, tree, page, contents) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
        let mut pdf = PdfWriter::new();
        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree).kids([page]).count(1);
        pdf.page(page).parent(tree).media_box(PdfRect::new(0.0, 0.0, 100.0, 50.0)).contents(contents).resources();
        let content = c.finish();
        pdf.stream(contents, &content);
        pdf.finish()
    }

    fn page() -> InkPage {
        InkDocument::open(synthetic_pdf()).unwrap().page(0).unwrap()
    }

    #[test]
    fn extracts_paper_and_strokes_in_order() {
        let page = page();
        assert_eq!((page.width, page.height), (100.0, 50.0));
        assert!(page.paper.is_some(), "the full-page fill is the paper");
        assert_eq!(page.strokes.len(), 2);
        assert!(page.strokes.iter().enumerate().all(|(i, s)| s.order == i));
        assert!(matches!(page.strokes[0].shape, StrokeShape::Outline { non_zero: true }));
        assert_eq!(page.strokes[0].subpaths, 2);
        let StrokeShape::Line { width } = page.strokes[1].shape else { panic!("expected a line") };
        assert!((width - 0.5).abs() < 1e-6);
    }

    #[test]
    fn flips_to_y_down_and_finds_pen_down_and_up() {
        let page = page();
        let fill = &page.strokes[0];
        // PDF y = 41 near the top of a 50 pt page → y = 9 with y down.
        assert!((fill.start - Point::new(10.0, 9.0)).hypot() < 1e-6, "{:?}", fill.start);
        // End of the last subpath's first segment.
        assert!((fill.end - Point::new(30.0, 9.0)).hypot() < 1e-6, "{:?}", fill.end);
        let line = &page.strokes[1];
        assert!((line.start - Point::new(50.0, 40.0)).hypot() < 1e-6);
        assert!((line.end - Point::new(60.0, 30.0)).hypot() < 1e-6);
    }

    #[test]
    fn hit_test_prefers_the_nearest_visible_stroke() {
        let page = page();
        assert_eq!(page.hit_test(Point::new(15.0, 10.0), 0.5, 2), Some(0)); // inside the fill
        assert_eq!(page.hit_test(Point::new(55.0, 35.2), 0.5, 2), Some(1)); // next to the line
        assert_eq!(page.hit_test(Point::new(55.0, 35.2), 0.5, 1), None); // line not visible yet
        assert_eq!(page.hit_test(Point::new(90.0, 45.0), 0.5, 2), None);
    }
}
