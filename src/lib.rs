//! The glue between the crates, shared by the viewer, the tools in `examples/` and the pipeline
//! test in `tests/`: PDF strokes (scriptoria-ink) → skeletons (scriptoria-skeleton) → outlines
//! (scriptoria-outline) → font glyphs (scriptoria-font, see [`font`]).

// GPUI comes through gpui-kit (with its platform layer and GPUI Component); under this name the
// `gpui::` paths read as usual.
extern crate gpui_kit as gpui;

pub mod app;
pub mod assign;
pub mod font;

use std::sync::atomic::{AtomicUsize, Ordering};

use scriptoria_ink::kurbo::{BezPath, Cap, Join, StrokeOpts};
use scriptoria_ink::{Stroke, StrokeShape};
use scriptoria_skeleton::Skeleton;

/// A stroke's skeleton: filled outlines are chains of capsules, lines are their own centerline.
pub fn skeleton(stroke: &Stroke) -> Skeleton {
    match stroke.shape {
        StrokeShape::Outline { .. } => scriptoria_skeleton::recover_outline(&stroke.path),
        StrokeShape::Line { width } => scriptoria_skeleton::recover_line(&stroke.path, width / 2.0),
    }
}

/// The area a stroke covers: the outline itself, or a line stroked with round caps and joins
/// (as Goodnotes draws it: `1 J 1 j`).
pub fn covered_area(stroke: &Stroke) -> BezPath {
    match stroke.shape {
        StrokeShape::Outline { .. } => stroke.path.clone(),
        StrokeShape::Line { width } => {
            let style = scriptoria_ink::kurbo::Stroke::new(width).with_caps(Cap::Round).with_join(Join::Round);
            scriptoria_ink::kurbo::stroke(stroke.path.iter(), &style, &StrokeOpts::default(), 1e-4)
        }
    }
}

/// `items.iter().map(f).collect()` on all cores, results in order. Workers take the next item from
/// a shared counter, so a few expensive strokes do not hold up a whole chunk.
pub fn par_map<T: Sync, U: Send>(items: &[T], f: impl Fn(&T) -> U + Sync) -> Vec<U> {
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(items.len()).max(1);
    let mut out: Vec<Option<U>> = (0..items.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break done };
                        done.push((i, f(item)));
                    }
                })
            })
            .collect();
        for worker in workers {
            for (i, u) in worker.join().expect("worker panicked") {
                out[i] = Some(u);
            }
        }
    });
    out.into_iter().map(|u| u.expect("every item mapped")).collect()
}


#[cfg(test)]
mod tests {
    use super::par_map;

    #[test]
    fn par_map_keeps_order() {
        let items: Vec<u64> = (0..1000).collect();
        // Uneven work per item, so workers finish out of order.
        let got = par_map(&items, |&i| {
            std::hint::black_box((0..(i % 7) * 1000).sum::<u64>());
            i
        });
        assert_eq!(got, items);
        assert!(par_map(&[] as &[u64], |&i| i).is_empty());
    }
}
