//! One glyph, as in FontForge's glyph window: its strokes in the em with the metric lines
//! (ascender, baseline, descender, advance) and each stroke's place in the writing order at its
//! start, at any weight of the axis. Strokes are selected (click, box, the list beside) and moved
//! (drag, arrow keys), scaled (the selection box's corners), deleted and reordered; the advance is
//! dragged or typed. Every change is one step to undo.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::notification::Notification;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::{ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{
    App, Bounds, ClickEvent, ContentMask, Context, Entity, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent, SharedString, Subscription, Task, TextAlign, Window, canvas,
    div, point, px, quad, size,
};
use scriptoria_ink::StrokeShape;
use scriptoria_ink::kurbo::{self, Affine, BezPath, ParamCurveNearest as _, Shape as _};
use scriptoria_outline::{Plan, PlanParams, WeightParams};
use scriptoria_project::{Glyph, GlyphStroke, Outline, OutlineKind, SkeletonPiece};

use crate::app::document::{Document, DocumentEvent};
use crate::app::mesh::Mesh;
use crate::font::{AXIS, Weights};

/// Tessellation tolerance (pt, the strokes' own units): an em of 4.5 pt is some 600 px here.
const TOLERANCE: f32 = 0.001;
/// How closely regenerated outlines follow the strokes (pt, made font units by the pen scale), as
/// in the font.
const PRECISION: f64 = 0.004;
/// How near (px) the pointer must be to a stroke, a handle or the advance line to grab it.
const HIT_PX: f64 = 4.0;
const HANDLE_PX: f32 = 4.0;
/// A drag shorter than this (px) is a click.
const DRAG_THRESHOLD: f32 = 3.0;
/// Zoom range, relative to "fit to window", and zoom per scrolled pixel.
const ZOOM_RANGE: (f32, f32) = (0.2, 40.0);
const ZOOM_PER_PX: f32 = 0.004;
/// Arrow keys move the selection this far (font units); ten times as far with Shift.
const NUDGE: f64 = 10.0;
/// The color of selected strokes (as on the pages).
const SELECTED: Hsla = Hsla { h: 0.75, s: 0.8, l: 0.5, a: 1.0 };
/// The face of a full-width character (its larger side), as a fraction of the em: shown as a
/// guide, and what "full width" scales to.
const FACE: f64 = 0.9;

/// Font units (y up) → window: `origin + (x, −y) · scale`.
#[derive(Clone, Copy, PartialEq, Debug)]
struct View {
    origin: Point<Pixels>,
    scale: f32,
}

impl View {
    /// The em square, with a margin, centered in `bounds`.
    fn fit(bounds: Bounds<Pixels>, em: kurbo::Rect) -> Self {
        let content = em.inflate(em.width() * 0.15, em.height() * 0.15);
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let scale = (w / content.width() as f32).min(h / content.height() as f32).max(1e-3);
        let x = f32::from(bounds.origin.x) + (w - content.width() as f32 * scale) / 2.0 - content.x0 as f32 * scale;
        let y = f32::from(bounds.origin.y) + (h - content.height() as f32 * scale) / 2.0 + content.y1 as f32 * scale;
        View { origin: point(px(x), px(y)), scale }
    }

    fn to_window(&self, p: kurbo::Point) -> Point<Pixels> {
        point(self.origin.x + px(p.x as f32 * self.scale), self.origin.y - px(p.y as f32 * self.scale))
    }

    fn to_font(&self, p: Point<Pixels>) -> kurbo::Point {
        let d = p - self.origin;
        kurbo::Point::new((f32::from(d.x) / self.scale) as f64, (-f32::from(d.y) / self.scale) as f64)
    }

    fn affine(&self) -> Affine {
        let s = self.scale as f64;
        Affine::new([s, 0.0, 0.0, -s, f32::from(self.origin.x) as f64, f32::from(self.origin.y) as f64])
    }

    fn rect(&self, r: kurbo::Rect) -> Bounds<Pixels> {
        Bounds::from_corners(self.to_window(kurbo::Point::new(r.x0, r.y1)), self.to_window(kurbo::Point::new(r.x1, r.y0)))
    }
}

/// The linear part of a placement (its scale and flip, not where it puts the stroke).
fn linear(placement: Affine) -> [f64; 4] {
    let [a, b, c, d, _, _] = placement.as_coeffs();
    [a, b, c, d]
}

/// What a stroke looks like, made from what it is and the linear part of its placement, so that
/// it is kept while the stroke is moved.
struct Shapes {
    skeleton: Vec<SkeletonPiece>,
    outline: Outline,
    /// The outline as written (page coordinates).
    original: Option<Rc<Mesh>>,
    /// What the plan and the regenerated outline are for.
    linear: [f64; 4],
    plan: Option<Arc<Plan>>,
    /// The outline regenerated at a radius scale: (scale, outline, mesh), in font units before the
    /// placement's translation. Its strokes keep the pen's width (see `FontInfo::pen_scale`).
    regenerated: Option<(f64, BezPath, Option<Rc<Mesh>>)>,
}

impl Shapes {
    fn new(s: &GlyphStroke) -> Self {
        let original = Mesh::from_path_with_tolerance(&s.outline.path, shape(s.outline.kind), true, TOLERANCE).map(Rc::new);
        Shapes { skeleton: s.skeleton.clone(), outline: s.outline.clone(), original, linear: linear(s.placement), plan: None, regenerated: None }
    }

    /// Whether these are the shapes of a stroke (wherever it is placed).
    fn is(&self, s: &GlyphStroke) -> bool {
        self.skeleton == s.skeleton && self.outline == s.outline
    }

    /// Follows a change of the placement's scale or flip: the regenerated outline is made again.
    fn follow(&mut self, placement: Affine) {
        let l = linear(placement);
        if self.linear.iter().zip(&l).any(|(a, b)| (a - b).abs() > 1e-9) {
            self.linear = l;
            self.plan = None;
            self.regenerated = None;
        }
    }

    fn regenerated(&self, wanted: bool) -> Option<&(f64, BezPath, Option<Rc<Mesh>>)> {
        self.regenerated.as_ref().filter(|_| wanted)
    }

    /// The mesh shown and what puts it into font units.
    fn mesh(&self, placement: Affine, regenerated: bool) -> Option<(Rc<Mesh>, Affine)> {
        match self.regenerated(regenerated) {
            Some((_, _, mesh)) => Some((mesh.clone()?, Affine::translate(placement.translation()))),
            None => Some((self.original.clone()?, placement)),
        }
    }

    /// The shown ink's box (font units).
    fn bounds(&self, placement: Affine, regenerated: bool) -> kurbo::Rect {
        match self.regenerated(regenerated) {
            Some((_, path, _)) => path.bounding_box() + placement.translation(),
            None => {
                let reach = match self.outline.kind {
                    OutlineKind::Filled => 0.0,
                    OutlineKind::Line { width } => width / 2.0,
                };
                placement.transform_rect_bbox(self.outline.path.bounding_box().inflate(reach, reach))
            }
        }
    }

    /// Whether ink shown is within `tolerance` of `q` (font units).
    fn hit(&self, placement: Affine, q: kurbo::Point, tolerance: f64, regenerated: bool) -> bool {
        let (path, p, reach, tolerance) = match self.regenerated(regenerated) {
            Some((_, path, _)) => (path, q - placement.translation(), 0.0, tolerance),
            None => {
                let reach = match self.outline.kind {
                    OutlineKind::Filled => 0.0,
                    OutlineKind::Line { width } => width / 2.0,
                };
                (&self.outline.path, placement.inverse() * q, reach, tolerance / placement.determinant().abs().sqrt())
            }
        };
        let r = reach + tolerance;
        if !path.bounding_box().inflate(r, r).contains(p) {
            return false;
        }
        (reach == 0.0 && path.contains(p)) || path.segments().any(|s| s.nearest(p, 1e-9).distance_sq <= r * r)
    }
}

fn shape(kind: OutlineKind) -> StrokeShape {
    match kind {
        OutlineKind::Filled => StrokeShape::Outline { non_zero: true },
        OutlineKind::Line { width } => StrokeShape::Line { width },
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum DragKind {
    /// Moving the selection.
    Move,
    /// Scaling the selection about `anchor`, by the grabbed `corner` (font units).
    Scale { anchor: kurbo::Point, corner: kurbo::Point },
    /// Moving the advance line, from `from`.
    Advance { from: f64 },
    /// A selection box; with `add`, added to the selection.
    Select { add: bool },
    Pan,
}

#[derive(Clone, Copy)]
struct Drag {
    button: MouseButton,
    kind: DragKind,
    start: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
}

fn scale_about(center: kurbo::Point, k: f64) -> Affine {
    Affine::translate(center.to_vec2()) * Affine::scale(k) * Affine::translate(-center.to_vec2())
}

/// Font units shown without a fraction when whole.
fn units(v: f64) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{r:.0}") } else { format!("{r:.1}") }
}

pub struct GlyphView {
    focus: FocusHandle,
    document: Entity<Document>,
    glyph: usize,
    /// Per stroke of the glyph, in order.
    shapes: Vec<Shapes>,
    planning: Option<Task<()>>,
    /// The radius scales (thinnest, boldest) the plans in `shapes` serve.
    planned_for: (f64, f64),
    /// The pen scale (font units per page unit) the plans are for.
    planned_pen: f64,
    /// The weight shown (`wght`).
    weight: Entity<SliderState>,
    /// Show the strokes as written instead of regenerated from their skeletons.
    original: bool,
    selection: BTreeSet<usize>,
    hovered: Option<usize>,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<Drag>,
    advance: Entity<InputState>,
    /// Written while painting, read by the event handlers.
    fit: Rc<Cell<Option<View>>>,
    _subscriptions: Vec<Subscription>,
}

impl GlyphView {
    pub fn new(document: Entity<Document>, glyph: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weight = cx.new(|_| SliderState::new().max(AXIS.max as f32).min(AXIS.min as f32).step(1.0).default_value(AXIS.default as f32));
        let advance = cx.new(|cx| InputState::new(window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&document, window, move |this, _, event: &DocumentEvent, window, cx| match event {
                DocumentEvent::Glyph(g) if *g == this.glyph => this.sync(window, cx),
                DocumentEvent::GlyphList | DocumentEvent::Saved => this.sync(window, cx),
                DocumentEvent::Font => {
                    this.ensure_plans(cx);
                    this.regenerate(cx);
                    cx.notify();
                }
                _ => {}
            }),
            cx.subscribe(&weight, |this, _, _: &SliderEvent, cx| {
                this.regenerate(cx);
                cx.notify();
            }),
            cx.subscribe_in(&advance, window, |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let text = input.read(cx).value().to_string();
                    this.type_advance(&text, window, cx);
                }
            }),
        ];
        let mut this = GlyphView {
            focus: cx.focus_handle(),
            document,
            glyph,
            shapes: Vec::new(),
            planning: None,
            planned_for: (0.0, 0.0),
            planned_pen: 0.0,
            weight,
            original: false,
            selection: BTreeSet::new(),
            hovered: None,
            zoom: 1.0,
            pan: Point::default(),
            drag: None,
            advance,
            fit: Rc::default(),
            _subscriptions: subscriptions,
        };
        this.sync(window, cx);
        this
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The glyph shown (its index in the project).
    pub fn glyph_index(&self) -> usize {
        self.glyph
    }

    /// The selected strokes, in writing order.
    pub fn selection(&self) -> Vec<usize> {
        self.selection.iter().copied().collect()
    }

    /// Whether every stroke's outline can be regenerated (their plans are made).
    pub fn ready(&self) -> bool {
        self.planning.is_none() && self.shapes.iter().all(|s| s.plan.is_some() || s.skeleton.is_empty())
    }

    /// The radius scales, thinnest and boldest, the outlines are made for.
    pub fn weight_range(&self) -> (f64, f64) {
        self.planned_for
    }

    /// Where a point of the em (font units) is in the window, once laid out.
    pub fn font_to_window(&self, p: kurbo::Point) -> Option<Point<Pixels>> {
        self.view().map(|v| v.to_window(p))
    }

    fn view(&self) -> Option<View> {
        let fit = self.fit.get()?;
        Some(View { origin: fit.origin + self.pan, scale: fit.scale * self.zoom })
    }

    fn glyph<'a>(&self, cx: &'a App) -> Option<&'a Glyph> {
        self.document.read(cx).project.glyphs.get(self.glyph)
    }

    fn wght(&self, cx: &App) -> f64 {
        self.weight.read(cx).value().start() as f64
    }

    /// The radius scale of the weight shown.
    fn weights(&self, cx: &App) -> Weights {
        Weights::of(&self.document.read(cx).project.font)
    }

    fn scale(&self, cx: &App) -> f64 {
        self.weights(cx).scale(self.wght(cx))
    }

    fn regenerated(&self) -> bool {
        !self.original
    }

    /// Follows the glyph after a change: keeps the shapes of strokes that are still there.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (shapes, advance) = match self.document.read(cx).project.glyphs.get(self.glyph) {
            Some(glyph) => {
                let mut old = std::mem::take(&mut self.shapes);
                let shapes: Vec<Shapes> = glyph
                    .strokes
                    .iter()
                    .map(|s| match old.iter().position(|o| o.is(s)) {
                        Some(i) => {
                            let mut shapes = old.swap_remove(i);
                            shapes.follow(s.placement);
                            shapes
                        }
                        None => Shapes::new(s),
                    })
                    .collect();
                (shapes, glyph.advance)
            }
            None => (Vec::new(), 0.0),
        };
        let n = shapes.len();
        self.shapes = shapes;
        self.selection.retain(|&i| i < n);
        self.hovered = self.hovered.filter(|&h| h < n);
        self.advance.update(cx, |s, cx| s.set_value(units(advance), window, cx));
        self.regenerate(cx);
        self.ensure_plans(cx);
        cx.notify();
    }

    /// Makes the outline plans strokes lack, on a background thread (again when done, for strokes
    /// added meanwhile).
    fn ensure_plans(&mut self, cx: &mut Context<Self>) {
        // Plans serve the radius scales from the thinnest weight to the boldest, at the pen's
        // width; new ones when those change.
        let (thinnest, _, boldest) = self.weights(cx).scales();
        let pen = self.pen(cx);
        if self.planned_for != (thinnest, boldest) || self.planned_pen != pen {
            self.planned_for = (thinnest, boldest);
            self.planned_pen = pen;
            for s in &mut self.shapes {
                s.plan = None;
                s.regenerated = None;
            }
        }
        if self.planning.is_some() {
            return;
        }
        let jobs: Vec<(Vec<SkeletonPiece>, [f64; 4])> =
            self.shapes.iter().filter(|s| s.plan.is_none() && !s.skeleton.is_empty()).map(|s| (s.skeleton.clone(), s.linear)).collect();
        if jobs.is_empty() {
            return;
        }
        let params = PlanParams { precision: PRECISION * pen, weights: self.planned_for };
        let background = cx.background_executor().clone();
        self.planning = Some(cx.spawn(async move |this, cx| {
            let plans = background
                .spawn(async move {
                    let plans = crate::par_map(&jobs, |(sk, [a, b, c, d])| {
                        let shape = Affine::new([*a, *b, *c, *d, 0.0, 0.0]);
                        Arc::new(scriptoria_outline::plan(&crate::assign::skeleton_of(sk, shape, pen), &params))
                    });
                    jobs.into_iter().zip(plans).collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.planning = None;
                // Plans for weights or a pen since changed are dropped.
                let current = this.planned_for == params.weights && this.planned_pen == pen;
                for ((skeleton, linear), plan) in plans.into_iter().filter(|_| current) {
                    for s in this.shapes.iter_mut().filter(|s| s.plan.is_none() && s.skeleton == skeleton && s.linear == linear) {
                        s.plan = Some(plan.clone());
                    }
                }
                this.regenerate(cx);
                this.ensure_plans(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Regenerates the outlines at the weight shown, where they are not yet.
    fn regenerate(&mut self, cx: &mut Context<Self>) {
        let scale = self.scale(cx);
        let weight = WeightParams::scaled(scale);
        // Regenerated outlines are in font units.
        let tolerance = (TOLERANCE as f64 * self.pen(cx)) as f32;
        for s in &mut self.shapes {
            let Some(plan) = &s.plan else { continue };
            if s.regenerated.as_ref().is_some_and(|r| (r.0 - scale).abs() < 1e-9) {
                continue;
            }
            let path = scriptoria_outline::expand(plan, &weight);
            let mesh = Mesh::from_path_with_tolerance(&path, StrokeShape::Outline { non_zero: true }, true, tolerance).map(Rc::new);
            s.regenerated = Some((scale, path, mesh));
        }
    }

    /// Font units per page unit for stroke widths.
    fn pen(&self, cx: &App) -> f64 {
        self.document.read(cx).project.font.pen_scale()
    }

    // ---- Edits ----

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Glyph)) {
        let glyph = self.glyph;
        self.document.update(cx, |d, cx| d.update_glyph(glyph, f, cx));
    }

    fn selected(&self) -> Vec<usize> {
        self.selection.iter().copied().collect()
    }

    fn nudge(&mut self, dx: f64, dy: f64, cx: &mut Context<Self>) {
        let which = self.selected();
        self.edit(cx, |g| g.transform_strokes(&which, Affine::translate((dx, dy))));
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) {
        let which = self.selected();
        self.selection.clear();
        self.edit(cx, |g| g.remove_strokes(&which));
    }

    fn reorder(&mut self, earlier: bool, cx: &mut Context<Self>) {
        let which = self.selected();
        let mut moved = which.clone();
        self.edit(cx, |g| moved = g.reorder_strokes(&which, earlier));
        self.selection = moved.into_iter().collect();
        cx.notify();
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = (0..self.shapes.len()).collect();
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.clear();
        cx.notify();
    }

    fn type_advance(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        match text.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 => self.edit(cx, |g| g.advance = v.round()),
            _ => {}
        }
        // Shows the advance as it is (also after a typo).
        let advance = self.glyph(cx).map_or(0.0, |g| g.advance);
        self.advance.update(cx, |s, cx| s.set_value(units(advance), window, cx));
    }

    // ---- Geometry ----

    /// The placed bounds of a stroke as shown (font units).
    fn stroke_bounds(&self, glyph: &Glyph, i: usize) -> Option<kurbo::Rect> {
        Some(self.shapes.get(i)?.bounds(glyph.strokes.get(i)?.placement, self.regenerated()))
    }

    fn selection_bounds(&self, glyph: &Glyph) -> Option<kurbo::Rect> {
        self.selection.iter().filter_map(|&i| self.stroke_bounds(glyph, i)).reduce(|a, b| a.union(b))
    }

    /// The topmost stroke (the last written) with ink under a window point.
    fn hit_test(&self, pos: Point<Pixels>, cx: &App) -> Option<usize> {
        let view = self.view()?;
        let glyph = self.glyph(cx)?;
        let q = view.to_font(pos);
        (0..glyph.strokes.len().min(self.shapes.len()))
            .rev()
            .find(|&i| self.shapes[i].hit(glyph.strokes[i].placement, q, HIT_PX / view.scale as f64, self.regenerated()))
    }

    /// What a drag does to the selection so far.
    fn drag_transform(&self) -> Option<Affine> {
        let d = self.drag.filter(|d| d.moved)?;
        let view = self.view()?;
        let (from, to) = (view.to_font(d.start), view.to_font(d.last));
        match d.kind {
            DragKind::Move => {
                let v = to - from;
                Some(Affine::translate((v.x.round(), v.y.round())))
            }
            DragKind::Scale { anchor, corner } => {
                let (c, t) = (corner - anchor, to - anchor);
                Some(scale_about(anchor, (t.dot(c) / c.hypot2().max(1e-9)).max(0.02)))
            }
            _ => None,
        }
    }

    /// The advance being dragged to.
    fn drag_advance(&self) -> Option<f64> {
        let d = self.drag.filter(|d| d.moved)?;
        let DragKind::Advance { from } = d.kind else { return None };
        let view = self.view()?;
        Some((from + view.to_font(d.last).x - view.to_font(d.start).x).round().max(0.0))
    }

    /// The selection box being dragged out (font units).
    fn drag_box(&self) -> Option<kurbo::Rect> {
        let d = self.drag.filter(|d| d.moved && matches!(d.kind, DragKind::Select { .. }))?;
        let view = self.view()?;
        Some(kurbo::Rect::from_points(view.to_font(d.start), view.to_font(d.last)))
    }

    // ---- Mouse ----

    /// Left button: on a corner of the selection box, scales the selection; on the advance line,
    /// moves it; on a stroke, selects it (with Ctrl or Shift, adds or removes it) and moves the
    /// selection; elsewhere, drags out a selection box. Middle button: pans.
    fn on_canvas_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.stop_propagation();
        let add = ev.modifiers.shift || ev.modifiers.secondary(cx);
        let kind = match ev.button {
            MouseButton::Middle => DragKind::Pan,
            MouseButton::Left => self.grab(ev.position, add, cx),
            _ => return,
        };
        self.drag = Some(Drag { button: ev.button, kind, start: ev.position, last: ev.position, moved: false });
        cx.notify();
    }

    fn grab(&mut self, pos: Point<Pixels>, add: bool, cx: &mut Context<Self>) -> DragKind {
        let Some(view) = self.view() else { return DragKind::Select { add } };
        let near = |p: kurbo::Point, r: f32| {
            let d = view.to_window(p) - pos;
            f32::from(d.x).abs() <= r && f32::from(d.y).abs() <= r
        };
        if let Some(glyph) = self.glyph(cx) {
            if let Some(b) = self.selection_bounds(glyph) {
                let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)].map(kurbo::Point::from);
                for (k, &corner) in corners.iter().enumerate() {
                    if near(corner, HANDLE_PX + 2.0) {
                        return DragKind::Scale { anchor: corners[(k + 2) % 4], corner };
                    }
                }
            }
            let advance = view.to_window(kurbo::Point::new(glyph.advance, 0.0)).x;
            if f32::from(pos.x - advance).abs() <= HIT_PX as f32 {
                return DragKind::Advance { from: glyph.advance };
            }
        }
        match self.hit_test(pos, cx) {
            Some(i) if add => {
                if !self.selection.remove(&i) {
                    self.selection.insert(i);
                }
                DragKind::Select { add }
            }
            Some(i) => {
                if !self.selection.contains(&i) {
                    self.selection = BTreeSet::from([i]);
                }
                DragKind::Move
            }
            None => {
                if !add {
                    self.selection.clear();
                }
                DragKind::Select { add }
            }
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(mut drag) = self.drag {
            if ev.pressed_button != Some(drag.button) {
                self.drag = None;
                cx.notify();
                return;
            }
            let d = ev.position - drag.start;
            if !drag.moved && f32::from(d.x).hypot(f32::from(d.y)) >= DRAG_THRESHOLD {
                drag.moved = true;
            }
            if drag.moved {
                if drag.kind == DragKind::Pan {
                    self.pan = self.pan + (ev.position - drag.last);
                }
                drag.last = ev.position;
                cx.notify();
            }
            self.drag = Some(drag);
            return;
        }
        let hit = self.hit_test(ev.position, cx);
        if hit != self.hovered {
            self.hovered = hit;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none_or(|d| d.button != ev.button) {
            return;
        }
        let (transform, advance, area) = (self.drag_transform(), self.drag_advance(), self.drag_box());
        let Some(drag) = self.drag.take() else { return };
        if let Some(t) = transform {
            let which = self.selected();
            self.edit(cx, |g| g.transform_strokes(&which, t));
        }
        if let Some(a) = advance {
            self.edit(cx, |g| g.advance = a);
        }
        if let (Some(area), DragKind::Select { add }, Some(glyph)) = (area, drag.kind, self.glyph(cx)) {
            let inside: Vec<usize> =
                (0..glyph.strokes.len()).filter(|&i| self.stroke_bounds(glyph, i).is_some_and(|b| area.contains(b.center()))).collect();
            if !add {
                self.selection.clear();
            }
            self.selection.extend(inside);
        }
        cx.notify();
    }

    // ---- Zoom and pan ----

    fn zoom_at(&mut self, anchor: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
        let (Some(fit), Some(view)) = (self.fit.get(), self.view()) else { return };
        let zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        let q = view.to_font(anchor);
        let scale = fit.scale * zoom;
        let origin = anchor - point(px(q.x as f32 * scale), px(-q.y as f32 * scale));
        self.zoom = zoom;
        self.pan = origin - fit.origin;
        cx.notify();
    }

    fn zoom_at_center(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some(view) = self.view() {
            // The em square's center stays put.
            let font = &self.document.read(cx).project.font;
            let center = kurbo::Point::new(font.units_per_em as f64 / 2.0, (font.ascender as f64 + font.descender as f64) / 2.0);
            self.zoom_at(view.to_window(center), factor, cx);
        }
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.pan = Point::default();
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(px(20.0));
        if ev.modifiers.secondary(cx) {
            self.zoom_at(ev.position, (f32::from(delta.y) * ZOOM_PER_PX).exp(), cx);
        } else {
            self.pan = self.pan + delta;
            cx.notify();
        }
        cx.stop_propagation();
    }

    // ---- Keys ----

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Keys typed into the advance box are its own.
        if !self.focus.is_focused(window) {
            return;
        }
        let m = ev.keystroke.modifiers;
        let step = if m.shift { NUDGE * 10.0 } else { NUDGE };
        match ev.keystroke.key.as_str() {
            "up" if m.alt => self.reorder(true, cx),
            "down" if m.alt => self.reorder(false, cx),
            "left" => self.nudge(-step, 0.0, cx),
            "right" => self.nudge(step, 0.0, cx),
            "up" => self.nudge(0.0, step, cx),
            "down" => self.nudge(0.0, -step, cx),
            "delete" | "backspace" => self.delete_selection(cx),
            "a" if m.secondary(cx) => self.select_all(cx),
            "escape" => self.clear_selection(cx),
            "=" | "+" => self.zoom_at_center(1.25, cx),
            "-" => self.zoom_at_center(0.8, cx),
            "0" => self.reset_zoom(cx),
            _ => {}
        }
    }

    // ---- Painting ----

    fn render_canvas(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (ink, accent, muted, line, paper) = (theme.foreground, SELECTED, theme.muted_foreground, theme.border, theme.background);
        let advance_color: Hsla = gpui::hsla(0.58, 0.85, 0.5, 1.0);
        let order_color: Hsla = gpui::hsla(0.0, 0.75, 0.52, 1.0);
        // The reference glyph and the guides, faint.
        let reference_color = muted.opacity(0.18);
        let guide = gpui::hsla(0.33, 0.6, 0.45, 0.35);
        let doc = self.document.read(cx);
        let font = &doc.project.font;
        let (ascender, descender, upm) = (font.ascender as f64, font.descender as f64, font.units_per_em as f64);
        let em = kurbo::Rect::new(0.0, descender, upm, ascender);
        let Some(glyph) = doc.project.glyphs.get(self.glyph) else { return div().into_any_element() };
        let advance = self.drag_advance().unwrap_or(glyph.advance);
        let preview = self.drag_transform();
        let regenerated = self.regenerated();
        // Per stroke: mesh and what puts it into the em (with the drag so far), start, color.
        let strokes: Vec<(Option<(Rc<Mesh>, Affine)>, Option<kurbo::Point>, Hsla)> = glyph
            .strokes
            .iter()
            .zip(&self.shapes)
            .enumerate()
            .map(|(i, (s, shapes))| {
                let selected = self.selection.contains(&i);
                let moved = match preview {
                    Some(p) if selected => p,
                    _ => Affine::IDENTITY,
                };
                let start = s.skeleton.first().and_then(|p| p.circles.first()).map(|&(c, _)| moved * s.placement * c);
                let color = if selected {
                    accent
                } else if self.hovered == Some(i) {
                    advance_color
                } else {
                    ink
                };
                (shapes.mesh(s.placement, regenerated).map(|(mesh, t)| (mesh, moved * t)), start, color)
            })
            .collect();
        // The reference font's glyph, underneath.
        let reference = doc
            .reference()
            .and_then(|r| glyph.chars.first().and_then(|&c| r.glyph(c, upm)))
            .and_then(|g| Mesh::from_path_with_tolerance(&g.outline, StrokeShape::Outline { non_zero: true }, false, 1.0))
            .map(Rc::new);
        let selection_box = self.selection_bounds(glyph).map(|b| match preview {
            Some(p) => p.transform_rect_bbox(b),
            None => b,
        });
        let marquee = self.drag_box();
        let fit_cell = self.fit.clone();
        let (zoom, pan) = (self.zoom, self.pan);

        canvas(
            move |bounds, _, _| {
                let fit = View::fit(bounds, em);
                fit_cell.set(Some(fit));
                View { origin: fit.origin + pan, scale: fit.scale * zoom }
            },
            move |bounds, view, window, cx| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(quad(bounds, px(0.0), paper, px(0.0), gpui::transparent_black(), Default::default()));
                    // The em square, then the metric lines across the whole view.
                    window.paint_quad(quad(view.rect(em), px(0.0), muted.opacity(0.05), px(1.0), line, Default::default()));
                    let hline = |window: &mut Window, y: f64, color: Hsla, width: f32| {
                        let w = view.to_window(kurbo::Point::new(0.0, y));
                        let b = Bounds::new(point(bounds.origin.x, w.y - px(width / 2.0)), size(bounds.size.width, px(width)));
                        window.paint_quad(quad(b, px(0.0), color, px(0.0), gpui::transparent_black(), Default::default()));
                    };
                    let vline = |window: &mut Window, x: f64, color: Hsla, width: f32| {
                        let w = view.to_window(kurbo::Point::new(x, 0.0));
                        let b = Bounds::new(point(w.x - px(width / 2.0), bounds.origin.y), size(px(width), bounds.size.height));
                        window.paint_quad(quad(b, px(0.0), color, px(0.0), gpui::transparent_black(), Default::default()));
                    };
                    hline(window, ascender, muted.opacity(0.6), 1.0);
                    hline(window, 0.0, muted, 1.5);
                    hline(window, descender, muted.opacity(0.6), 1.0);
                    vline(window, 0.0, muted, 1.0);
                    vline(window, advance, advance_color, 1.5);
                    let left = bounds.origin.x + px(6.0);
                    for (y, name) in [(ascender, "上伸线"), (0.0, "基线"), (descender, "下伸线")] {
                        let at = point(left, view.to_window(kurbo::Point::new(0.0, y)).y - px(16.0));
                        label(window, cx, &format!("{name} {}", units(y)), at, muted, px(11.0));
                    }
                    let top = point(view.to_window(kurbo::Point::new(advance, 0.0)).x + px(4.0), bounds.origin.y + px(6.0));
                    label(window, cx, &format!("字宽 {}", units(advance)), top, advance_color, px(11.0));

                    let to_window = view.affine();
                    if let Some(mesh) = &reference {
                        window.paint_path(mesh.to_path_affine(to_window), reference_color);
                    }
                    // The face (of a full-width character) and the em's middle lines.
                    let face = em.inflate(-(1.0 - FACE) / 2.0 * em.width(), -(1.0 - FACE) / 2.0 * em.height());
                    window.paint_quad(quad(view.rect(face), px(0.0), gpui::transparent_black(), px(1.0), guide, Default::default()));
                    let middle = em.center();
                    for (a, b) in [((em.x0, middle.y), (em.x1, middle.y)), ((middle.x, em.y0), (middle.x, em.y1))] {
                        let r = view.rect(kurbo::Rect::from_points(a, b));
                        let line = Bounds::new(r.origin - point(px(0.5), px(0.5)), size(r.size.width.max(px(1.0)), r.size.height.max(px(1.0))));
                        window.paint_quad(quad(line, px(0.0), guide, px(0.0), gpui::transparent_black(), Default::default()));
                    }
                    for (mesh, _, color) in &strokes {
                        if let Some((mesh, t)) = mesh {
                            window.paint_path(mesh.to_path_affine(to_window * *t), *color);
                        }
                    }
                    // The writing order, at each stroke's start.
                    for (i, (_, start, _)) in strokes.iter().enumerate() {
                        let Some(start) = start else { continue };
                        let c = view.to_window(*start);
                        let dot = Bounds::new(c - point(px(3.0), px(3.0)), size(px(6.0), px(6.0)));
                        window.paint_quad(quad(dot, px(3.0), order_color, px(1.0), gpui::white(), Default::default()));
                        let badge = Bounds::new(c - point(px(22.0), px(22.0)), size(px(16.0), px(16.0)));
                        window.paint_quad(quad(badge, px(8.0), order_color, px(0.0), gpui::transparent_black(), Default::default()));
                        label_centered(window, cx, &(i + 1).to_string(), badge.center(), gpui::white(), px(10.0));
                    }
                    if let Some(b) = selection_box {
                        let r = view.rect(b);
                        window.paint_quad(quad(r, px(0.0), gpui::transparent_black(), px(1.0), accent, Default::default()));
                        for c in [r.origin, r.top_right(), r.bottom_right(), r.bottom_left()] {
                            let h = Bounds::new(c - point(px(HANDLE_PX), px(HANDLE_PX)), size(px(2.0 * HANDLE_PX), px(2.0 * HANDLE_PX)));
                            window.paint_quad(quad(h, px(1.0), paper, px(1.0), accent, Default::default()));
                        }
                    }
                    if let Some(m) = marquee {
                        window.paint_quad(quad(view.rect(m), px(0.0), accent.opacity(0.08), px(1.0), accent, Default::default()));
                    }
                });
            },
        )
        .size_full()
        .into_any_element()
    }

    fn render_stroke_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (accent, muted, border) = (SELECTED, theme.muted_foreground, theme.border);
        let doc = self.document.read(cx);
        let sources = &doc.project.sources;
        let rows: Vec<_> = self
            .glyph(cx)
            .map(|g| g.strokes.as_slice())
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let from = match &s.origin {
                    Some(o) => {
                        let file = sources.iter().find(|x| x.id == o.source).and_then(|x| x.file.file_name()).map(|n| n.to_string_lossy().into_owned());
                        format!("{} 第 {} 页 #{}", file.unwrap_or_else(|| o.source.clone()), o.page + 1, o.index)
                    }
                    None => "来源不明".into(),
                };
                let selected = self.selection.contains(&i);
                let hovered = self.hovered == Some(i);
                h_flex()
                    .id(("stroke", i))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(accent.opacity(0.15)))
                    .when(hovered && !selected, |d| d.bg(muted.opacity(0.1)))
                    .child(div().w(px(22.0)).text_right().font_weight(gpui::FontWeight::SEMIBOLD).child((i + 1).to_string()))
                    .child(div().text_xs().text_color(muted).overflow_hidden().whitespace_nowrap().child(from))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        let m = ev.modifiers();
                        if m.shift || m.secondary(cx) {
                            if !this.selection.remove(&i) {
                                this.selection.insert(i);
                            }
                        } else {
                            this.selection = BTreeSet::from([i]);
                        }
                        cx.notify();
                    }))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        let now = if *hovered { Some(i) } else { this.hovered.filter(|&h| h != i) };
                        if now != this.hovered {
                            this.hovered = now;
                            cx.notify();
                        }
                    }))
                    .test_support()
            })
            .collect();
        let none = self.selection.is_empty();
        v_flex()
            .w(px(220.0))
            .h_full()
            .border_l_1()
            .border_color(border)
            .child(div().px_2().py_1().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child("笔画（按笔顺）"))
            .child(div().id("stroke-list").flex_1().min_h_0().overflow_y_scroll().children(rows))
            .child(
                h_flex()
                    .gap_1()
                    .p_2()
                    .border_t_1()
                    .border_color(border)
                    .child(Button::new("earlier").small().label("提前").disabled(none).on_click(cx.listener(|this, _, _, cx| this.reorder(true, cx))))
                    .child(Button::new("later").small().label("推后").disabled(none).on_click(cx.listener(|this, _, _, cx| this.reorder(false, cx))))
                    .child(
                        Button::new("delete-strokes")
                            .small()
                            .danger()
                            .label("删除")
                            .disabled(none)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_selection(cx))),
                    ),
            )
    }

    fn status_text(&self, cx: &App) -> String {
        let hints = "点选或框选笔画（Ctrl/Shift 加选，Ctrl+A 全选）· 拖动移动，拖选框四角缩放 · 方向键微调（Shift 加大）· Delete 删除 · Alt+↑/↓ 调整笔顺 · 拖蓝线改字宽 · 滚轮或中键拖动平移，Ctrl+滚轮缩放";
        let (Some(i), Some(glyph)) = (self.hovered, self.glyph(cx)) else { return hints.into() };
        let Some(s) = glyph.strokes.get(i) else { return hints.into() };
        let circles: usize = s.skeleton.iter().map(|p| p.circles.len()).sum();
        let knots = self.shapes.get(i).and_then(|sh| sh.plan.as_ref()).map(|p| p.knot_counts().iter().sum::<usize>());
        let mut text = format!("第 {} 笔 · 骨架 {} 段 {circles} 个圆", i + 1, s.skeleton.len());
        if let Some(k) = knots {
            text += &format!("（轮廓节点 {k} 个）");
        }
        text
    }
}

/// Paints a line of text with its top left at `origin`.
fn label(window: &mut Window, cx: &mut App, text: &str, origin: Point<Pixels>, color: Hsla, size: Pixels) {
    let line = shape_text(window, text, color, size);
    line.paint(origin, size * 1.3, TextAlign::Left, None, window, cx).ok();
}

/// Paints a line of text centered on `center`.
fn label_centered(window: &mut Window, cx: &mut App, text: &str, center: Point<Pixels>, color: Hsla, size: Pixels) {
    let line = shape_text(window, text, color, size);
    let origin = center - point(line.width / 2.0, size * 1.3 / 2.0);
    line.paint(origin, size * 1.3, TextAlign::Left, None, window, cx).ok();
}

fn shape_text(window: &mut Window, text: &str, color: Hsla, size: Pixels) -> gpui::ShapedLine {
    let mut run = window.text_style().to_run(text.len());
    run.color = color;
    window.text_system().shape_line(SharedString::from(text.to_string()), size, &[run], None)
}

impl Render for GlyphView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.document.read(cx);
        let font = doc.project.font.clone();
        let has_reference = doc.reference().is_some();
        let (title, name, count, bearings) = match doc.project.glyphs.get(self.glyph) {
            Some(g) => {
                let code = g.chars.iter().map(|c| format!("U+{:04X}", *c as u32)).collect::<Vec<_>>().join(" ");
                let bearings = g.ink_bounds(&doc.project.font).map(|b| (b.x0, g.advance - b.x1));
                (g.chars.iter().collect::<String>(), format!("{} · {code}", g.name), g.strokes.len(), bearings)
            }
            None => (String::new(), "字形已不存在".into(), 0, None),
        };
        let wght = self.wght(cx);
        let scale = self.scale(cx);
        let theme = cx.theme();
        let (border, muted, background) = (theme.border, theme.muted_foreground, theme.background);
        let separator = || div().w(px(1.0)).h(px(20.0)).bg(border);

        let toolbar = h_flex()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(border)
            .child(div().text_2xl().child(title))
            .child(v_flex().child(div().text_sm().child(name)).child(div().text_xs().text_color(muted).child(format!("{count} 笔"))))
            .child(separator())
            .child(div().text_sm().child("字宽"))
            .child(div().flex_shrink_0().w(px(80.0)).child(Input::new(&self.advance).small().id("advance")))
            .child(div().text_xs().text_color(muted).child(match bearings {
                Some((l, r)) => format!("左边距 {} · 右边距 {}", units(l), units(r)),
                None => String::new(),
            }))
            .child(Button::new("fit-reference").small().label("按参考摆放").disabled(!has_reference).tooltip("照参考字体里这个字的位置和大小摆放（强度见左侧设置）").on_click(cx.listener(
                |this, _, window, cx| {
                    let glyph = this.glyph;
                    if this.document.update(cx, |d, cx| d.fit_to_reference(&[glyph], cx)) == 0 {
                        window.push_notification(Notification::warning("参考字体里没有这个字"), cx);
                    }
                },
            )))
            .child(Button::new("center").small().label("左右居中").on_click(cx.listener({
                let font = font.clone();
                move |this, _, _, cx| {
                    let font = font.clone();
                    this.edit(cx, |g| g.center(&font));
                }
            })))
            .child(Button::new("full-width").small().label("全角放置").tooltip("缩放到字面占字身框的 90%，居中，字宽 1 em").on_click(cx.listener(
                move |this, _, _, cx| {
                    let font = font.clone();
                    this.edit(cx, |g| g.fit_full_width(&font, FACE));
                },
            )));

        // Below the canvas: the weight shown, the zoom, and what the mouse and keys do.
        let view_bar = h_flex()
            .gap_3()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(border)
            .child(div().text_sm().child("字重"))
            .child(div().flex_shrink_0().w(px(200.0)).child(Slider::new(&self.weight)))
            .child(div().flex_shrink_0().w(px(120.0)).text_xs().child(format!("wght {wght:.0} · ×{scale:.2}")))
            .child(
                Button::new("original")
                    .small()
                    .label("原稿")
                    .selected(self.original)
                    .tooltip("显示写下来的笔画，而不是由骨架生成的轮廓")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.original = !this.original;
                        cx.notify();
                    })),
            )
            .when(self.planning.is_some(), |d| d.child(div().text_xs().text_color(muted).child("计算中…")))
            .child(div().flex_1())
            .child(Button::new("zoom-out").small().label("−").on_click(cx.listener(|this, _, _, cx| this.zoom_at_center(0.8, cx))))
            .child(div().w(px(44.0)).text_xs().text_center().child(format!("{:.0}%", self.zoom * 100.0)))
            .child(Button::new("zoom-in").small().label("+").on_click(cx.listener(|this, _, _, cx| this.zoom_at_center(1.25, cx))))
            .child(Button::new("zoom-fit").small().label("适合").on_click(cx.listener(|this, _, _, cx| this.reset_zoom(cx))));

        v_flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .size_full()
            .bg(background)
            .child(toolbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("glyph-canvas")
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(self.render_canvas(cx))
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_canvas_down))
                            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_canvas_down))
                            .on_scroll(cx.listener(Self::on_scroll))
                            .test_support(),
                    )
                    .child(self.render_stroke_list(cx)),
            )
            .child(view_bar)
            .child(div().px_3().pb_1().text_xs().text_color(muted).child(self.status_text(cx)))
    }
}
