//! Per-stroke metrics of the whole pipeline on `test/test.pdf`, as CSV on stdout, for comparing two
//! versions of the code stroke by stroke:
//!
//! - `iou`, `hausdorff_r`, `edge_ratio`: regenerated outline at ×1 against the original stroke;
//! - `union_iou`: the skeleton's own shape (its swept region) against the original, which tells
//!   skeleton errors from outline-construction errors;
//! - `iou_w`, `hausdorff_w_r`, `edge_w`: the outline at the weights below against the exact region
//!   its skeleton sweeps at that weight;
//! - `points`: on- and off-curve points of the outline at ×1.
//!
//! `cargo run --release --example metrics -- [page...] > metrics.csv` (all pages by default)

mod common;

use scriptoria_ink::kurbo::{BezPath, PathEl, Shape};
use scriptoria_outline::{PlanParams, WeightParams, expand, plan};
use scriptoria_testkit::metrics::{compare, compare_at};

const WEIGHTS: [f64; 2] = [0.3, 3.0];

fn points(path: &BezPath) -> usize {
    path.elements()
        .iter()
        .map(|e| match e {
            PathEl::MoveTo(_) | PathEl::LineTo(_) => 1,
            PathEl::QuadTo(..) => 2,
            PathEl::CurveTo(..) => 3,
            PathEl::ClosePath => 0,
        })
        .sum()
}

fn main() {
    let doc = common::test_pdf();
    let pages: Vec<usize> = match std::env::args().skip(1).map(|a| a.parse().expect("page index")).collect::<Vec<_>>() {
        v if v.is_empty() => (0..doc.page_count()).collect(),
        v => v,
    };
    let weight_columns: String = WEIGHTS.iter().map(|w| format!(",iou_{w},hausdorff_{w}_r,edge_{w}")).collect();
    println!("page,stroke,subpaths,circles,rmax,iou,hausdorff_r,edge_ratio,union_iou{weight_columns},points");
    for page_index in pages {
        let page = doc.page(page_index).unwrap();
        let rows = scriptoria::par_map(&page.strokes, |stroke| {
            let sk = scriptoria::skeleton(stroke);
            let plan = plan(&sk, &PlanParams::default());
            let rmax = sk.max_radius().max(1e-9);
            let original = scriptoria::covered_area(stroke);
            let regen = expand(&plan, &WeightParams::scaled(1.0));
            let c = compare(&original, &regen);
            let data = common::skeleton_data(&sk);
            let union = compare(&original, &data.swept(1.0));
            let mut row = format!(
                "{page_index},{},{},{},{rmax:.4},{:.4},{:.3},{:.3},{:.4}",
                stroke.order,
                stroke.subpaths,
                sk.circle_count(),
                c.iou,
                c.hausdorff / rmax,
                c.edge_ratio,
                union.iou
            );
            for w in WEIGHTS {
                let out = expand(&plan, &WeightParams::scaled(w));
                let reference = data.swept(w);
                let extent = reference.bounding_box().union(out.bounding_box());
                let step = (rmax * w / 20.0).max(extent.width().max(extent.height()) / 3000.0);
                let cw = compare_at(&reference, &out, step);
                row += &format!(",{:.4},{:.3},{:.3}", cw.iou, cw.hausdorff / (rmax * w), cw.edge_ratio);
            }
            row + &format!(",{}", points(&regen))
        });
        for row in rows {
            println!("{row}");
        }
    }
}
