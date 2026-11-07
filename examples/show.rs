//! Draws strokes of `test/test.pdf` as SVG: reference (gray), regenerated outline (red) with its
//! on-curve points, centerline and circles (blue). At weight ×1 the reference is the original
//! stroke; at other weights it is the region the skeleton sweeps at that weight.
//!
//! `cargo run --release --example show -- <page> <out dir> [w=<weight>] <stroke>...`

mod common;

use scriptoria_ink::kurbo::{PathEl, Rect, Shape};
use scriptoria_outline::{PlanParams, WeightParams, expand, plan};
use scriptoria_testkit::report::{Svg, color};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc = common::test_pdf();
    let page = doc.page(args[1].parse().expect("page index")).unwrap();
    let out = std::path::Path::new(&args[2]);
    std::fs::create_dir_all(out).unwrap();
    let mut weight = 1.0;
    for arg in &args[3..] {
        if let Some(w) = arg.strip_prefix("w=") {
            weight = w.parse().unwrap();
            continue;
        }
        let i: usize = arg.parse().unwrap();
        let stroke = &page.strokes[i];
        let sk = scriptoria::skeleton(stroke);
        let regen = expand(&plan(&sk, &PlanParams::default()), &WeightParams::scaled(weight));
        let reference = if weight == 1.0 {
            scriptoria::covered_area(stroke)
        } else {
            common::skeleton_data(&sk).swept(weight)
        };
        let pad = sk.max_radius() * weight.max(1.0) + 0.1;
        let view = stroke.bbox.union(regen.bounding_box()).inflate(pad, pad);
        let side = view.width().max(view.height());
        let mut svg = Svg::new(Rect::from_center_size(view.center(), (side, side)), 800.0);
        svg.path(&reference, color::ORIGINAL, "none", 0.0);
        svg.path(&regen, color::REGENERATED_FILL, color::REGENERATED, 1.0);
        for piece in &sk.pieces {
            svg.polyline(&piece.densify(16).iter().map(|s| s.p).collect::<Vec<_>>(), color::SPINE, 1.0);
        }
        for s in sk.samples() {
            svg.circle(s.p, s.r * weight, color::SPINE, 0.6);
        }
        for el in regen.elements() {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::CurveTo(_, _, p) = el {
                svg.dot(*p, 1.5, color::REGENERATED);
            }
        }
        svg.dot(stroke.start, 4.0, color::START);
        let name = if weight == 1.0 { format!("{i}.svg") } else { format!("{i}-w{weight}.svg") };
        std::fs::write(out.join(name), svg.render()).unwrap();
    }
}
