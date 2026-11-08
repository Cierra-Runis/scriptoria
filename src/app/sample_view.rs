//! Sample text, as FontForge's metrics window: characters set one after another with the project's
//! glyphs as they are placed and with their advances, at any weight and size, to judge whether
//! they balance. Characters without strokes yet show as empty boxes; a click on a glyph opens it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::TestSupportExt as _;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::{ActiveTheme as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{
    Bounds, ContentMask, Context, Entity, EventEmitter, MouseButton, MouseDownEvent, Pixels, Point, Subscription, Window, canvas,
    div, point, px, quad, size,
};
use scriptoria_ink::kurbo::Affine;

use crate::app::document::Document;
use crate::app::mesh::Mesh;
use crate::app::outlines::Outlines;
use crate::font::AXIS;

/// Space around the text (px).
const PADDING: f32 = 16.0;
/// Lines are this many ems apart.
const LINE: f32 = 1.15;

pub enum SampleEvent {
    /// Open this glyph.
    Open(usize),
}

impl EventEmitter<SampleEvent> for SampleView {}

/// One character of the text, ready to set.
enum Item {
    Newline,
    /// A glyph: its index, advance (font units) and outline (font units, y up; none while it is
    /// planned or if it has no strokes).
    Glyph { glyph: usize, advance: f64, mesh: Option<Rc<Mesh>> },
    /// A character the project has no strokes for.
    Missing,
}

pub struct SampleView {
    document: Entity<Document>,
    outlines: Entity<Outlines>,
    text: Entity<InputState>,
    weight: Entity<SliderState>,
    /// Pixels per em.
    size: Entity<SliderState>,
    /// Where each glyph was drawn at the last paint, for clicks.
    boxes: Rc<RefCell<Vec<(Bounds<Pixels>, usize)>>>,
    _subscriptions: Vec<Subscription>,
}

impl SampleView {
    pub fn new(document: Entity<Document>, outlines: Entity<Outlines>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // To start with, the characters written so far.
        let written: String = document.read(cx).project.glyphs.iter().filter(|g| !g.strokes.is_empty()).flat_map(|g| g.chars.first()).take(40).collect();
        let text = cx.new(|cx| InputState::new(window, cx).placeholder("输入要排出来看的文字").default_value(written));
        let weight = cx.new(|_| SliderState::new().max(AXIS.max as f32).min(AXIS.min as f32).step(1.0).default_value(AXIS.default as f32));
        let size = cx.new(|_| SliderState::new().max(240.0).min(24.0).step(1.0).default_value(72.0));
        let subscriptions = vec![
            cx.subscribe(&text, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&weight, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.subscribe(&size, |_, _, _: &SliderEvent, cx| cx.notify()),
            cx.observe(&document, |_, _, cx| cx.notify()),
            cx.observe(&outlines, |_, _, cx| cx.notify()),
        ];
        SampleView { document, outlines, text, weight, size, boxes: Rc::default(), _subscriptions: subscriptions }
    }

    /// The glyph drawn at a window point, at the last paint.
    pub fn glyph_at(&self, p: Point<Pixels>) -> Option<usize> {
        self.boxes.borrow().iter().find(|(b, _)| b.contains(&p)).map(|&(_, g)| g)
    }

    /// Where the glyphs were drawn at the last paint: (box, glyph), in the order set.
    pub fn glyph_boxes(&self) -> Vec<(Bounds<Pixels>, usize)> {
        self.boxes.borrow().clone()
    }

    fn items(&mut self, cx: &mut Context<Self>) -> Vec<Item> {
        let text = self.text.read(cx).value().to_string();
        let wght = self.weight.read(cx).value().start() as f64;
        text.chars()
            .filter(|c| *c == '\n' || !c.is_control())
            .map(|c| {
                if c == '\n' {
                    return Item::Newline;
                }
                let doc = self.document.read(cx);
                match doc.project.glyph_for(c).filter(|&g| !doc.project.glyphs[g].strokes.is_empty()) {
                    Some(glyph) => {
                        let advance = doc.project.glyphs[glyph].advance;
                        let mesh = self.outlines.update(cx, |o, cx| o.mesh(glyph, wght, cx));
                        Item::Glyph { glyph, advance, mesh }
                    }
                    None => Item::Missing,
                }
            })
            .collect()
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(glyph) = self.glyph_at(ev.position) {
            cx.emit(SampleEvent::Open(glyph));
        }
    }
}

impl Render for SampleView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.items(cx);
        let font = self.document.read(cx).project.font.clone();
        let (upm, ascender, descender) = (font.units_per_em as f32, font.ascender as f32, font.descender as f32);
        let wght = self.weight.read(cx).value().start();
        let px_per_em = self.size.read(cx).value().start();
        let boxes = self.boxes.clone();
        let theme = cx.theme();
        let (ink, muted, border, paper) = (theme.foreground, theme.muted_foreground, theme.border, theme.background);

        let toolbar = h_flex()
            .gap_3()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(border)
            .child(div().flex_1().child(Input::new(&self.text).id("sample-text")))
            .child(div().text_sm().child("字重"))
            .child(div().flex_shrink_0().w(px(140.0)).child(Slider::new(&self.weight)))
            .child(div().flex_shrink_0().w(px(70.0)).text_xs().child(format!("wght {wght:.0}")))
            .child(div().text_sm().child("字号"))
            .child(div().flex_shrink_0().w(px(120.0)).child(Slider::new(&self.size)))
            .child(div().flex_shrink_0().w(px(50.0)).text_xs().child(format!("{px_per_em:.0} px")));

        let page = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    window.paint_quad(quad(bounds, px(0.0), paper, px(0.0), gpui::transparent_black(), Default::default()));
                    let s = px_per_em / upm;
                    let left = f32::from(bounds.origin.x) + PADDING;
                    let right = f32::from(bounds.origin.x + bounds.size.width) - PADDING;
                    let line = (ascender - descender) * s * LINE;
                    let mut baseline = f32::from(bounds.origin.y) + PADDING + ascender * s;
                    let mut x = left;
                    let mut placed = Vec::new();
                    let guide = |window: &mut Window, baseline: f32| {
                        let b = Bounds::new(point(px(left), px(baseline)), size(px(right - left), px(1.0)));
                        window.paint_quad(quad(b, px(0.0), muted.opacity(0.25), px(0.0), gpui::transparent_black(), Default::default()));
                    };
                    guide(window, baseline);
                    for item in &items {
                        let advance = match item {
                            Item::Newline => None,
                            Item::Glyph { advance, .. } => Some(*advance as f32 * s),
                            Item::Missing => Some(upm * 0.5 * s),
                        };
                        let Some(advance) = advance.filter(|a| x + a <= right || x == left) else {
                            x = left;
                            baseline += line;
                            guide(window, baseline);
                            if matches!(item, Item::Newline) {
                                continue;
                            }
                            // The item that did not fit starts the new line.
                            let advance = match item {
                                Item::Glyph { advance, .. } => *advance as f32 * s,
                                _ => upm * 0.5 * s,
                            };
                            draw(window, item, x, baseline, s, advance, ascender, descender, ink, muted, &mut placed);
                            x += advance;
                            continue;
                        };
                        draw(window, item, x, baseline, s, advance, ascender, descender, ink, muted, &mut placed);
                        x += advance;
                    }
                    *boxes.borrow_mut() = placed;
                });
            },
        )
        .size_full();

        v_flex()
            .size_full()
            .bg(paper)
            .child(toolbar)
            .child(
                div()
                    .id("sample-canvas")
                    .flex_1()
                    .min_h_0()
                    .child(page)
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .test_support(),
            )
            .child(div().px_3().py_1().border_t_1().border_color(border).text_xs().text_color(muted).child(
                "按字的摆放和字宽连排；灰框是还没有笔画的字 · 点一个字打开它",
            ))
    }
}

/// Draws one item at pen position `x` on `baseline` (window px), `s` px per font unit, and notes
/// where a glyph went.
#[allow(clippy::too_many_arguments)]
fn draw(
    window: &mut Window,
    item: &Item,
    x: f32,
    baseline: f32,
    s: f32,
    advance: f32,
    ascender: f32,
    descender: f32,
    ink: gpui::Hsla,
    muted: gpui::Hsla,
    placed: &mut Vec<(Bounds<Pixels>, usize)>,
) {
    let cell = Bounds::new(point(px(x), px(baseline - ascender * s)), size(px(advance), px((ascender - descender) * s)));
    match item {
        Item::Glyph { glyph, mesh, .. } => {
            if let Some(mesh) = mesh {
                let t = Affine::new([s as f64, 0.0, 0.0, -s as f64, x as f64, baseline as f64]);
                window.paint_path(mesh.to_path_affine(t), ink);
            }
            placed.push((cell, *glyph));
        }
        Item::Missing => {
            let b = cell.dilate(px(-2.0));
            window.paint_quad(quad(b, px(2.0), gpui::transparent_black(), px(1.0), muted.opacity(0.4), Default::default()));
        }
        Item::Newline => {}
    }
}
