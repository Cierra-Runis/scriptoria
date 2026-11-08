//! One glyph, as in FontForge's glyph window: its strokes in the em with the metric lines
//! (ascender, baseline, descender, advance) and each stroke's place in the writing order at its
//! start, at any weight of the axis; on demand the outlines' points and handles, as cubic curves or
//! as the font's quadratic splines. Strokes are selected (click, box, the list beside) and moved
//! (drag, arrow keys), scaled (the selection box's corners), stretched (its edges), turned (the
//! handle above it), turned, flipped, stretched and slanted by the numbers (the panel beside),
//! deleted and reordered; their widths stay the pen's through all of it. The advance is dragged or
//! typed. Every change is one step to undo.

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
    PathBuilder, div, point, px, quad, size,
};
use scriptoria_ink::StrokeShape;
use scriptoria_ink::kurbo::{self, Affine, BezPath, ParamCurveNearest as _, Shape as _};
use scriptoria_outline::{Plan, PlanParams, WeightParams};
use scriptoria_project::{Glyph, GlyphStroke, Outline, OutlineKind, SkeletonPiece};

use crate::app::document::{Document, DocumentEvent};
use crate::app::mesh::Mesh;
use crate::app::theme::palette;
use crate::font::{AXIS, Weights};

/// Tessellation tolerance (pt, the strokes' own units): an em of a few pt is some 600 px here.
const TOLERANCE: f32 = 0.001;
/// How closely regenerated outlines follow the strokes (pt, made font units by the pen scale), as
/// in the font.
const PRECISION: f64 = 0.004;
/// How near (px) the pointer must be to a stroke, a handle or the advance line to grab it.
const HIT_PX: f64 = 4.0;
const HANDLE_PX: f32 = 4.0;
/// How far (px) the turning handle sits above the selection box.
const ROTATE_PX: f32 = 22.0;
/// A drag shorter than this (px) is a click.
const DRAG_THRESHOLD: f32 = 3.0;
/// Zoom range, relative to "fit to window", and zoom per scrolled pixel.
const ZOOM_RANGE: (f32, f32) = (0.2, 40.0);
const ZOOM_PER_PX: f32 = 0.004;
/// Arrow keys move the selection this far (font units); ten times as far with Shift.
const NUDGE: f64 = 10.0;
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

/// What a stroke's regenerated outline depends on besides the stroke itself: the linear part of
/// its placement (its scale and flip, not where it puts the stroke) and its pen.
fn form(s: &GlyphStroke) -> [f64; 5] {
    let [a, b, c, d, _, _] = s.placement.as_coeffs();
    [a, b, c, d, s.pen]
}

/// What a stroke looks like, made from what it is and the linear part of its placement, so that
/// it is kept while the stroke is moved.
struct Shapes {
    skeleton: Vec<SkeletonPiece>,
    outline: Outline,
    /// The outline as written (page coordinates).
    original: Option<Rc<Mesh>>,
    /// What the plan and the regenerated outline are for.
    linear: [f64; 5],
    plan: Option<Arc<Plan>>,
    /// The outline regenerated at a radius scale: (scale, outline, mesh), in font units before the
    /// placement's translation. Its strokes keep the pen's width (see `FontInfo::pen_scale`).
    regenerated: Option<(f64, BezPath, Option<Rc<Mesh>>)>,
    /// Until it is: the ink drawn quickly from the skeleton ([`capsules`]), for a radius scale.
    quick: std::cell::RefCell<Option<(f64, Option<Rc<Mesh>>)>>,
}

impl Shapes {
    fn new(s: &GlyphStroke) -> Self {
        let original = Mesh::from_path_with_tolerance(&s.outline.path, shape(s.outline.kind), true, TOLERANCE).map(Rc::new);
        Shapes { skeleton: s.skeleton.clone(), outline: s.outline.clone(), original, linear: form(s), plan: None, regenerated: None, quick: Default::default() }
    }

    /// Whether these are the shapes of a stroke (wherever it is placed).
    fn is(&self, s: &GlyphStroke) -> bool {
        self.skeleton == s.skeleton && self.outline == s.outline
    }

    /// Follows a change of the placement's scale or flip, or of the pen: the regenerated outline is
    /// made again.
    fn follow(&mut self, s: &GlyphStroke) {
        let l = form(s);
        if self.linear.iter().zip(&l).any(|(a, b)| (a - b).abs() > 1e-9) {
            self.linear = l;
            self.plan = None;
            self.regenerated = None;
            self.quick.take();
        }
    }

    fn regenerated(&self, wanted: bool) -> Option<&(f64, BezPath, Option<Rc<Mesh>>)> {
        self.regenerated.as_ref().filter(|_| wanted)
    }

    /// The mesh shown and what puts it into font units: the regenerated outline; until that is made
    /// (after a change of the stroke's shape), the ink drawn from the skeleton at radius scale
    /// `scale`, so that the stroke keeps its width meanwhile; the outline as written when asked.
    fn mesh(&self, placement: Affine, regenerated: bool, scale: f64) -> Option<(Rc<Mesh>, Affine)> {
        let at = Affine::translate(placement.translation());
        match self.regenerated(regenerated) {
            Some((_, _, mesh)) => Some((mesh.clone()?, at)),
            None if regenerated && !self.skeleton.is_empty() => {
                let mut quick = self.quick.borrow_mut();
                if quick.as_ref().is_none_or(|q| (q.0 - scale).abs() > 1e-9) {
                    let [a, b, c, d, pen] = self.linear;
                    *quick = Some((scale, capsules(&self.skeleton, Affine::new([a, b, c, d, 0.0, 0.0]), pen * scale)));
                }
                Some((quick.as_ref()?.1.clone()?, at))
            }
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

/// Which points of the outlines are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nodes {
    Hidden,
    /// The cubic curves as made from the skeleton.
    Cubic,
    /// The quadratic splines the font gets (see [`scriptoria_font::glyf_points`]).
    Quadratic,
}

/// A mark of an outline's points (font units).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Node {
    On(kurbo::Point),
    Off(kurbo::Point),
    /// An on-curve point the font leaves out (halfway between two off-curve ones).
    Implied(kurbo::Point),
    /// A line from a point on the curve to its off-curve point (or between off-curve points).
    Handle(kurbo::Point, kurbo::Point),
}

/// The marks of an outline's points, as cubic curves or as the font's quadratic splines.
fn nodes_of(path: &BezPath, nodes: Nodes) -> Vec<Node> {
    let mut out = Vec::new();
    match nodes {
        Nodes::Hidden => {}
        Nodes::Cubic => {
            let mut last = kurbo::Point::ZERO;
            for el in path.elements() {
                match *el {
                    kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => {
                        out.push(Node::On(p));
                        last = p;
                    }
                    kurbo::PathEl::QuadTo(c, p) => {
                        out.extend([Node::Handle(last, c), Node::Handle(c, p), Node::Off(c), Node::On(p)]);
                        last = p;
                    }
                    kurbo::PathEl::CurveTo(c1, c2, p) => {
                        out.extend([Node::Handle(last, c1), Node::Handle(c2, p), Node::Off(c1), Node::Off(c2), Node::On(p)]);
                        last = p;
                    }
                    kurbo::PathEl::ClosePath => {}
                }
            }
        }
        Nodes::Quadratic => {
            for contour in scriptoria_font::glyf_points(path, crate::font::QUADRATIC_ACCURACY) {
                let n = contour.len();
                for (k, &(x, y, on)) in contour.iter().enumerate() {
                    let p = kurbo::Point::new(x, y);
                    let (nx, ny, next_on) = contour[(k + 1) % n];
                    let next = kurbo::Point::new(nx, ny);
                    // The control polygon, but not along straight segments.
                    if !(on && next_on) {
                        out.push(Node::Handle(p, next));
                    }
                    if !on && !next_on {
                        out.push(Node::Implied(p.midpoint(next)));
                    }
                    out.push(if on { Node::On(p) } else { Node::Off(p) });
                }
            }
        }
    }
    out
}

/// Whether a transform only moves things.
fn is_translation(t: Affine) -> bool {
    let [a, b, c, d, _, _] = t.as_coeffs();
    (a - 1.0).abs() < 1e-12 && b.abs() < 1e-12 && c.abs() < 1e-12 && (d - 1.0).abs() < 1e-12
}

/// A stroke's ink drawn quickly from its skeleton (while its outline is being made): each circle,
/// and the band between each two along a piece, at `shape` (the linear part of a placement), the
/// radii times `radius` (font units).
fn capsules(skeleton: &[SkeletonPiece], shape: Affine, radius: f64) -> Option<Rc<Mesh>> {
    // Every shape counterclockwise (y up), so that the non-zero fill is their union.
    fn polygon(path: &mut BezPath, points: &[kurbo::Point]) {
        let area: f64 = (0..points.len()).map(|i| points[i].to_vec2().cross(points[(i + 1) % points.len()].to_vec2())).sum();
        let ordered: Vec<kurbo::Point> = if area < 0.0 { points.iter().rev().copied().collect() } else { points.to_vec() };
        path.move_to(ordered[0]);
        for p in &ordered[1..] {
            path.line_to(*p);
        }
        path.close_path();
    }
    const SIDES: usize = 16;
    let mut path = BezPath::new();
    let mut largest: f64 = 0.0;
    for piece in skeleton {
        let circles: Vec<(kurbo::Point, f64)> = piece.circles.iter().map(|&(c, r)| (shape * c, r * radius)).collect();
        for &(c, r) in &circles {
            largest = largest.max(r);
            let ring: Vec<kurbo::Point> = (0..SIDES).map(|k| c + kurbo::Vec2::from_angle(k as f64 * std::f64::consts::TAU / SIDES as f64) * r).collect();
            polygon(&mut path, &ring);
        }
        for w in circles.windows(2) {
            let ((c0, r0), (c1, r1)) = (w[0], w[1]);
            let d = c1 - c0;
            let len = d.hypot();
            // One circle within the other: the larger one is the band.
            if len <= (r0 - r1).abs() {
                continue;
            }
            let (u, a) = (d / len, (r0 - r1) / len);
            let b = (1.0 - a * a).max(0.0).sqrt();
            let normal = |side: f64| u * a + kurbo::Vec2::new(-u.y, u.x) * (b * side);
            let (n0, n1) = (normal(1.0), normal(-1.0));
            polygon(&mut path, &[c0 + n0 * r0, c1 + n0 * r1, c1 + n1 * r1, c0 + n1 * r0]);
        }
    }
    if path.elements().is_empty() {
        return None;
    }
    // A tenth of a pixel or so at the sizes shown.
    let tolerance = (largest * 0.02).max(0.05) as f32;
    Mesh::from_path_with_tolerance(&path, StrokeShape::Outline { non_zero: true }, true, tolerance).map(Rc::new)
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
    /// Stretching the selection across (or up and down) from `anchor`, the middle of the edge
    /// opposite the grabbed `edge` (font units).
    Stretch { anchor: kurbo::Point, edge: kurbo::Point, across: bool },
    /// Turning the selection about `center` (font units).
    Rotate { center: kurbo::Point },
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
    /// Shift held: turns go by 15°.
    snap: bool,
}

fn scale_about(center: kurbo::Point, k: f64) -> Affine {
    about(center, Affine::scale(k))
}

/// A transform `t` made about a point instead of the origin.
fn about(center: kurbo::Point, t: Affine) -> Affine {
    Affine::translate(center.to_vec2()) * t * Affine::translate(-center.to_vec2())
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
    /// Whether the outline's points and handles are shown, and as what curves.
    nodes: Nodes,
    selection: BTreeSet<usize>,
    hovered: Option<usize>,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<Drag>,
    advance: Entity<InputState>,
    /// A transform typed: turn (degrees, counterclockwise), scale across and up (%), slant (degrees).
    turn: Entity<InputState>,
    scale_x: Entity<InputState>,
    scale_y: Entity<InputState>,
    slant: Entity<InputState>,
    /// Written while painting, read by the event handlers.
    fit: Rc<Cell<Option<View>>>,
    _subscriptions: Vec<Subscription>,
}

impl GlyphView {
    pub fn new(document: Entity<Document>, glyph: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weight = cx.new(|_| SliderState::new().max(AXIS.max as f32).min(AXIS.min as f32).step(1.0).default_value(AXIS.default as f32));
        let advance = cx.new(|cx| InputState::new(window, cx));
        let typed = |value: &'static str, window: &mut Window, cx: &mut Context<Self>| cx.new(|cx| InputState::new(window, cx).default_value(value));
        let (turn, scale_x, scale_y, slant) = (typed("0", window, cx), typed("100", window, cx), typed("100", window, cx), typed("0", window, cx));
        let subscriptions = vec![
            cx.subscribe_in(&document, window, move |this, _, event: &DocumentEvent, window, cx| match event {
                DocumentEvent::Glyph(g) if *g == this.glyph => this.sync(window, cx),
                DocumentEvent::GlyphList | DocumentEvent::Saved => this.sync(window, cx),
                // The glyph moved (or went: then there is none to show; the tab closes).
                DocumentEvent::Reindexed(map) => this.glyph = map.get(this.glyph).copied().flatten().unwrap_or(usize::MAX),
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
            nodes: Nodes::Hidden,
            selection: BTreeSet::new(),
            hovered: None,
            zoom: 1.0,
            pan: Point::default(),
            drag: None,
            advance,
            turn,
            scale_x,
            scale_y,
            slant,
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

    /// Where the selection box's handles are in the window: its corners, the middles of its edges
    /// (left, right, bottom, top) and the turning handle above it.
    pub fn handles(&self, cx: &App) -> Option<([Point<Pixels>; 4], [Point<Pixels>; 4], Point<Pixels>)> {
        let (view, glyph) = (self.view()?, self.glyph(cx)?);
        let b = self.selection_bounds(glyph)?;
        let c = b.center();
        let at = |x: f64, y: f64| view.to_window(kurbo::Point::new(x, y));
        let corners = [at(b.x0, b.y0), at(b.x1, b.y0), at(b.x1, b.y1), at(b.x0, b.y1)];
        let edges = [at(b.x0, c.y), at(b.x1, c.y), at(c.x, b.y0), at(c.x, b.y1)];
        Some((corners, edges, at(c.x, b.y1) - point(px(0.0), px(ROTATE_PX))))
    }

    /// How the outlines' points are shown.
    pub fn nodes(&self) -> Nodes {
        self.nodes
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
                            shapes.follow(s);
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
        let jobs: Vec<(Vec<SkeletonPiece>, [f64; 5])> =
            self.shapes.iter().filter(|s| s.plan.is_none() && !s.skeleton.is_empty()).map(|s| (s.skeleton.clone(), s.linear)).collect();
        if jobs.is_empty() {
            return;
        }
        let params = PlanParams { precision: PRECISION * pen, weights: self.planned_for };
        let background = cx.background_executor().clone();
        self.planning = Some(cx.spawn(async move |this, cx| {
            let plans = background
                .spawn(async move {
                    let plans = crate::par_map(&jobs, |(sk, [a, b, c, d, stroke_pen])| {
                        let shape = Affine::new([*a, *b, *c, *d, 0.0, 0.0]);
                        Arc::new(scriptoria_outline::plan(&crate::assign::skeleton_of(sk, shape, *stroke_pen), &params))
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

    /// The strokes a transform acts on: those selected, or all of them.
    fn targets(&self) -> Vec<usize> {
        if self.selection.is_empty() { (0..self.shapes.len()).collect() } else { self.selected() }
    }

    /// Transforms the strokes acted on about the middle of their box, as one step to undo. Strokes
    /// keep their width (only where their skeleton lies changes).
    pub fn transform(&mut self, t: Affine, cx: &mut Context<Self>) {
        let which = self.targets();
        let Some(glyph) = self.glyph(cx) else { return };
        let Some(b) = which.iter().filter_map(|&i| self.stroke_bounds(glyph, i)).reduce(|a, b| a.union(b)) else { return };
        let t = about(b.center(), t);
        self.edit(cx, |g| g.transform_strokes(&which, t));
    }

    /// Applies the transform typed (turn, then slant, then scale, about the middle), then clears it.
    fn apply_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let read = |input: &Entity<InputState>, cx: &App| input.read(cx).value().trim().trim_end_matches(['%', '°']).parse::<f64>().ok().filter(|v| v.is_finite());
        let (turn, sx, sy, slant) = (read(&self.turn, cx), read(&self.scale_x, cx), read(&self.scale_y, cx), read(&self.slant, cx));
        let (Some(turn), Some(sx), Some(sy), Some(slant)) = (turn, sx, sy, slant) else {
            window.push_notification(Notification::warning("变换要填数字：旋转和倾斜是角度，缩放是百分比"), cx);
            return;
        };
        if sx.abs() < 1.0 || sy.abs() < 1.0 || slant.abs() >= 80.0 {
            window.push_notification(Notification::warning("缩放至少 1%，倾斜不超过 80°"), cx);
            return;
        }
        let t = Affine::rotate(turn.to_radians()) * Affine::skew(slant.to_radians().tan(), 0.0) * Affine::scale_non_uniform(sx / 100.0, sy / 100.0);
        self.transform(t, cx);
        for (input, value) in [(&self.turn, "0"), (&self.scale_x, "100"), (&self.scale_y, "100"), (&self.slant, "0")] {
            input.update(cx, |s, cx| s.set_value(value, window, cx));
        }
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
            DragKind::Stretch { anchor, edge, across } => {
                let (e, t) = (edge - anchor, to - anchor);
                let k = if across { t.x / e.x } else { t.y / e.y };
                let k = if k.is_finite() { k.max(0.02) } else { 1.0 };
                let (kx, ky) = if across { (k, 1.0) } else { (1.0, k) };
                Some(about(anchor, Affine::scale_non_uniform(kx, ky)))
            }
            DragKind::Rotate { center } => {
                let mut angle = (to - center).atan2() - (from - center).atan2();
                if d.snap {
                    let step = 15f64.to_radians();
                    angle = (angle / step).round() * step;
                }
                Some(about(center, Affine::rotate(angle)))
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
        self.drag = Some(Drag { button: ev.button, kind, start: ev.position, last: ev.position, moved: false, snap: ev.modifiers.shift });
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
                let c = b.center();
                // The turning handle, above the box's top.
                let handle = view.to_window(kurbo::Point::new(c.x, b.y1)) - point(px(0.0), px(ROTATE_PX));
                let d = handle - pos;
                if f32::from(d.x).hypot(f32::from(d.y)) <= HANDLE_PX + 3.0 {
                    return DragKind::Rotate { center: c };
                }
                let corners = [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)].map(kurbo::Point::from);
                for (k, &corner) in corners.iter().enumerate() {
                    if near(corner, HANDLE_PX + 2.0) {
                        return DragKind::Scale { anchor: corners[(k + 2) % 4], corner };
                    }
                }
                // The edges' middles: left, right, bottom, top.
                let edges = [
                    ((b.x0, c.y), (b.x1, c.y), true),
                    ((b.x1, c.y), (b.x0, c.y), true),
                    ((c.x, b.y0), (c.x, b.y1), false),
                    ((c.x, b.y1), (c.x, b.y0), false),
                ];
                for (edge, anchor, across) in edges {
                    if near(edge.into(), HANDLE_PX + 2.0) {
                        return DragKind::Stretch { anchor: anchor.into(), edge: edge.into(), across };
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
            drag.snap = ev.modifiers.shift;
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
        let p = palette(cx);
        // Selected strokes in the accent (as on the pages), the advance and hovered strokes blue, the
        // writing order red.
        let (ink, muted, line, paper) = (theme.foreground, theme.muted_foreground, theme.border, theme.background);
        let (accent, advance_color, order_color, on_badge) = (p.mauve.hsla(), p.blue.hsla(), p.red.hsla(), p.base.hsla());
        // The reference glyph and the guides, faint.
        let reference_color = muted.opacity(0.18);
        let guide = p.green.opacity(0.45);
        let doc = self.document.read(cx);
        let font = &doc.project.font;
        let (ascender, descender, upm) = (font.ascender as f64, font.descender as f64, font.units_per_em as f64);
        let em = kurbo::Rect::new(0.0, descender, upm, ascender);
        let Some(glyph) = doc.project.glyphs.get(self.glyph) else { return div().into_any_element() };
        let advance = self.drag_advance().unwrap_or(glyph.advance);
        let preview = self.drag_transform();
        let regenerated = self.regenerated();
        let scale = self.scale(cx);
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
                // Turned, stretched or scaled by the drag: drawn from the skeleton at its new shape, so that
                // its width stays the pen's (as it will be).
                let reshaped = regenerated && !is_translation(moved) && !s.skeleton.is_empty();
                let mesh = if reshaped {
                    let placed = moved * s.placement;
                    let linear = Affine::new([placed.as_coeffs()[0], placed.as_coeffs()[1], placed.as_coeffs()[2], placed.as_coeffs()[3], 0.0, 0.0]);
                    capsules(&s.skeleton, linear, s.pen * scale).map(|m| (m, Affine::translate(placed.translation())))
                } else {
                    shapes.mesh(s.placement, regenerated, scale).map(|(mesh, t)| (mesh, moved * t))
                };
                (mesh, start, color)
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
        // The outlines' points (not while strokes are reshaped by a drag).
        let nodes: Vec<Node> = if self.nodes == Nodes::Hidden || !regenerated || preview.is_some_and(|p| !is_translation(p)) {
            Vec::new()
        } else {
            glyph.strokes.iter().zip(&self.shapes).enumerate().flat_map(|(i, (s, shapes))| {
                let moved = match preview {
                    Some(p) if self.selection.contains(&i) => p,
                    _ => Affine::IDENTITY,
                };
                shapes.regenerated.as_ref().map(|(_, path, _)| nodes_of(&(Affine::translate((moved * s.placement).translation()) * path.clone()), self.nodes)).unwrap_or_default()
            }).collect()
        };
        let (on_color, off_color, handle_color) = (p.blue.hsla(), p.peach.hsla(), p.overlay1.hsla());
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
                        window.paint_quad(quad(dot, px(3.0), order_color, px(1.0), on_badge, Default::default()));
                        let badge = Bounds::new(c - point(px(22.0), px(22.0)), size(px(16.0), px(16.0)));
                        window.paint_quad(quad(badge, px(8.0), order_color, px(0.0), gpui::transparent_black(), Default::default()));
                        label_centered(window, cx, &(i + 1).to_string(), badge.center(), on_badge, px(10.0));
                    }
                    // Points and handles: handles first, then off-curve points (round), then on-curve ones (square).
                    for node in &nodes {
                        if let Node::Handle(a, b) = node {
                            let mut builder = PathBuilder::stroke(px(1.0));
                            builder.move_to(view.to_window(*a));
                            builder.line_to(view.to_window(*b));
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, handle_color);
                            }
                        }
                    }
                    for node in &nodes {
                        let (p, half, round, fill, edge) = match *node {
                            Node::On(p) => (p, 3.0, 0.0, on_color, on_color),
                            Node::Off(p) => (p, 3.0, 3.0, paper, off_color),
                            Node::Implied(p) => (p, 1.5, 1.5, handle_color, handle_color),
                            Node::Handle(..) => continue,
                        };
                        let c = view.to_window(p);
                        let b = Bounds::new(c - point(px(half), px(half)), size(px(2.0 * half), px(2.0 * half)));
                        window.paint_quad(quad(b, px(round), fill, px(1.0), edge, Default::default()));
                    }
                    if let Some(b) = selection_box {
                        let r = view.rect(b);
                        window.paint_quad(quad(r, px(0.0), gpui::transparent_black(), px(1.0), accent, Default::default()));
                        // Corners scale, the edges' middles stretch (smaller), the round handle above turns.
                        let handle = |window: &mut Window, c: Point<Pixels>, half: f32, round: f32| {
                            let h = Bounds::new(c - point(px(half), px(half)), size(px(2.0 * half), px(2.0 * half)));
                            window.paint_quad(quad(h, px(round), paper, px(1.0), accent, Default::default()));
                        };
                        for c in [r.origin, r.top_right(), r.bottom_right(), r.bottom_left()] {
                            handle(window, c, HANDLE_PX, 1.0);
                        }
                        let m = r.center();
                        for c in [point(r.left(), m.y), point(r.right(), m.y), point(m.x, r.top()), point(m.x, r.bottom())] {
                            handle(window, c, HANDLE_PX - 1.0, 0.0);
                        }
                        let turn = point(m.x, r.top() - px(ROTATE_PX));
                        let stem = Bounds::new(point(m.x - px(0.5), turn.y), size(px(1.0), px(ROTATE_PX)));
                        window.paint_quad(quad(stem, px(0.0), accent, px(0.0), gpui::transparent_black(), Default::default()));
                        handle(window, turn, HANDLE_PX + 1.0, HANDLE_PX + 1.0);
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
        let (accent, muted, border) = (palette(cx).mauve.hsla(), theme.muted_foreground, theme.border);
        let rows: Vec<_> = self
            .glyph(cx)
            .map(|g| g.strokes.as_slice())
            .unwrap_or_default()
            .iter()
            .enumerate()
            .map(|(i, s)| {
                // Where the pen came down (font units).
                let start = s.skeleton.first().and_then(|p| p.circles.first()).map(|&(c, _)| s.placement * c);
                let from = start.map_or_else(|| "（没有骨架）".to_string(), |p| format!("落笔 ({:.0}, {:.0})", p.x, p.y));
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
            .child(self.render_transforms(cx))
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

    /// A button showing the outlines' points as `nodes` (again: hiding them).
    fn nodes_button(&self, id: &'static str, label: &'static str, tip: &'static str, nodes: Nodes, cx: &mut Context<Self>) -> Button {
        Button::new(id).small().label(label).tooltip(tip).selected(self.nodes == nodes).on_click(cx.listener(move |this, _, _, cx| {
            this.nodes = if this.nodes == nodes { Nodes::Hidden } else { nodes };
            cx.notify();
        }))
    }

    /// Turning, flipping, stretching and slanting the selected strokes (or all of them), about
    /// their middle; their widths stay.
    fn render_transforms(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let what = if self.selection.is_empty() { "整个字" } else { "选中的笔画" };
        let field = |label: &'static str, input: &Entity<InputState>, id: &'static str, unit: &'static str| {
            h_flex()
                .gap_1()
                .text_xs()
                .child(div().w(px(28.0)).child(label))
                .child(div().w(px(56.0)).child(Input::new(input).xsmall().id(id)))
                .child(div().text_color(muted).child(unit))
        };
        let quick = |id: &'static str, label: &'static str, tip: &'static str, t: Affine| {
            Button::new(id).xsmall().label(label).tooltip(tip).on_click(cx.listener(move |this, _, _, cx| this.transform(t, cx)))
        };
        let quarter = std::f64::consts::FRAC_PI_2;
        v_flex()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(border)
            .child(h_flex().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child("变换").child(div().ml_2().text_xs().text_color(muted).child(what)))
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(quick("turn-left", "↺ 90°", "逆时针转 90°", Affine::rotate(quarter)))
                    .child(quick("turn-right", "↻ 90°", "顺时针转 90°", Affine::rotate(-quarter)))
                    .child(quick("flip-across", "⇋ 翻转", "左右翻转", Affine::scale_non_uniform(-1.0, 1.0)))
                    .child(quick("flip-up", "⇅ 翻转", "上下翻转", Affine::scale_non_uniform(1.0, -1.0))),
            )
            .child(field("旋转", &self.turn, "turn", "°（逆时针）"))
            .child(field("横向", &self.scale_x, "scale-x", "%"))
            .child(field("纵向", &self.scale_y, "scale-y", "%"))
            .child(field("倾斜", &self.slant, "slant", "°"))
            .child(
                Button::new("apply-transform")
                    .xsmall()
                    .label("应用")
                    .tooltip("先旋转，再倾斜，再缩放，都绕中心；笔画粗细不变")
                    .on_click(cx.listener(|this, _, window, cx| this.apply_typed(window, cx))),
            )
    }

    fn status_text(&self, cx: &App) -> String {
        let hints = "点选或框选笔画（Ctrl/Shift 加选，Ctrl+A 全选）· 拖动移动 · 选框四角等比缩放，边中点单向拉伸，上方圆点旋转（Shift 按 15°）· 方向键微调（Shift 加大）· Delete 删除 · Alt+↑/↓ 调整笔顺 · 拖蓝线改字宽 · 滚轮或中键拖动平移，Ctrl+滚轮缩放";
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
                let bearings = g.ink_bounds().map(|b| (b.x0, g.advance - b.x1));
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
            .child(Button::new("center").small().label("左右居中").on_click(cx.listener(|this, _, _, cx| this.edit(cx, Glyph::center))))
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
            .child(self.nodes_button("nodes-cubic", "三次节点", "显示由骨架生成的三次曲线的节点和把手", Nodes::Cubic, cx))
            .child(self.nodes_button("nodes-quadratic", "二次节点", "显示导出到字体里的二次曲线的点（空心是曲线外的控制点，小点是省略的曲线上的点）", Nodes::Quadratic, cx))
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
            // Two lines high whatever it says, so that the canvas (and the glyph in it) stays put.
            .child(div().flex_shrink_0().h(px(36.0)).px_3().pb_1().overflow_hidden().text_xs().text_color(muted).child(self.status_text(cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_are_drawn_from_their_skeletons_meanwhile() {
        // Two circles of radius 0.1 and 0.2 a unit apart, placed twice as large, with a pen of 10.
        let piece = SkeletonPiece {
            circles: vec![(kurbo::Point::new(0.0, 0.0), 0.1), (kurbo::Point::new(1.0, 0.0), 0.2)],
            spine: vec![kurbo::CubicBez::new((0.0, 0.0), (0.3, 0.0), (0.7, 0.0), (1.0, 0.0))],
        };
        let mesh = capsules(&[piece], Affine::scale(2.0), 10.0).expect("ink");
        assert!(mesh.triangle_count() > 0);
        // A circle inside another adds no band; an empty skeleton draws nothing.
        let inside = SkeletonPiece { circles: vec![(kurbo::Point::ZERO, 1.0), (kurbo::Point::new(0.1, 0.0), 0.5)], spine: Vec::new() };
        assert!(capsules(&[inside], Affine::IDENTITY, 1.0).is_some());
        assert!(capsules(&[], Affine::IDENTITY, 1.0).is_none());
    }

    #[test]
    fn the_fonts_points_are_marked() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.curve_to((0.0, 300.0), (300.0, 300.0), (300.0, 0.0));
        path.close_path();
        let cubic = nodes_of(&path, Nodes::Cubic);
        assert_eq!(cubic.iter().filter(|n| matches!(n, Node::Off(_))).count(), 2);
        assert_eq!(cubic.iter().filter(|n| matches!(n, Node::Handle(..))).count(), 2);
        let quadratic = nodes_of(&path, Nodes::Quadratic);
        assert!(quadratic.iter().any(|n| matches!(n, Node::Off(_))) && quadratic.iter().any(|n| matches!(n, Node::On(_))));
        assert!(nodes_of(&path, Nodes::Hidden).is_empty());
    }
}
