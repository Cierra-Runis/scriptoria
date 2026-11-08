//! The glyph grid, as in FontForge's main window: the project's glyphs in cells, drawn as the font
//! draws them at the default weight (from their strokes as written until that is ready), with
//! their characters. A list beside it sorts the glyphs into groups (all; not written yet, written;
//! by Unicode block; by practice sheet; by the problems they have, see [`crate::problems`]) and the grid shows one group, in the project's order or by
//! code point. Characters typed or pasted into the box above become new glyphs (`U+4E00-U+4E2F`
//! adds a range); a block is filled in with the characters the reference font has. A click
//! selects a glyph (Ctrl adds or removes, Shift selects a range), a double click opens it.
//! Selected glyphs are placed after the reference font, made full width, centered, given a
//! practice sheet or removed all at once; a sheet is written out again or forgotten. Above the grid:
//! the weight the handwriting is, and how large the hand writes (points per em, typed or measured
//! against the reference font).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::notification::Notification;
use gpui::component::{ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{
    App, Bounds, ClickEvent, Context, Entity, EventEmitter, Pixels, SharedString, Subscription, Window, canvas, div,
    px, uniform_list,
};
use scriptoria_ink::StrokeShape;
use scriptoria_ink::kurbo::Affine;
use scriptoria_project::{Glyph, OutlineKind};

use crate::app::document::{Document, DocumentEvent};
use crate::app::mesh::Mesh;
use crate::app::outlines::Outlines;
use crate::font::{AXIS, WRITTEN_MIN, Weights};
use crate::groups::{self, Group, Listed, Section};

/// A cell's drawing area (px, square), the margin inside it, and the label under it.
const CELL: f32 = 76.0;
const MARGIN: f32 = 6.0;
const LABEL: f32 = 20.0;
/// The width of the list of groups (px).
const GROUPS: f32 = 172.0;
/// Tessellation tolerance (font units): far below a pixel at cell size.
const TOLERANCE: f32 = 2.0;
/// What a full-width glyph's larger side becomes, as a fraction of the em.
const FACE: f64 = 0.9;
/// How far 统一笔宽 draws strokes' widths towards the typical one.
const EVEN_WIDTHS: f64 = 0.7;
/// How long a stroke must be (a share of the em) to count as a horizontal stroke for 横画取平.
const HORIZONTAL_LENGTH: f64 = 0.2;

pub enum FontViewEvent {
    /// The selected glyph changed.
    Selected(Option<usize>),
    /// Open the glyph (double click).
    Open(usize),
    /// Show the sample text.
    OpenSample,
    /// Make a practice sheet for these glyphs.
    MakeTemplate(Vec<usize>),
    /// Write a practice sheet (by id) out again.
    ExportTemplate(u32),
}

impl EventEmitter<FontViewEvent> for FontView {}

/// The groups and the glyphs shown, as they were for a state of the document.
struct Shown {
    /// The document (its id, its generation), the group and the order they were made for.
    key: (u64, u64, Group, bool),
    listed: Vec<Listed>,
    glyphs: Vec<usize>,
    /// For a block: how many characters the reference font has that the project does not.
    missing: usize,
}

pub struct FontView {
    document: Entity<Document>,
    outlines: Entity<Outlines>,
    input: Entity<InputState>,
    /// The glyph strokes go to, and where a Shift range starts.
    selected: Option<usize>,
    /// All glyphs selected (with `selected`).
    selection: BTreeSet<usize>,
    /// The group shown, and whether by code point.
    group: Group,
    by_code: bool,
    shown: Option<Shown>,
    /// The grid's width at the last paint, for the number of columns.
    width: Rc<Cell<Pixels>>,
    /// Per glyph: its strokes as written (font units, y down from the ascender) at a revision.
    meshes: Rc<RefCell<HashMap<usize, (u64, Rc<Vec<Rc<Mesh>>>)>>>,
    /// The weight the strokes as written are (`wght`), set when the slider is let go.
    written: Entity<SliderState>,
    /// How large the hand writes (points per em), set on Enter.
    scale: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl FontView {
    pub fn new(document: Entity<Document>, outlines: Entity<Outlines>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("输入要做的字或 U+4E00-U+4E2F，回车加入"));
        let written_weight = document.read(cx).project.font.written_weight;
        let written = cx.new(|_| SliderState::new().max(AXIS.max as f32).min(WRITTEN_MIN as f32).step(10.0).default_value(written_weight as f32));
        let pt_per_em = document.read(cx).project.font.pt_per_em;
        let scale = cx.new(|cx| InputState::new(window, cx).default_value(format_scale(pt_per_em)));
        let subscriptions = vec![
            cx.subscribe(&written, |this, _, event: &SliderEvent, cx| match event {
                SliderEvent::Change(_) => cx.notify(),
                SliderEvent::Release(v) => {
                    let w = v.start() as f64;
                    this.document.update(cx, |d, cx| d.update_font(|f| f.written_weight = w, cx));
                }
            }),
            // The slider follows the project (undo, another project opened).
            cx.subscribe_in(&document, window, |this, document, event: &DocumentEvent, window, cx| {
                if matches!(event, DocumentEvent::Font | DocumentEvent::Saved) {
                    let w = document.read(cx).project.font.written_weight as f32;
                    this.written.update(cx, |s, cx| s.set_value(w, window, cx));
                    let pt = format_scale(document.read(cx).project.font.pt_per_em);
                    this.scale.update(cx, |s, cx| s.set_value(pt, window, cx));
                }
            }),
            cx.subscribe_in(&scale, window, |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let text = input.read(cx).value().to_string();
                    this.set_scale(&text, window, cx);
                }
            }),
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add_typed(input.clone(), window, cx);
                }
            }),
            cx.subscribe(&document, |this, document, event: &DocumentEvent, cx| {
                match event {
                    // Glyphs added or removed: the selection follows them.
                    DocumentEvent::Reindexed(map) => {
                        let moved = |g: usize| map.get(g).copied().flatten();
                        this.selection = this.selection.iter().filter_map(|&g| moved(g)).collect();
                        this.selected = this.selected.and_then(moved);
                    }
                    DocumentEvent::GlyphList | DocumentEvent::Glyph(_) | DocumentEvent::Saved => {
                        // A replaced project may have fewer glyphs.
                        let n = document.read(cx).project.glyphs.len();
                        if this.selected.is_some_and(|s| s >= n) {
                            this.select(None, cx);
                        }
                        this.selection.retain(|&i| i < n);
                        if matches!(event, DocumentEvent::Saved) {
                            this.group = Group::All;
                        }
                    }
                    DocumentEvent::Font | DocumentEvent::Parts => {}
                }
                cx.notify();
            }),
            cx.observe(&outlines, |_, _, cx| cx.notify()),
        ];
        FontView {
            document,
            outlines,
            input,
            selected: None,
            selection: BTreeSet::new(),
            group: Group::All,
            by_code: false,
            shown: None,
            width: Rc::new(Cell::new(px(600.0))),
            meshes: Rc::default(),
            written,
            scale,
            _subscriptions: subscriptions,
        }
    }

    /// The font's weights as the slider has them (also while it is dragged).
    fn shown_weights(&self, cx: &App) -> Weights {
        let thinnest = self.document.read(cx).project.font.thinnest;
        Weights { thinnest, written: self.written.read(cx).value().start() as f64 }
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// All selected glyphs, in font order.
    pub fn selection(&self) -> Vec<usize> {
        self.selection.iter().copied().collect()
    }

    /// The group shown.
    pub fn group(&self) -> Group {
        self.group
    }

    /// Shows a group of glyphs (in code point order, or the project's). The selection keeps only
    /// the glyphs shown.
    pub fn show(&mut self, group: Group, by_code: bool, cx: &mut Context<Self>) {
        self.group = group;
        self.by_code = by_code;
        let shown: BTreeSet<usize> = self.glyphs(cx).iter().copied().collect();
        self.selection.retain(|g| shown.contains(g));
        if self.selected.is_some_and(|s| !shown.contains(&s)) {
            self.set_selected(None, cx);
        }
        cx.notify();
    }

    /// The groups and the glyphs shown, made again when the document has changed.
    fn refresh(&mut self, cx: &App) -> &Shown {
        let doc = self.document.read(cx);
        let key = (doc.id(), doc.generation(), self.group, self.by_code);
        if self.shown.as_ref().is_none_or(|s| s.key != key) {
            let project = &doc.project;
            let problems = doc.problems();
            let listed = groups::list(project, &problems);
            // A group gone (its sheet forgotten, its last glyph removed): all of them.
            if !listed.iter().any(|l| l.group == self.group) {
                self.group = Group::All;
            }
            let mut glyphs = groups::members(project, self.group, &problems);
            if self.by_code {
                glyphs.sort_by_key(|&g| project.glyphs[g].chars.first().map_or(u32::MAX, |&c| c as u32));
            }
            let missing = match (self.group, doc.reference()) {
                (Group::Block(start), Some(r)) => groups::missing_in_block(project, start, |c| r.has(c)).chars().count(),
                _ => 0,
            };
            self.shown = Some(Shown { key: (key.0, key.1, self.group, self.by_code), listed, glyphs, missing });
        }
        self.shown.as_ref().expect("just made")
    }

    /// The glyphs shown, in the order shown.
    pub fn glyphs(&mut self, cx: &App) -> Vec<usize> {
        self.refresh(cx).glyphs.clone()
    }

    /// The groups listed.
    pub fn groups(&mut self, cx: &App) -> Vec<Listed> {
        self.refresh(cx).listed.clone()
    }

    /// Selects one glyph (or none).
    pub fn select(&mut self, glyph: Option<usize>, cx: &mut Context<Self>) {
        self.selection = glyph.into_iter().collect();
        self.set_selected(glyph, cx);
        cx.notify();
    }

    fn set_selected(&mut self, glyph: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != glyph {
            self.selected = glyph;
            cx.emit(FontViewEvent::Selected(glyph));
        }
    }

    /// A click on a glyph: selects it alone; with Ctrl adds or removes it; with Shift adds the
    /// glyphs shown from the last one clicked.
    fn click(&mut self, glyph: usize, ctrl: bool, shift: bool, cx: &mut Context<Self>) {
        match (ctrl, shift, self.selected) {
            (_, true, Some(from)) => {
                let shown = self.glyphs(cx);
                let at = |g: usize| shown.iter().position(|&s| s == g);
                if let (Some(a), Some(b)) = (at(from), at(glyph)) {
                    self.selection.extend(&shown[a.min(b)..=a.max(b)]);
                }
                self.set_selected(Some(glyph), cx);
            }
            (true, _, _) => {
                if !self.selection.remove(&glyph) {
                    self.selection.insert(glyph);
                }
                self.set_selected(Some(glyph), cx);
            }
            _ => self.select(Some(glyph), cx),
        }
        cx.notify();
    }

    /// Selects every glyph shown.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = self.glyphs(cx).into_iter().collect();
        cx.notify();
    }

    /// Changes every selected glyph, as one step to undo.
    fn edit_selection(&mut self, cx: &mut Context<Self>, f: impl FnMut(&mut Glyph)) {
        let glyphs = self.selection();
        self.document.update(cx, |d, cx| d.update_glyphs(&glyphs, f, cx));
    }

    /// Draws the selected glyphs' strokes towards the font's typical width, as one step to undo.
    fn even_widths(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.document.read(cx).project.typical_width() else { return };
        self.edit_selection(cx, move |g| g.even_widths(target, EVEN_WIDTHS));
    }

    /// Shears the selected glyphs so that their horizontal strokes slope as the font's typically do,
    /// as one step to undo.
    fn level_horizontals(&mut self, cx: &mut Context<Self>) {
        let font = &self.document.read(cx).project.font;
        let min_length = HORIZONTAL_LENGTH * (font.ascender - font.descender) as f64;
        let Some(target) = self.document.read(cx).project.typical_slope(min_length) else { return };
        self.edit_selection(cx, move |g| {
            g.level_to(target, min_length);
        });
    }

    /// Removes the selected glyphs, as one step to undo.
    fn remove_selection(&mut self, cx: &mut Context<Self>) {
        let glyphs = self.selection();
        self.document.update(cx, |d, cx| d.remove_glyphs(&glyphs, cx));
        self.select(None, cx);
    }

    /// Selects the next glyph shown without strokes after the selected one (wrapping around), if
    /// any.
    pub fn select_next_empty(&mut self, cx: &mut Context<Self>) {
        let shown = self.glyphs(cx);
        let glyphs = &self.document.read(cx).project.glyphs;
        let n = shown.len();
        let from = self.selected.and_then(|s| shown.iter().position(|&g| g == s)).map_or(0, |p| p + 1);
        let next = (0..n).map(|k| shown[(from + k) % n]).find(|&g| glyphs[g].strokes.is_empty());
        if next.is_some() {
            self.select(next, cx);
        }
    }

    fn add_typed(&mut self, input: Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let text = groups::expand_ranges(&input.read(cx).value());
        self.add_chars(&text, cx);
        input.update(cx, |s, cx| s.set_value("", window, cx));
    }

    /// Adds glyphs for characters; the first new one is selected if none was.
    fn add_chars(&mut self, text: &str, cx: &mut Context<Self>) -> usize {
        let first_new = self.document.read(cx).project.glyphs.len();
        let added = self.document.update(cx, |d, cx| d.add_chars(text, cx));
        if added > 0 && self.selected.is_none() {
            self.select(Some(first_new), cx);
        }
        cx.notify();
        added
    }

    /// Adds the characters of a block the reference font has and the project does not.
    pub fn fill_block(&mut self, start: u32, cx: &mut Context<Self>) -> usize {
        let doc = self.document.read(cx);
        let Some(reference) = doc.reference().cloned() else { return 0 };
        let missing = groups::missing_in_block(&doc.project, start, |c| reference.has(c));
        self.add_chars(&missing, cx)
    }

    /// Sets how large the hand writes (points per em) from what was typed; puts the value back if
    /// it is no size.
    fn set_scale(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typed = text.trim().trim_end_matches("pt").trim().parse::<f64>().ok().filter(|v| (0.5..=500.0).contains(v));
        match typed {
            Some(pt) => self.document.update(cx, |d, cx| d.update_font(|f| f.pt_per_em = pt, cx)),
            None => {
                let pt = format_scale(self.document.read(cx).project.font.pt_per_em);
                self.scale.update(cx, |s, cx| s.set_value(pt, window, cx));
                if !text.trim().is_empty() {
                    window.push_notification(Notification::warning("手写字号要填 0.5 到 500 之间的磅数"), cx);
                }
            }
        }
    }

    /// Sets how large the hand writes from the glyphs written so far, as the reference font
    /// measures them (see [`crate::assign::estimate_pt_per_em`]).
    fn estimate_scale(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let Some(reference) = doc.reference().cloned() else {
            window.push_notification(Notification::warning("没有参考字体，没法估计"), cx);
            return;
        };
        match crate::assign::estimate_pt_per_em(&doc.project, &reference) {
            Some((pt, n)) => {
                let pt = (pt * 100.0).round() / 100.0;
                self.document.update(cx, |d, cx| d.update_font(|f| f.pt_per_em = pt, cx));
                window.push_notification(
                    Notification::success(format!("按 {n} 个已写的字估计：每 em {pt} pt。之后分配的笔画和新生成的模板按这个大小（可撤销）")),
                    cx,
                );
            }
            None => window.push_notification(Notification::warning("还没有可以量的字：先写几个参考字体里有的字"), cx),
        }
    }

    fn columns(&self) -> usize {
        ((f32::from(self.width.get()) / (CELL + 8.0)).floor() as usize).max(1)
    }

    /// The glyph's meshes, made again when it has changed.
    fn meshes_for(&self, index: usize, glyph: &Glyph, revision: u64, ascender: f64) -> Rc<Vec<Rc<Mesh>>> {
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
        // As the font draws it at the default weight, once planned; the strokes as written until then.
        let outline = self.outlines.update(cx, |o, cx| o.mesh(index, AXIS.default, cx));
        let doc = self.document.read(cx);
        let font = &doc.project.font;
        let glyph = &doc.project.glyphs[index];
        let ascender = font.ascender as f64;
        let meshes: Rc<Vec<(Rc<Mesh>, Affine)>> = match outline {
            Some(mesh) => Rc::new(vec![(mesh, Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, ascender]))]),
            None => Rc::new(self.meshes_for(index, glyph, doc.revision(index), ascender).iter().map(|m| (m.clone(), Affine::IDENTITY)).collect()),
        };
        let em = (font.ascender - font.descender) as f32;
        let advance = glyph.advance as f32;
        let label: SharedString = glyph.chars.iter().collect::<String>().into();
        let empty = glyph.strokes.is_empty();
        let selected = self.selection.contains(&index);
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
                        let y = f32::from(bounds.origin.y) + MARGIN;
                        // Font units (y down from the ascender) to the cell.
                        let to_cell = Affine::new([scale as f64, 0.0, 0.0, scale as f64, x as f64, y as f64]);
                        for (mesh, t) in meshes.iter() {
                            window.paint_path(mesh.to_path_affine(to_cell * *t), ink);
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
                let m = event.modifiers();
                if event.click_count() >= 2 {
                    this.select(Some(index), cx);
                    cx.emit(FontViewEvent::Open(index));
                } else {
                    this.click(index, m.secondary(cx), m.shift, cx);
                }
            }))
            .test_support()
    }

    /// The list of groups: status, Unicode blocks, practice sheets.
    fn render_groups(&self, listed: &[Listed], cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, active, hover, border, background) = (theme.muted_foreground, theme.primary, theme.secondary_hover, theme.border, theme.sidebar);
        let mut list = v_flex().id("groups").w(px(GROUPS)).flex_shrink_0().h_full().overflow_y_scroll().p_1().gap_px().bg(background).border_r_1().border_color(border);
        let mut section = Section::Status;
        for (i, l) in listed.iter().enumerate() {
            if l.section != section {
                section = l.section;
                let title = match section {
                    Section::Status => "状态",
                    Section::Blocks => "Unicode 区块",
                    Section::Templates => "格子模板",
                    Section::Problems => "检查",
                };
                list = list.child(div().px_2().pt_3().pb_1().text_xs().text_color(muted).child(title));
            }
            let current = l.group == self.group;
            let count = match l.group {
                Group::Unwritten | Group::Written | Group::Problem(_) => l.glyphs.to_string(),
                _ => format!("{}/{}", l.written, l.glyphs),
            };
            let group = l.group;
            list = list.child(
                h_flex()
                    .id(("group", i))
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_xs()
                    .cursor_pointer()
                    .when(current, |d| d.bg(active.opacity(0.18)))
                    .when(!current, |d| d.hover(move |d| d.bg(hover)))
                    .child(div().flex_1().min_w_0().truncate().child(l.name.clone()))
                    .child(div().flex_shrink_0().text_color(muted).child(count))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let by_code = this.by_code;
                        this.show(group, by_code, cx);
                    }))
                    .test_support(),
            );
        }
        list
    }

    /// What can be done with the group shown as a whole.
    fn render_group_actions(&self, shown: &Shown, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let listed = shown.listed.iter().find(|l| l.group == self.group);
        let (name, n, written) = listed.map_or(("", 0, 0), |l| (l.name.as_str(), l.glyphs, l.written));
        let has_reference = self.document.read(cx).reference().is_some();
        let glyphs = shown.glyphs.clone();
        let mut row = h_flex()
            .px_2()
            .py_1()
            .gap_1()
            .flex_wrap()
            .child(div().flex_1().min_w(px(120.0)).text_xs().text_color(theme.muted_foreground).child(match self.group {
                // A problem: what it means and what to do.
                Group::Problem(p) => format!("{name}：{n} 个字形。{}", p.advice()),
                _ => format!("{name}：{n} 个字形，已写 {written}"),
            }));
        match self.group {
            Group::Block(start) => {
                let missing = shown.missing;
                row = row.child(
                    Button::new("fill-block")
                        .xsmall()
                        .label(format!("补全本区块（{missing} 字）"))
                        .tooltip("加入参考字体有、工程里还没有的字")
                        .disabled(!has_reference || missing == 0)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.fill_block(start, cx);
                        })),
                );
            }
            Group::Template(id) => {
                row = row
                    .child(
                        Button::new("export-template")
                            .xsmall()
                            .label("重新导出 PDF…")
                            .on_click(cx.listener(move |_, _, _, cx| cx.emit(FontViewEvent::ExportTemplate(id)))),
                    )
                    .child(
                        Button::new("remove-template")
                            .xsmall()
                            .danger()
                            .label("删除模板")
                            .tooltip("之后导入写在这份模板上的页，不再自动归位（可撤销）")
                            .on_click(cx.listener(move |this, _, _, cx| this.document.update(cx, |d, cx| d.remove_template(id, cx)))),
                    );
            }
            _ => {}
        }
        row.child(
            Button::new("template-group")
                .xsmall()
                .label("为本组生成格子模板…")
                .disabled(glyphs.is_empty())
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(FontViewEvent::MakeTemplate(glyphs.clone())))),
        )
    }

    /// What can be done with the glyphs selected.
    fn render_selection_actions(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let n = self.selection.len();
        let font = self.document.read(cx).project.font.clone();
        let has_reference = self.document.read(cx).reference().is_some();
        let selection = self.selection();
        h_flex()
            .px_2()
            .pb_1()
            .gap_1()
            .flex_wrap()
            .text_xs()
            .child(div().flex_1().min_w(px(60.0)).child(format!("已选 {n} 个：")))
            .child(Button::new("fit-selection").xsmall().label("按参考摆放").disabled(!has_reference).on_click(cx.listener(|this, _, _, cx| {
                let glyphs = this.selection();
                this.document.update(cx, |d, cx| d.fit_to_reference(&glyphs, cx));
            })))
            .child(
                Button::new("full-width-selection")
                    .xsmall()
                    .label("全角")
                    .on_click(cx.listener(move |this, _, _, cx| this.edit_selection(cx, |g| g.fit_full_width(&font, FACE)))),
            )
            .child(Button::new("center-selection").xsmall().label("居中").on_click(cx.listener(|this, _, _, cx| this.edit_selection(cx, Glyph::center))))
            .child(
                Button::new("even-widths")
                    .xsmall()
                    .label("统一笔宽")
                    .tooltip("每一笔的粗细向全字体的典型笔宽靠拢七成（只改笔宽）")
                    .on_click(cx.listener(|this, _, _, cx| this.even_widths(cx))),
            )
            .child(
                Button::new("level-horizontals")
                    .xsmall()
                    .label("横画取平")
                    .tooltip("上下错切每个字，让横画的倾斜和全字体的典型倾斜一致（竖画保持竖直）")
                    .on_click(cx.listener(|this, _, _, cx| this.level_horizontals(cx))),
            )
            .child(
                Button::new("template-selection")
                    .xsmall()
                    .label("生成格子模板…")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(FontViewEvent::MakeTemplate(selection.clone())))),
            )
            .child(
                Button::new("remove-selection")
                    .xsmall()
                    .danger()
                    .label("删除")
                    .tooltip("删除选中的字形（可撤销）")
                    .on_click(cx.listener(|this, _, _, cx| this.remove_selection(cx))),
            )
    }
}

/// A size in points per em as shown.
fn format_scale(pt: f64) -> String {
    let text = format!("{pt:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A glyph's strokes as written, placed in the em, y down from the ascender (font units).
pub fn glyph_meshes(glyph: &Glyph, ascender: f64) -> Vec<Rc<Mesh>> {
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
            Mesh::from_path_with_tolerance(&path, shape, true, TOLERANCE).map(Rc::new)
        })
        .collect()
}

impl Render for FontView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(cx);
        let shown = self.shown.take().expect("refreshed");
        let shown_glyphs = Rc::new(shown.glyphs.clone());
        let count = shown_glyphs.len();
        let columns = self.columns();
        let rows = count.div_ceil(columns);
        let width = self.width.clone();
        let input = self.input.clone();
        let weights = self.shown_weights(cx);
        let groups = self.render_groups(&shown.listed, cx);
        let group_actions = self.render_group_actions(&shown, cx);
        let selection_actions = (!self.selection.is_empty()).then(|| self.render_selection_actions(cx));
        self.shown = Some(shown);
        let by_code = self.by_code;
        let theme = cx.theme();
        let grid = if count == 0 {
            div().flex_1().p_4().text_sm().text_color(theme.muted_foreground).child("这一组还没有字形").into_any_element()
        } else {
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
                                    let cells = shown_glyphs[row * columns..((row + 1) * columns).min(count)].iter().map(|&g| this.render_cell(g, cx)).collect::<Vec<_>>();
                                    h_flex().gap_2().px_2().py_1().children(cells)
                                })
                                .collect()
                        }),
                    )
                    .size_full(),
                )
                .into_any_element()
        };
        let theme = cx.theme();
        h_flex()
            .size_full()
            .bg(theme.background)
            .child(groups)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        h_flex()
                            .gap_2()
                            .p_2()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(div().flex_1().child(Input::new(&self.input).id("chars")))
                            .child(Button::new("add-chars").primary().label("加入").on_click(cx.listener(move |this, _, window, cx| {
                                this.add_typed(input.clone(), window, cx);
                            })))
                            .child(
                                Button::new("add-charset")
                                    .label("字符集…")
                                    .tooltip("按国家标准（通用规范汉字表、GB 2312、Big5、JIS、KS）或 Unicode 区块加字")
                                    .on_click(cx.listener(|this, _, window, cx| crate::app::charset_picker::open(this.document.clone(), window, cx))),
                            ),
                    )
                    .child(
                        // Which weight the handwriting is: the others are its strokes thinned or thickened.
                        h_flex()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(theme.border)
                            .text_xs()
                            .child(div().flex_shrink_0().child("原稿字重"))
                            .child(div().id("written-weight").flex_1().min_w(px(80.0)).child(Slider::new(&self.written)).test_support())
                            .child(div().flex_shrink_0().w(px(28.0)).child(format!("{:.0}", weights.written)))
                            .child(div().flex_shrink_0().text_color(theme.muted_foreground).child(format!(
                                "100 = ×{:.2} · 900 = ×{:.2}",
                                weights.scale(AXIS.min),
                                weights.scale(AXIS.max)
                            ))),
                    )
                    .child(
                        // How large the hand writes: new strokes are placed at it, sheets' squares are its em.
                        h_flex()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .border_b_1()
                            .border_color(theme.border)
                            .text_xs()
                            .child(div().flex_shrink_0().child("手写字号"))
                            .child(div().w(px(64.0)).flex_shrink_0().child(Input::new(&self.scale).xsmall().id("pt-per-em")))
                            .child(div().flex_shrink_0().child("pt / em"))
                            .child(
                                Button::new("estimate-scale")
                                    .xsmall()
                                    .label("按已写的字估计")
                                    .tooltip("量已写的字和参考字体比有多大。之后分配的笔画、新生成的格子模板的方格都按这个大小")
                                    .on_click(cx.listener(|this, _, window, cx| this.estimate_scale(window, cx))),
                            )
                    )
                    .child(
                        h_flex()
                            .px_2()
                            .py_1()
                            .gap_1()
                            .child(div().flex_1().text_xs().text_color(theme.muted_foreground).child("单击选中（Ctrl/Shift 多选），双击打开"))
                            .child(Button::new("select-all").xsmall().ghost().label("全选").on_click(cx.listener(|this, _, _, cx| this.select_all(cx))))
                            .child(
                                Button::new("by-code")
                                    .xsmall()
                                    .ghost()
                                    .label("按码位排")
                                    .selected(by_code)
                                    .tooltip("按码位排列，而不是按加入的先后")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let group = this.group;
                                        this.show(group, !by_code, cx);
                                    })),
                            )
                            .child(
                                Button::new("open-sample")
                                    .xsmall()
                                    .label("样张")
                                    .tooltip("把一段文字按现在的摆放排出来看")
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FontViewEvent::OpenSample))),
                            ),
                    )
                    .child(group_actions)
                    .children(selection_actions)
                    .child(grid),
            )
    }
}
