//! A few strokes from drawings (`tests/data/drawing.svg`): not handwriting, so only a smoke test
//! (recovers without trouble, roughly the right shape).

mod common;

use common::fit;

#[test]
fn drawing_strokes_recover() {
    for stroke in common::read("drawing.svg").iter().flat_map(|g| &g.items) {
        let f = fit(stroke);
        assert!(f.iou > 0.8, "{}: IoU {:.3}", stroke.id, f.iou);
    }
}
