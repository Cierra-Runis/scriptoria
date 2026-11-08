//! Glyph outlines as the font draws them, for the views that show many glyphs at once (the grid,
//! the sample text): each glyph's strokes planned on background threads, when a view first asks
//! for the glyph, and its outline at a weight made from the plans and kept until the glyph or the
//! font's weights change.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Entity, Subscription, Task};
use scriptoria_ink::StrokeShape;
use scriptoria_ink::kurbo::BezPath;
use scriptoria_outline::{Plan, PlanParams, WeightParams};
use scriptoria_project::Glyph;

use crate::app::document::Document;
use crate::app::mesh::Mesh;
use crate::font::Weights;

/// How closely outlines follow the strokes (pt), as in the font.
const PRECISION: f64 = 0.004;
/// Tessellation tolerance (font units): small views.
const TOLERANCE: f32 = 1.0;
/// Glyphs planned in one go on the background threads.
const BATCH: usize = 64;

/// What a glyph's plans are made from: the document, the glyph's revision, the weights and the
/// pen.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Key {
    document: u64,
    revision: u64,
    weights: Weights,
    pen: f64,
}

struct Entry {
    key: Key,
    plans: Arc<Vec<Plan>>,
    /// The last outline made, at a radius scale.
    mesh: Option<(f64, Option<Rc<Mesh>>)>,
}

pub struct Outlines {
    document: Entity<Document>,
    entries: HashMap<usize, Entry>,
    /// Glyphs asked for and not planned yet, in the order asked.
    queue: Vec<usize>,
    queued: HashSet<usize>,
    planning: Option<Task<()>>,
    _subscription: Subscription,
}

impl Outlines {
    pub fn new(document: Entity<Document>, cx: &mut Context<Self>) -> Self {
        // Every change of the document may make outlines out of date (they say so by their key).
        let subscription = cx.observe(&document, |_, _, cx| cx.notify());
        Outlines { document, entries: HashMap::new(), queue: Vec::new(), queued: HashSet::new(), planning: None, _subscription: subscription }
    }

    fn key(&self, glyph: usize, cx: &Context<Self>) -> Key {
        let doc = self.document.read(cx);
        Key { document: doc.id(), revision: doc.revision(glyph), weights: Weights::of(&doc.project.font), pen: doc.project.font.pen_scale() }
    }

    /// Whether every glyph asked for is planned.
    pub fn idle(&self) -> bool {
        self.planning.is_none() && self.queue.is_empty()
    }

    /// A glyph's outline at a weight (`wght`), in font units (y up); none while it is being
    /// planned (it is planned then) or if it has no strokes.
    pub fn mesh(&mut self, glyph: usize, wght: f64, cx: &mut Context<Self>) -> Option<Rc<Mesh>> {
        let key = self.key(glyph, cx);
        let current = self.entries.get(&glyph).is_some_and(|e| e.key == key);
        if !current {
            if self.queued.insert(glyph) {
                self.queue.push(glyph);
            }
            self.plan(cx);
            return None;
        }
        let entry = self.entries.get_mut(&glyph)?;
        let scale = key.weights.scale(wght);
        if entry.mesh.as_ref().is_none_or(|(s, _)| (s - scale).abs() > 1e-9) {
            let w = WeightParams::scaled(scale);
            let mut path = BezPath::new();
            for plan in entry.plans.iter() {
                path.extend(scriptoria_outline::expand(plan, &w).elements().iter().copied());
            }
            let mesh = (!path.elements().is_empty())
                .then(|| Mesh::from_path_with_tolerance(&path, StrokeShape::Outline { non_zero: true }, true, TOLERANCE))
                .flatten()
                .map(Rc::new);
            entry.mesh = Some((scale, mesh));
        }
        entry.mesh.as_ref().and_then(|(_, m)| m.clone())
    }

    /// Plans the next glyphs waiting, on the background threads.
    fn plan(&mut self, cx: &mut Context<Self>) {
        if self.planning.is_some() || self.queue.is_empty() {
            return;
        }
        let batch: Vec<usize> = self.queue.drain(..self.queue.len().min(BATCH)).collect();
        let doc = self.document.read(cx);
        let jobs: Vec<(usize, Key, Glyph)> =
            batch.iter().filter_map(|&g| Some((g, self.key(g, cx), doc.project.glyphs.get(g)?.clone()))).collect();
        let background = cx.background_executor().clone();
        self.planning = Some(cx.spawn(async move |this, cx| {
            let done = background
                .spawn(async move {
                    crate::par_map(&jobs, |(g, key, glyph)| {
                        let (thinnest, _, boldest) = key.weights.scales();
                        let params = PlanParams { precision: PRECISION * key.pen, weights: (thinnest, boldest) };
                        let plans: Vec<Plan> = glyph
                            .strokes
                            .iter()
                            .filter(|s| !s.skeleton.is_empty())
                            .map(|s| scriptoria_outline::plan(&crate::assign::placed_skeleton(s), &params))
                            .collect();
                        (*g, *key, Arc::new(plans))
                    })
                })
                .await;
            this.update(cx, |this, cx| {
                this.planning = None;
                for (g, key, plans) in done {
                    this.queued.remove(&g);
                    this.entries.insert(g, Entry { key, plans, mesh: None });
                }
                this.plan(cx);
                cx.notify();
            })
            .ok();
        }));
    }
}
