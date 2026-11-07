//! Ink viewer: tells strokes apart, replays them in writing order, zooms and pans; in skeleton mode
//! every stroke is regenerated from its skeleton at any weight.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Bounds, ContentMask, Context, FocusHandle, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Path, PathBuilder, Pixels, Point, ScrollEvent, SharedString, Task, Window, canvas, div, hsla,
    point, px, quad, rgb, size,
};
use scriptoria_ink::kurbo;
use scriptoria_ink::{InkDocument, InkPage, Stroke, StrokeShape};
use scriptoria_outline::{Plan, PlanParams, WeightParams};
use scriptoria_skeleton::Skeleton;

use crate::mesh::Mesh;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode {
    /// The original ink color.
    Original,
    /// One color per stroke (neighboring strokes ~222° apart in hue).
    Distinct,
    /// Blue to red in writing order.
    Sequence,
}

impl ColorMode {
    const ALL: [(ColorMode, &'static str); 3] =
        [(ColorMode::Original, "原色"), (ColorMode::Distinct, "区分笔画"), (ColorMode::Sequence, "顺序渐变")];
}

const SPEEDS: [(f32, &str); 3] = [(10.0, "×1"), (40.0, "×4"), (160.0, "×16")];
const PAGE_MARGIN: f32 = 24.0;
const HIT_TOLERANCE_PX: f64 = 5.0;
/// Zoom range, relative to "fit to window".
const ZOOM_RANGE: (f32, f32) = (0.25, 64.0);
/// Zoom exponent per scrolled pixel. One wheel notch on Windows is about 60 px, i.e. ×1.27.
const ZOOM_PER_PX: f32 = 0.004;
/// A drag shorter than this is a click, not a pan.
const DRAG_THRESHOLD: f32 = 3.0;
/// Weight (radius scale) range and step.
const WEIGHT_RANGE: (f32, f32) = (0.3, 2.5);
const WEIGHT_STEP: f32 = 0.05;
/// What regenerated outlines must achieve: the default precision over the weight slider's range.
const PLAN: PlanParams = PlanParams { precision: 0.004, weights: (WEIGHT_RANGE.0 as f64, WEIGHT_RANGE.1 as f64) };

/// Page points → window: `origin + p * scale`.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Layout {
    origin: Point<Pixels>,
    scale: f32,
}

impl Layout {
    /// The whole page, centered in `bounds`.
    fn fit(bounds: Bounds<Pixels>, page: &InkPage) -> Self {
        let avail_w = (f32::from(bounds.size.width) - 2.0 * PAGE_MARGIN).max(1.0);
        let avail_h = (f32::from(bounds.size.height) - 2.0 * PAGE_MARGIN).max(1.0);
        let scale = (avail_w / page.width as f32).min(avail_h / page.height as f32);
        let w = page.width as f32 * scale;
        let h = page.height as f32 * scale;
        let origin = bounds.origin
            + point(px((f32::from(bounds.size.width) - w) / 2.0), px((f32::from(bounds.size.height) - h) / 2.0));
        Self { origin, scale }
    }

    fn to_window(&self, p: kurbo::Point) -> Point<Pixels> {
        self.origin + point(px(p.x as f32 * self.scale), px(p.y as f32 * self.scale))
    }

    fn to_page(&self, p: Point<Pixels>) -> kurbo::Point {
        let d = p - self.origin;
        kurbo::Point::new((f32::from(d.x) / self.scale) as f64, (f32::from(d.y) / self.scale) as f64)
    }

    fn rect(&self, r: kurbo::Rect) -> Bounds<Pixels> {
        let o = self.to_window(kurbo::Point::new(r.x0, r.y0));
        Bounds::new(o, size(px(r.width() as f32 * self.scale), px(r.height() as f32 * self.scale)))
    }

    /// The page rectangle under the window area `bounds`.
    fn page_rect(&self, bounds: Bounds<Pixels>) -> kurbo::Rect {
        kurbo::Rect::from_points(self.to_page(bounds.origin), self.to_page(bounds.bottom_right()))
    }
}

/// One page: its strokes and their pre-tessellated meshes.
struct PageData {
    ink: InkPage,
    meshes: Vec<Option<Mesh>>,
}

impl PageData {
    fn new(ink: InkPage) -> Self {
        let meshes = ink.strokes.iter().map(Mesh::tessellate).collect();
        Self { ink, meshes }
    }
}

/// Skeleton-mode data shared by all weights: every stroke's skeleton and outline plan. Computed
/// once per page, on a background thread.
struct SkeletonBase {
    skeletons: Vec<Skeleton>,
    plans: Vec<Plan>,
}

impl SkeletonBase {
    fn compute(strokes: &[Stroke]) -> Self {
        let skeletons = scriptoria::par_map(strokes, scriptoria::skeleton);
        let plans = scriptoria::par_map(&skeletons, |sk| scriptoria_outline::plan(sk, &PLAN));
        Self { skeletons, plans }
    }

    /// Every stroke's outline at `weight`, tessellated.
    fn meshes(&self, weight: f32) -> Vec<Option<Mesh>> {
        let w = WeightParams::scaled(weight as f64);
        scriptoria::par_map(&self.plans, |plan| {
            let path = scriptoria_outline::expand(plan, &w);
            Mesh::from_path(&path, StrokeShape::Outline { non_zero: true }, true)
        })
    }
}

struct Regenerated {
    weight: f32,
    meshes: Vec<Option<Mesh>>,
}

/// The slider being dragged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    Progress,
    Weight,
}

/// Window-space paths of the strokes inside the viewport, for the current layout. Rebuilt from
/// the meshes (a vertex transform only, which is fast) when the layout, the viewport or the mesh
/// source changes.
#[derive(Default)]
struct PathCache {
    key: Option<(Layout, Bounds<Pixels>, u64)>,
    paths: Vec<Option<Path<Pixels>>>,
}

impl PathCache {
    /// `generation` identifies the mesh source (0 for the original outlines, a new value for every
    /// regeneration).
    fn ensure(&mut self, layout: Layout, viewport: Bounds<Pixels>, strokes: &[Stroke], meshes: &[Option<Mesh>], generation: u64) {
        if self.key == Some((layout, viewport, generation)) && self.paths.len() == meshes.len() {
            return;
        }
        self.key = Some((layout, viewport, generation));
        let view = layout.page_rect(viewport);
        self.paths = strokes
            .iter()
            .zip(meshes)
            .map(|(stroke, mesh)| {
                let mesh = mesh.as_ref()?;
                // Skip strokes outside the viewport (with 1 pt of slack for bolder weights).
                let visible = stroke.bbox.inflate(1.0, 1.0).intersect(view);
                (visible.width() > 0.0 && visible.height() > 0.0).then(|| mesh.to_path(layout.origin, layout.scale))
            })
            .collect();
    }
}

#[derive(Clone, Copy)]
struct Drag {
    button: MouseButton,
    start: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
}

fn rgba_to_hsla([r, g, b, a]: [u8; 4]) -> Hsla {
    gpui::Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }.into()
}

/// Draws a stroke's skeleton on top: the centerline of every piece, and the circles at the outline
/// knots at the current weight.
fn paint_skeleton(window: &mut Window, layout: Layout, sk: &Skeleton, plan: &Plan, weight: &WeightParams) {
    let ink = gpui::black().opacity(0.75);
    for piece in &sk.pieces {
        let dense = piece.densify(8);
        if dense.len() < 2 {
            continue;
        }
        let mut builder = PathBuilder::stroke(px(1.5));
        for (j, s) in dense.iter().enumerate() {
            let p = layout.to_window(s.p);
            if j == 0 { builder.move_to(p) } else { builder.line_to(p) }
        }
        if let Ok(path) = builder.build() {
            window.paint_path(path, ink);
        }
    }
    for s in plan.pieces.iter().flat_map(|p| p.knot_samples()) {
        let r = weight.radius(s.r) as f32 * layout.scale;
        let c = layout.to_window(s.p);
        let b = Bounds::new(c - point(px(r), px(r)), size(px(2.0 * r), px(2.0 * r)));
        window.paint_quad(quad(b, px(r), gpui::transparent_black(), px(1.0), ink.opacity(0.8), Default::default()));
    }
}

/// Mixes `a` and `b` by `t` in RGB; the result is opaque.
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (gpui::Rgba::from(a), gpui::Rgba::from(b));
    let l = |x: f32, y: f32| x + (y - x) * t;
    gpui::Rgba { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: 1.0 }.into()
}

pub struct Viewer {
    focus: FocusHandle,
    doc: InkDocument,
    file_name: SharedString,
    page_index: usize,
    page: Rc<PageData>,
    /// The first `visible` strokes are shown.
    visible: usize,
    hovered: Option<usize>,
    mode: ColorMode,
    speed: usize,
    playing: Option<Task<()>>,
    slider: Option<Slider>,
    /// Zoom relative to "fit to window".
    zoom: f32,
    /// Pan relative to the "fit to window" position.
    pan: Point<Pixels>,
    drag: Option<Drag>,
    /// Skeleton mode: show outlines regenerated from skeletons instead of the originals.
    skeleton_mode: bool,
    /// Weight: radius scale.
    weight: f32,
    skeletons: Option<Arc<SkeletonBase>>,
    regenerated: Option<Rc<Regenerated>>,
    /// Bumped on every regeneration, to invalidate the path cache.
    generation: u64,
    computing: Option<Task<()>>,
    /// Written while painting, read by the event handlers.
    fit: Rc<Cell<Option<Layout>>>,
    viewport: Rc<Cell<Option<Bounds<Pixels>>>>,
    scrub_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    weight_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cache: Rc<RefCell<PathCache>>,
}

impl Viewer {
    pub fn new(doc: InkDocument, file_name: SharedString, cx: &mut Context<Self>) -> Self {
        let page = Rc::new(PageData::new(doc.page(0).expect("the PDF has no pages")));
        Self {
            focus: cx.focus_handle(),
            visible: page.ink.strokes.len(),
            doc,
            file_name,
            page_index: 0,
            page,
            hovered: None,
            mode: ColorMode::Distinct,
            speed: 0,
            playing: None,
            slider: None,
            zoom: 1.0,
            pan: Point::default(),
            drag: None,
            skeleton_mode: false,
            weight: 1.0,
            skeletons: None,
            regenerated: None,
            generation: 0,
            computing: None,
            fit: Rc::default(),
            viewport: Rc::default(),
            scrub_bounds: Rc::default(),
            weight_bounds: Rc::default(),
            cache: Rc::default(),
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    fn total(&self) -> usize {
        self.page.ink.strokes.len()
    }

    /// The current layout: fit-to-window × zoom + pan.
    fn layout(&self) -> Option<Layout> {
        self.fit.get().map(|fit| Layout { origin: fit.origin + self.pan, scale: fit.scale * self.zoom })
    }

    fn set_visible(&mut self, n: usize, cx: &mut Context<Self>) {
        let n = n.min(self.total());
        if n != self.visible {
            self.visible = n;
            if self.hovered.is_some_and(|h| h >= n) {
                self.hovered = None;
            }
            cx.notify();
        }
    }

    fn go_to_page(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.doc.page_count() || index == self.page_index {
            return;
        }
        if let Some(ink) = self.doc.page(index) {
            self.playing = None;
            self.page_index = index;
            self.page = Rc::new(PageData::new(ink));
            self.visible = self.total();
            self.hovered = None;
            self.skeletons = None;
            self.regenerated = None;
            self.computing = None;
            *self.cache.borrow_mut() = PathCache::default();
            if self.skeleton_mode {
                self.recompute(cx);
            }
            self.reset_zoom(cx);
        }
    }

    // ---- Skeleton mode ----

    fn toggle_skeleton(&mut self, cx: &mut Context<Self>) {
        self.skeleton_mode = !self.skeleton_mode;
        if self.skeleton_mode && self.regenerated.as_ref().is_none_or(|r| r.weight != self.weight) {
            self.recompute(cx);
        }
        cx.notify();
    }

    fn set_weight(&mut self, w: f32, cx: &mut Context<Self>) {
        let w = ((w / WEIGHT_STEP).round() * WEIGHT_STEP).clamp(WEIGHT_RANGE.0, WEIGHT_RANGE.1);
        if (w - self.weight).abs() < 1e-6 {
            return;
        }
        self.weight = w;
        if self.skeleton_mode && self.computing.is_none() {
            self.recompute(cx);
        }
        cx.notify();
    }

    /// Computes the skeletons (once per page) and the meshes for the current weight on a
    /// background thread. If the weight changed meanwhile, computes again when done.
    fn recompute(&mut self, cx: &mut Context<Self>) {
        let base = self.skeletons.clone();
        let strokes = base.is_none().then(|| self.page.ink.strokes.clone());
        let weight = self.weight;
        let background = cx.background_executor().clone();
        self.computing = Some(cx.spawn(async move |this, cx| {
            let (base, meshes) = background
                .spawn(async move {
                    let base = base.unwrap_or_else(|| Arc::new(SkeletonBase::compute(&strokes.unwrap_or_default())));
                    let meshes = base.meshes(weight);
                    (base, meshes)
                })
                .await;
            this.update(cx, |this, cx| {
                this.computing = None;
                this.skeletons = Some(base);
                this.regenerated = Some(Rc::new(Regenerated { weight, meshes }));
                this.generation += 1;
                if this.skeleton_mode && (this.weight - weight).abs() > 1e-6 {
                    this.recompute(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ---- Zoom and pan ----

    /// Zooms around window point `anchor` (the page point under it stays put).
    fn zoom_at(&mut self, anchor: Point<Pixels>, factor: f32, cx: &mut Context<Self>) {
        let (Some(fit), Some(layout)) = (self.fit.get(), self.layout()) else { return };
        let zoom = (self.zoom * factor).clamp(ZOOM_RANGE.0, ZOOM_RANGE.1);
        if zoom == self.zoom {
            return;
        }
        let q = layout.to_page(anchor);
        let scale = fit.scale * zoom;
        let origin = anchor - point(px(q.x as f32 * scale), px(q.y as f32 * scale));
        self.zoom = zoom;
        self.pan = origin - fit.origin;
        cx.notify();
    }

    fn zoom_at_center(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some(v) = self.viewport.get() {
            self.zoom_at(v.center(), factor, cx);
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
            // Ctrl + wheel, or a touchpad pinch: zoom.
            self.zoom_at(ev.position, (f32::from(delta.y) * ZOOM_PER_PX).exp(), cx);
        } else {
            // Wheel: pan (Shift + wheel pans horizontally).
            self.pan = self.pan + delta;
            cx.notify();
        }
        cx.stop_propagation();
    }

    // ---- Playback ----

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing.take().is_some() {
            cx.notify();
            return;
        }
        if self.visible >= self.total() {
            self.visible = 0;
        }
        let task = cx.spawn(async move |this, cx| {
            const TICK: Duration = Duration::from_millis(16);
            let mut acc = 0.0f32;
            loop {
                cx.background_executor().timer(TICK).await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        acc += SPEEDS[this.speed].0 * TICK.as_secs_f32();
                        let step = acc.floor() as usize;
                        acc -= step as f32;
                        this.set_visible(this.visible + step, cx);
                        if this.visible >= this.total() {
                            this.playing = None;
                            cx.notify();
                            false
                        } else {
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        });
        self.playing = Some(task);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let step = if ev.keystroke.modifiers.shift { 10 } else { 1 };
        match ev.keystroke.key.as_str() {
            "right" => self.set_visible(self.visible + step, cx),
            "left" => self.set_visible(self.visible.saturating_sub(step), cx),
            "home" => self.set_visible(0, cx),
            "end" => self.set_visible(self.total(), cx),
            "space" => self.toggle_play(cx),
            "pagedown" => self.go_to_page(self.page_index + 1, cx),
            "pageup" => self.go_to_page(self.page_index.saturating_sub(1), cx),
            "=" | "+" => self.zoom_at_center(1.25, cx),
            "-" => self.zoom_at_center(0.8, cx),
            "0" => self.reset_zoom(cx),
            "k" => self.toggle_skeleton(cx),
            "[" => self.set_weight(self.weight - 0.1, cx),
            "]" => self.set_weight(self.weight + 0.1, cx),
            "1" | "2" | "3" => {
                let i = ev.keystroke.key.parse::<usize>().unwrap() - 1;
                self.mode = ColorMode::ALL[i].0;
                cx.notify();
            }
            _ => {}
        }
    }

    // ---- Mouse ----

    fn on_canvas_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.drag = Some(Drag { button: ev.button, start: ev.position, last: ev.position, moved: false });
        cx.stop_propagation();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(slider) = self.slider {
            if ev.pressed_button == Some(MouseButton::Left) {
                self.slide(slider, ev.position.x, cx);
            } else {
                self.slider = None;
            }
            return;
        }
        if let Some(mut drag) = self.drag {
            if ev.pressed_button != Some(drag.button) {
                self.drag = None;
                return;
            }
            let from_start = ev.position - drag.start;
            if !drag.moved && f32::from(from_start.x).hypot(f32::from(from_start.y)) >= DRAG_THRESHOLD {
                drag.moved = true;
                drag.last = drag.start;
            }
            if drag.moved {
                self.pan = self.pan + (ev.position - drag.last);
                drag.last = ev.position;
                cx.notify();
            }
            self.drag = Some(drag);
            return;
        }
        self.update_hover(ev.position, cx);
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.slider = None;
        if let Some(drag) = self.drag.take().filter(|d| d.button == ev.button) {
            if drag.moved {
                self.update_hover(ev.position, cx);
            } else if drag.button == MouseButton::Left
                && let Some(i) = self.hovered
            {
                // Clicking a stroke moves playback to it.
                self.playing = None;
                self.set_visible(i + 1, cx);
            }
        }
    }

    fn update_hover(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(layout) = self.layout() else { return };
        let inside = self.viewport.get().is_some_and(|v| v.contains(&pos));
        let hit = inside
            .then(|| {
                let p = layout.to_page(pos);
                self.page.ink.hit_test(p, HIT_TOLERANCE_PX / layout.scale as f64, self.visible)
            })
            .flatten();
        if hit != self.hovered {
            self.hovered = hit;
            cx.notify();
        }
    }

    /// Sets a slider's value from the mouse x coordinate.
    fn slide(&mut self, slider: Slider, x: Pixels, cx: &mut Context<Self>) {
        let cell = match slider {
            Slider::Progress => &self.scrub_bounds,
            Slider::Weight => &self.weight_bounds,
        };
        let Some(b) = cell.get() else { return };
        let t = (f32::from(x - b.origin.x) / f32::from(b.size.width)).clamp(0.0, 1.0);
        match slider {
            Slider::Progress => self.set_visible((t * self.total() as f32).round() as usize, cx),
            Slider::Weight => self.set_weight(WEIGHT_RANGE.0 + t * (WEIGHT_RANGE.1 - WEIGHT_RANGE.0), cx),
        }
    }

    // ---- Painting ----

    fn stroke_color(&self, i: usize) -> Hsla {
        let n = self.total().max(2);
        match self.mode {
            ColorMode::Original => rgba_to_hsla(self.page.ink.strokes[i].rgba),
            ColorMode::Distinct => hsla((i as f32 * 0.618_034) % 1.0, 0.75, 0.45, 1.0),
            ColorMode::Sequence => {
                let t = i as f32 / (n - 1) as f32;
                hsla(0.66 * (1.0 - t), 0.8, 0.45, 1.0)
            }
        }
    }

    fn render_page_canvas(&self) -> impl IntoElement {
        let page = self.page.clone();
        let cache = self.cache.clone();
        let fit_cell = self.fit.clone();
        let viewport_cell = self.viewport.clone();
        let (zoom, pan) = (self.zoom, self.pan);
        let visible = self.visible;
        let colors: Vec<Hsla> = (0..page.ink.strokes.len()).map(|i| self.stroke_color(i)).collect();
        let current = visible.checked_sub(1);
        let hovered = self.hovered;
        // In skeleton mode, use the regenerated meshes (the originals until they are ready).
        let regenerated = self.regenerated.clone().filter(|_| self.skeleton_mode);
        let generation = if regenerated.is_some() { self.generation } else { 0 };
        let skeletons = self.skeletons.clone().filter(|_| self.skeleton_mode);
        let weight = WeightParams::scaled(self.regenerated.as_ref().map_or(self.weight, |r| r.weight) as f64);

        canvas(
            move |bounds, _, _| {
                let fit = Layout::fit(bounds, &page.ink);
                fit_cell.set(Some(fit));
                viewport_cell.set(Some(bounds));
                let layout = Layout { origin: fit.origin + pan, scale: fit.scale * zoom };
                let meshes = regenerated.as_ref().map_or(&page.meshes[..], |r| &r.meshes[..]);
                cache.borrow_mut().ensure(layout, bounds, &page.ink.strokes, meshes, generation);
                (layout, page, cache, skeletons)
            },
            move |bounds, (layout, page, cache, skeletons), window, _| {
                // Paint inside the canvas only, so zoomed content never covers the toolbars.
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let ink = &page.ink;
                    let page_bounds = layout.rect(kurbo::Rect::new(0.0, 0.0, ink.width, ink.height));
                    let paper = ink.paper.map(rgba_to_hsla).unwrap_or(gpui::white());
                    window.paint_quad(quad(page_bounds, px(2.0), paper, px(1.0), rgb(0xC8C8C8), Default::default()));

                    let cache = cache.borrow();
                    // Strokes not yet "written" are shown faintly, painted first (underneath). They use
                    // a color pre-mixed with the paper: translucency would darken where the small
                    // pieces of a stroke overlap.
                    for (i, path) in cache.paths.iter().enumerate().skip(visible) {
                        if let Some(path) = path {
                            window.paint_path(path.clone(), mix(paper, colors[i], 0.12));
                        }
                    }
                    for (i, path) in cache.paths.iter().enumerate().take(visible) {
                        if let Some(path) = path {
                            window.paint_path(path.clone(), colors[i]);
                        }
                    }

                    let mut mark = |i: usize, accent: Hsla| {
                        let s = &ink.strokes[i];
                        let r = layout.rect(s.bbox).dilate(px(3.0));
                        window.paint_quad(quad(r, px(3.0), gpui::transparent_black(), px(1.5), accent, Default::default()));
                        if let Some(path) = &cache.paths[i] {
                            window.paint_path(path.clone(), accent);
                        }
                        if let Some(sk) = &skeletons {
                            paint_skeleton(window, layout, &sk.skeletons[i], &sk.plans[i], &weight);
                        }
                        for (p, color) in [(s.start, rgb(0x16A34A)), (s.end, rgb(0xDC2626))] {
                            let c = layout.to_window(p);
                            let dot = Bounds::new(c - point(px(4.0), px(4.0)), size(px(8.0), px(8.0)));
                            window.paint_quad(quad(dot, px(4.0), color, px(1.5), gpui::white(), Default::default()));
                        }
                    };
                    if let Some(i) = current {
                        mark(i, hsla(0.11, 0.95, 0.5, 1.0));
                    }
                    if let Some(i) = hovered.filter(|h| Some(*h) != current) {
                        mark(i, hsla(0.58, 0.9, 0.5, 1.0));
                    }
                });
            },
        )
        .size_full()
    }

    fn render_scrubber(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.scrub_bounds.clone();
        let total = self.total();
        let frac = if total == 0 { 0.0 } else { self.visible as f32 / total as f32 };
        let hovered_frac = self.hovered.map(|h| (h as f32 + 0.5) / total.max(1) as f32);

        div()
            .id("scrubber")
            .flex_1()
            .h(px(24.0))
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(Some(bounds));
                        bounds
                    },
                    move |_, bounds, window, _| {
                        let track_h = px(6.0);
                        let y = bounds.origin.y + (bounds.size.height - track_h) / 2.0;
                        let track = Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, track_h));
                        window.paint_quad(quad(track, px(3.0), rgb(0xDDDDDD), px(0.0), gpui::transparent_black(), Default::default()));
                        let filled = Bounds::new(track.origin, size(bounds.size.width * frac, track_h));
                        window.paint_quad(quad(filled, px(3.0), hsla(0.11, 0.95, 0.5, 1.0), px(0.0), gpui::transparent_black(), Default::default()));
                        if let Some(hf) = hovered_frac {
                            let x = bounds.origin.x + bounds.size.width * hf;
                            let tick = Bounds::new(point(x - px(1.0), bounds.origin.y + px(2.0)), size(px(2.0), bounds.size.height - px(4.0)));
                            window.paint_quad(quad(tick, px(1.0), hsla(0.58, 0.9, 0.5, 1.0), px(0.0), gpui::transparent_black(), Default::default()));
                        }
                        let thumb_c = point(bounds.origin.x + bounds.size.width * frac, bounds.origin.y + bounds.size.height / 2.0);
                        let thumb = Bounds::new(thumb_c - point(px(7.0), px(7.0)), size(px(14.0), px(14.0)));
                        window.paint_quad(quad(thumb, px(7.0), gpui::white(), px(2.0), hsla(0.11, 0.95, 0.5, 1.0), Default::default()));
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.playing = None;
                    this.slider = Some(Slider::Progress);
                    this.slide(Slider::Progress, ev.position.x, cx);
                }),
            )
    }

    fn render_weight_slider(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds_cell = self.weight_bounds.clone();
        let frac = (self.weight - WEIGHT_RANGE.0) / (WEIGHT_RANGE.1 - WEIGHT_RANGE.0);
        let at_one = (1.0 - WEIGHT_RANGE.0) / (WEIGHT_RANGE.1 - WEIGHT_RANGE.0);
        let accent = hsla(0.58, 0.9, 0.5, 1.0);
        div()
            .id("weight-slider")
            .w(px(160.0))
            .h(px(24.0))
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| {
                        bounds_cell.set(Some(bounds));
                        bounds
                    },
                    move |_, bounds, window, _| {
                        let track_h = px(4.0);
                        let y = bounds.origin.y + (bounds.size.height - track_h) / 2.0;
                        let track = Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, track_h));
                        window.paint_quad(quad(track, px(2.0), rgb(0xDDDDDD), px(0.0), gpui::transparent_black(), Default::default()));
                        // Tick at ×1 (the original weight).
                        let x1 = bounds.origin.x + bounds.size.width * at_one;
                        let tick = Bounds::new(point(x1 - px(1.0), bounds.origin.y + px(5.0)), size(px(2.0), bounds.size.height - px(10.0)));
                        window.paint_quad(quad(tick, px(1.0), rgb(0x9CA3AF), px(0.0), gpui::transparent_black(), Default::default()));
                        let c = point(bounds.origin.x + bounds.size.width * frac, bounds.origin.y + bounds.size.height / 2.0);
                        let thumb = Bounds::new(c - point(px(7.0), px(7.0)), size(px(14.0), px(14.0)));
                        window.paint_quad(quad(thumb, px(7.0), gpui::white(), px(2.0), accent, Default::default()));
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.slider = Some(Slider::Weight);
                    this.slide(Slider::Weight, ev.position.x, cx);
                }),
            )
    }

    fn button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        active: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let id: SharedString = id.into();
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(active, |d| d.bg(rgb(0x1F2937)).text_color(gpui::white()))
            .when(!active, |d| d.bg(rgb(0xF3F4F6)).text_color(rgb(0x1F2937)).hover(|d| d.bg(rgb(0xE5E7EB))))
            .child(label.into())
            .on_click(cx.listener(move |this, _, _, cx| {
                on_click(this, cx);
                cx.notify();
            }))
    }

    fn status_text(&self) -> String {
        let Some(i) = self.hovered else {
            return "悬停查看笔画 · 点击跳到该笔 · 拖动/滚轮平移 · Ctrl+滚轮缩放（+/- 键，0 复位）· ←/→ 单步（Shift ×10）· 空格播放 · 1/2/3 配色 · K 骨架模式 · [ ] 字重".into();
        };
        let s = &self.page.ink.strokes[i];
        let kind = match s.shape {
            StrokeShape::Outline { .. } => format!("填充轮廓，{} 段", s.subpaths),
            StrokeShape::Line { width } => format!("描边线，线宽 {width:.2} pt"),
        };
        let [r, g, b, _] = s.rgba;
        let mut text = format!(
            "第 {} 笔 · {kind} · 墨色 #{r:02X}{g:02X}{b:02X} · 落笔 ({:.1}, {:.1}) → 收笔 ({:.1}, {:.1})",
            i + 1,
            s.start.x,
            s.start.y,
            s.end.x,
            s.end.y
        );
        if let Some(base) = self.skeletons.as_ref().filter(|_| self.skeleton_mode) {
            let sk = &base.skeletons[i];
            let (lo, hi) = sk.samples().fold((f64::INFINITY, 0.0f64), |(lo, hi), s| (lo.min(s.r), hi.max(s.r)));
            let knots: usize = base.plans[i].knot_counts().iter().sum();
            text += &format!(
                " · 骨架 {} 段 {} 个圆（轮廓节点 {knots} 个）· 半径 {lo:.3}–{hi:.3} pt",
                sk.pieces.len(),
                sk.circle_count()
            );
        }
        text
    }
}

impl Render for Viewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.total();
        let page_count = self.doc.page_count();
        let playing = self.playing.is_some();

        let mut mode_buttons = div().flex().gap_1();
        for (i, (mode, label)) in ColorMode::ALL.into_iter().enumerate() {
            mode_buttons = mode_buttons.child(self.button(
                SharedString::from(format!("mode-{i}")),
                label,
                self.mode == mode,
                cx,
                move |this, _| this.mode = mode,
            ));
        }

        let zoom_controls = div()
            .flex()
            .items_center()
            .gap_1()
            .child(self.button("zoom-out", "−", false, cx, |this, cx| this.zoom_at_center(0.8, cx)))
            .child(div().w(px(56.0)).text_sm().text_center().child(format!("{:.0}%", self.zoom * 100.0)))
            .child(self.button("zoom-in", "+", false, cx, |this, cx| this.zoom_at_center(1.25, cx)))
            .child(self.button("zoom-fit", "适合窗口", false, cx, |this, cx| this.reset_zoom(cx)));

        let skeleton_controls = div()
            .flex()
            .items_center()
            .gap_2()
            .child(self.button("skeleton", "骨架", self.skeleton_mode, cx, |this, cx| this.toggle_skeleton(cx)))
            .when(self.skeleton_mode, |d| {
                d.child(div().text_sm().text_color(rgb(0x6B7280)).child("字重"))
                    .child(self.render_weight_slider(cx))
                    .child(div().w(px(44.0)).text_sm().child(format!("×{:.2}", self.weight)))
                    .when(self.computing.is_some(), |d| d.child(div().text_xs().text_color(rgb(0x6B7280)).child("计算中…")))
            });

        let top_bar = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .bg(gpui::white())
            .border_b_1()
            .border_color(rgb(0xE5E7EB))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.file_name.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x6B7280))
                    .child(format!("第 {}/{} 页 · 共 {total} 笔", self.page_index + 1, page_count)),
            )
            .when(page_count > 1, |d| {
                d.child(self.button("prev-page", "上一页", false, cx, |this, cx| this.go_to_page(this.page_index.saturating_sub(1), cx)))
                    .child(self.button("next-page", "下一页", false, cx, |this, cx| this.go_to_page(this.page_index + 1, cx)))
            })
            .child(div().flex_1())
            .child(skeleton_controls)
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(0xE5E7EB)))
            .child(zoom_controls)
            .child(div().w(px(1.0)).h(px(20.0)).bg(rgb(0xE5E7EB)))
            .child(mode_buttons);

        let bottom_bar = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_4()
            .py_2()
            .bg(gpui::white())
            .border_t_1()
            .border_color(rgb(0xE5E7EB))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(self.button("play", if playing { "暂停" } else { "播放" }, playing, cx, |this, cx| this.toggle_play(cx)))
                    .child(self.button("speed", SPEEDS[self.speed].1, false, cx, |this, _| this.speed = (this.speed + 1) % SPEEDS.len()))
                    .child(self.render_scrubber(cx))
                    .child(div().w(px(120.0)).text_sm().text_right().child(format!("{} / {total} 笔", self.visible))),
            )
            .child(div().text_xs().text_color(rgb(0x6B7280)).child(self.status_text()));

        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0xEDEDED))
            .text_color(rgb(0x111827))
            .child(top_bar)
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(self.render_page_canvas())
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_canvas_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_canvas_down))
                    .on_scroll(cx.listener(Self::on_scroll)),
            )
            .child(bottom_bar)
    }
}

