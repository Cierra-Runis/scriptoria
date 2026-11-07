//! Draws a region of a page of `test/test.pdf` with every stroke labelled by its index, and lists
//! simple features per stroke, to choose test strokes from.
//!
//! `cargo run --release --example survey -- <page> [x0 y0 x1 y1]` (page points, y down) → report
//! `survey-p<page>`.

mod common;

use scriptoria_ink::kurbo::{PathEl, Rect};
use scriptoria_ink::StrokeShape;
use scriptoria_testkit::report::{Panel, Report, Svg, color};

fn main() {
    let args: Vec<f64> = std::env::args().skip(1).map(|a| a.parse().expect("number")).collect();
    let page_index = args.first().copied().unwrap_or(0.0) as usize;
    let doc = common::test_pdf();
    let page = doc.page(page_index).unwrap();
    let region = match args.get(1..5) {
        Some(r) => Rect::new(r[0], r[1], r[2], r[3]),
        None => Rect::new(0.0, 0.0, page.width, page.height),
    };
    let strokes: Vec<_> = page.strokes.iter().filter(|s| region.contains(s.bbox.center())).collect();

    let mut report = Report::new(&format!("survey-p{page_index}"), &format!("survey · page {page_index} · {region:?}"));
    let mut svg = Svg::new(region, 1400.0);
    // A grid every point (labels every point too), to read off rectangles.
    for x in (region.x0.ceil() as i64)..=(region.x1.floor() as i64) {
        let x = x as f64;
        svg.polyline(&[(x, region.y0).into(), (x, region.y1).into()], "#93c5fd", 0.5);
        svg.text((x, region.y0 + 0.02 * region.height()).into(), &format!("{x}"), 10.0, "#2563eb");
    }
    for y in (region.y0.ceil() as i64)..=(region.y1.floor() as i64) {
        let y = y as f64;
        svg.polyline(&[(region.x0, y).into(), (region.x1, y).into()], "#93c5fd", 0.5);
        svg.text((region.x0, y).into(), &format!("{y}"), 10.0, "#2563eb");
    }
    for s in &strokes {
        svg.path(&scriptoria::covered_area(s), color::ORIGINAL, "none", 0.0);
    }
    for s in &strokes {
        svg.text(s.bbox.center(), &s.order.to_string(), 11.0, "#b91c1c");
    }
    report.section("Strokes by index", "label at each stroke's center").panels.push(Panel { caption: format!("{} strokes", strokes.len()), svg: svg.render(), flagged: false });

    report.summary("index · kind · subpaths · zero-length markers per subpath (max) · size (pt) · largest radius (pt)");
    for s in &strokes {
        let (kind, markers) = match s.shape {
            StrokeShape::Outline { .. } => ("outline", max_markers(&s.path)),
            StrokeShape::Line { .. } => ("line", 0),
        };
        let sk = scriptoria::skeleton(s);
        report.summary(format!(
            "#{:<5} {kind:<7} {:>4} {markers:>2}  {:.2}×{:.2}  r {:.3}",
            s.order,
            s.subpaths,
            s.bbox.width(),
            s.bbox.height(),
            sk.max_radius()
        ));
    }
    println!("{}", report.write().display());
}

/// The largest number of zero-length line segments (Goodnotes' side markers) in one subpath.
fn max_markers(path: &scriptoria_ink::kurbo::BezPath) -> usize {
    let mut best = 0;
    let mut count = 0;
    let mut start = None;
    let mut last = None;
    for el in path.elements() {
        if let PathEl::MoveTo(p) = el {
            best = best.max(count);
            count = 0;
            start = Some(*p);
            last = Some(*p);
            continue;
        }
        if let (Some(a), PathEl::LineTo(b)) = (last, el) {
            if (*b - a).hypot() < 1e-3 {
                count += 1;
            }
        }
        last = match el {
            PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => Some(*p),
            PathEl::ClosePath => start,
            PathEl::MoveTo(_) => last,
        };
    }
    best.max(count)
}
