//! The glyph grid, as in FontForge's main window: every glyph of the project in a cell, drawn
//! from its strokes as written, with its character. Characters typed or pasted into the box above
//! become new glyphs; a click selects a glyph, a double click opens it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::{ActiveTheme as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{
    Bounds, ClickEvent, Context, Entity, EventEmitter, Pixels, SharedString, Subscription, Window, canvas, div, point,
    px, uniform_list,
};
use scriptoria_ink::StrokeShape;
use scriptoria_ink::kurbo::Affine;
use scriptoria_project::{Glyph, OutlineKind};

use crate::app::document::{Document, DocumentEvent};
use crate::app::mesh::Mesh;

/// A cell's drawing area (px, square), the margin inside it, and the label under it.
const CELL: f32 = 76.0;
const MARGIN: f32 = 6.0;
const LABEL: f32 = 20.0;
/// Tessellation tolerance (font units): far below a pixel at cell size.
const TOLERANCE: f32 = 2.0;

pub enum FontViewEvent {
    /// The selected glyph changed.
    Selected(Option<usize>),
    /// Open the glyph (double click).
    Open(usize),
}

impl EventEmitter<FontViewEvent> for FontView {}

pub struct FontView {
    document: Entity<Document>,
    input: Entity<InputState>,
    selected: Option<usize>,
    /// The grid's width at the last paint, for the number of columns.
    width: Rc<Cell<Pixels>>,
    /// Per glyph: its meshes (font units, y down from the ascender) at a revision.
    meshes: Rc<RefCell<HashMap<usize, (u64, Rc<Vec<Mesh>>)>>>,
    _subscriptions: Vec<Subscription>,
}

impl FontView {
    pub fn new(document: Entity<Document>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入或粘贴要做的字，回车加入"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_typed(input.clone(), window, cx);
                }
            }),
            cx.subscribe(&document, |this, document, event: &DocumentEvent, cx| {
                if matches!(event, DocumentEvent::GlyphList | DocumentEvent::Glyph(_) | DocumentEvent::Saved) {
                    // A replaced project may have fewer glyphs; its meshes are all new.
                    let n = document.read(cx).project.glyphs.len();
                    if this.selected.is_some_and(|s| s >= n) {
                        this.select(None, cx);
                    }
                    if matches!(event, DocumentEvent::Saved) && document.read(cx).folder.is_some() {
                        this.meshes.borrow_mut().retain(|&i, _| i < n);
                    }
                    cx.notify();
                }
            }),
        ];
        FontView { document, input, selected: None, width: Rc::new(Cell::new(px(600.0))), meshes: Rc::default(), _subscriptions: subscriptions }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn select(&mut self, glyph: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != glyph {
            self.selected = glyph;
            cx.emit(FontViewEvent::Selected(glyph));
            cx.notify();
        }
    }

    /// Selects the first glyph without strokes after the selected one (wrapping around), if any.
    pub fn select_next_empty(&mut self, cx: &mut Context<Self>) {
        let glyphs = &self.document.read(cx).project.glyphs;
        let n = glyphs.len();
        let from = self.selected.map_or(0, |s| s + 1);
        let next = (0..n).map(|k| (from + k) % n).find(|&i| glyphs[i].strokes.is_empty());
        if next.is_some() {
            self.select(next, cx);
        }
    }

    fn add_typed(&mut self, input: Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let text = input.read(cx).value().to_string();
        let first_new = self.document.read(cx).project.glyphs.len();
        let added = self.document.update(cx, |d, cx| d.add_chars(&text, cx));
        if added > 0 && self.selected.is_none() {
            self.select(Some(first_new), cx);
        }
        input.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    fn columns(&self) -> usize {
        ((f32::from(self.width.get()) / (CELL + 8.0)).floor() as usize).max(1)
    }

    /// The glyph's meshes, made again when it has changed.
    fn meshes_for(&self, index: usize, glyph: &Glyph, revision: u64, ascender: f64) -> Rc<Vec<Mesh>> {
        if let Some((rev, meshes)) = self.meshes.borrow().get(&index)
            && *rev == revision
        {
            return meshes.clone();
        }
        let meshes = Rc::new(glyph_meshes(glyph, ascender));
        self.meshes.borrow_mut().insert(index, (revision, meshes.clone()));
        meshes
    }

    fn render_cell(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let doc = self.document.read(cx);
        let font = &doc.project.font;
        let glyph = &doc.project.glyphs[index];
        let meshes = self.meshes_for(index, glyph, doc.revision(index), font.ascender as f64);
        let em = (font.ascender - font.descender) as f32;
        let advance = glyph.advance as f32;
        let label: SharedString = glyph.chars.iter().collect::<String>().into();
        let empty = glyph.strokes.is_empty();
        let selected = self.selected == Some(index);
        let theme = cx.theme();
        let (ink, border, accent, muted, background) = (theme.foreground, theme.border, theme.primary, theme.muted_foreground, theme.background);
        v_flex()
            .id(("cell", index))
            .w(px(CELL))
            .border_1()
            .rounded_sm()
            .border_color(if selected { accent } else { border })
            .bg(background)
            .cursor_pointer()
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window, _| {
                        // The em (ascender to descender) fills the area inside the margin, the advance centered.
                        let scale = (f32::from(bounds.size.height) - 2.0 * MARGIN) / em;
                        let x = f32::from(bounds.origin.x) + (f32::from(bounds.size.width) - advance * scale) / 2.0;
                        let origin = point(px(x), bounds.origin.y + px(MARGIN));
                        for mesh in meshes.iter() {
                            window.paint_path(mesh.to_path(origin, scale), ink);
                        }
                    },
                )
                .w(px(CELL))
                .h(px(CELL)),
            )
            .child(
                div()
                    .h(px(LABEL))
                    .border_t_1()
                    .border_color(border)
                    .text_xs()
                    .text_center()
                    .text_color(if empty { muted } else { ink })
                    .when(selected, |d| d.bg(accent.opacity(0.15)))
                    .child(label),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.select(Some(index), cx);
                if event.click_count() >= 2 {
                    cx.emit(FontViewEvent::Open(index));
                }
            }))
            .test_support()
    }
}

/// A glyph's strokes as written, placed in the em, y down from the ascender (font units).
pub fn glyph_meshes(glyph: &Glyph, ascender: f64) -> Vec<Mesh> {
    let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, ascender]);
    glyph
        .strokes
        .iter()
        .filter_map(|s| {
            let t = flip * s.placement;
            let path = t * s.outline.path.clone();
            let shape = match s.outline.kind {
                OutlineKind::Filled => StrokeShape::Outline { non_zero: true },
                OutlineKind::Line { width } => StrokeShape::Line { width: width * t.determinant().abs().sqrt() },
            };
            Mesh::from_path_with_tolerance(&path, shape, true, TOLERANCE)
        })
        .collect()
}

impl Render for FontView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.document.read(cx).project.glyphs.len();
        let columns = self.columns();
        let rows = count.div_ceil(columns);
        let width = self.width.clone();
        let input = self.input.clone();
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.secondary)
            .child(
                h_flex()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().flex_1().child(Input::new(&self.input).id("chars")))
                    .child(Button::new("add-chars").primary().label("加入").on_click(cx.listener(move |this, _, window, cx| {
                        this.add_typed(input.clone(), window, cx);
                    }))),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{count} 个字形 · 单击选中，双击打开")),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        canvas(
                            move |bounds, window, _| {
                                if (f32::from(bounds.size.width) - f32::from(width.get())).abs() > 1.0 {
                                    width.set(bounds.size.width);
                                    window.refresh();
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(
                        uniform_list(
                            "glyph-rows",
                            rows,
                            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .map(|row| {
                                        let cells = (row * columns..((row + 1) * columns).min(count)).map(|i| this.render_cell(i, cx)).collect::<Vec<_>>();
                                        h_flex().gap_2().px_2().py_1().children(cells)
                                    })
                                    .collect()
                            }),
                        )
                        .size_full(),
                    ),
            )
    }
}
