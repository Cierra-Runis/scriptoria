//! Where a part is used and could be (in a dialog): the project's glyphs whose character has the
//! part's component, by what the characters are made of (see [`crate::components`]): those linked
//! to it, those written with strokes of their own, and those still empty, which it is put into
//! (each where the component sits) a selection at a time. Characters of 通用规范汉字表 with the
//! component are offered to add; and empty glyphs the parts make up between them are put together.

use std::collections::BTreeSet;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputState};
use gpui::component::notification::Notification;
use gpui::component::{ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{App, Context, Entity, Window, div, px};
use scriptoria_ids::Table;

use crate::app::document::Document;
use crate::components::{self, UseKind};

pub struct PartUses {
    document: Entity<Document>,
    part: u32,
    /// The empty glyphs chosen to put the part into.
    chosen: BTreeSet<usize>,
    /// The component, when the part has none (or another is wanted).
    component: Entity<InputState>,
}

impl PartUses {
    pub fn new(document: Entity<Document>, part: u32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = document.read(cx).project.part(part).and_then(|p| p.component).map(String::from).unwrap_or_default();
        let component = cx.new(|cx| InputState::new(window, cx).placeholder("如 氵").default_value(current));
        let mut this = PartUses { document, part, chosen: BTreeSet::new(), component };
        this.chosen = this.empty(cx).into_iter().collect();
        this
    }

    fn table() -> &'static Table {
        Table::builtin()
    }

    /// The empty glyphs with the component.
    fn empty(&self, cx: &App) -> Vec<usize> {
        components::uses(Self::table(), &self.document.read(cx).project, self.part).into_iter().filter(|u| u.kind == UseKind::Empty).map(|u| u.glyph).collect()
    }

    /// Chooses an empty glyph, or lets it go.
    pub fn toggle(&mut self, glyph: usize, cx: &mut Context<Self>) {
        if !self.chosen.remove(&glyph) {
            self.chosen.insert(glyph);
        }
        cx.notify();
    }

    /// Takes the component typed as the part's (one character), as a step to undo.
    fn set_component(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.component.read(cx).value().trim().to_string();
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            window.push_notification(Notification::warning("部件字填一个字，如 氵、口、木"), cx);
            return;
        };
        let part = self.part;
        self.document.update(cx, |d, cx| d.update_project(&[], |p| p.part_mut(part).map(|p| p.component = Some(c)), cx));
        self.chosen = self.empty(cx).into_iter().collect();
        cx.notify();
    }

    /// Puts the part into the empty glyphs chosen, as one step to undo.
    pub fn put_into_chosen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let glyphs: Vec<usize> = std::mem::take(&mut self.chosen).into_iter().collect();
        let part = self.part;
        let n = self.document.update(cx, |d, cx| {
            d.update_project(&glyphs, |p| glyphs.iter().map(|&g| components::put_into(Self::table(), p, part, g)).filter(|&n| n > 0).count(), cx)
        });
        window.push_notification(Notification::success(format!("部件已加进 {n} 个字（链接；可撤销）")), cx);
        cx.notify();
    }

    /// Adds glyphs for the characters of 通用规范汉字表 with the component (one step to undo).
    pub fn add_common(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let common = scriptoria_charsets::tongyong_guifan();
        let missing: String = components::missing_with(Self::table(), &self.document.read(cx).project, self.part, &common).into_iter().collect();
        let n = self.document.update(cx, |d, cx| d.add_chars(&missing, cx));
        self.chosen.extend(self.empty(cx));
        window.push_notification(Notification::success(format!("加入了 {n} 个含这个部件的常用字（可撤销）")), cx);
        cx.notify();
    }

    /// Puts together every empty glyph the parts make up between them, as one step to undo.
    pub fn compose_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let glyphs: Vec<usize> =
            (0..doc.project.glyphs.len()).filter(|&g| doc.project.glyphs[g].strokes.is_empty() && components::composable(Self::table(), &doc.project, g).is_some()).collect();
        let (made, enclosed) = self.document.update(cx, |d, cx| {
            d.update_project(
                &glyphs,
                |p| {
                    let made: Vec<_> = glyphs.iter().filter_map(|&g| components::compose(Self::table(), p, g)).collect();
                    (made.len(), made.iter().filter(|c| c.enclosed).count())
                },
                cx,
            )
        });
        let check = if enclosed > 0 { format!("；其中 {enclosed} 个有包围结构，笔顺见“检查 → 笔顺需核对”") } else { String::new() };
        window.push_notification(Notification::success(format!("用部件拼出了 {made} 个字（可撤销）{check}")), cx);
        self.chosen = self.empty(cx).into_iter().collect();
        cx.notify();
    }
}

impl Render for PartUses {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.document.read(cx);
        let project = &doc.project;
        let Some(part) = project.part(self.part) else { return div().child("这个部件已不存在").into_any_element() };
        let component = part.component;
        let found = components::uses(Self::table(), project, self.part);
        let composable = (0..project.glyphs.len()).filter(|&g| project.glyphs[g].strokes.is_empty() && components::composable(Self::table(), project, g).is_some()).count();
        let common = scriptoria_charsets::tongyong_guifan();
        let missing = components::missing_with(Self::table(), project, self.part, &common).len();
        let theme = cx.theme();
        let (muted, border, active, hover) = (theme.muted_foreground, theme.border, theme.primary, theme.secondary_hover);
        let label = |g: usize| project.glyphs[g].chars.iter().collect::<String>();
        let header = |text: String| div().px_1().pt_2().pb_1().text_xs().text_color(muted).child(text);
        let chips = |kind: UseKind| -> Vec<String> { found.iter().filter(|u| u.kind == kind).map(|u| label(u.glyph)).collect() };
        let (linked, written) = (chips(UseKind::Linked), chips(UseKind::Written));
        let empty: Vec<(usize, String, usize)> = found.iter().filter(|u| u.kind == UseKind::Empty).map(|u| (u.glyph, label(u.glyph), u.regions.len())).collect();
        let plain = |text: String| div().px_1().text_lg().child(text);
        let empty_chips: Vec<_> = empty
            .iter()
            .map(|&(g, ref text, times)| {
                let chosen = self.chosen.contains(&g);
                div()
                    .id(("empty-use", g))
                    .px_1()
                    .rounded_sm()
                    .text_lg()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if chosen { active } else { border })
                    .when(chosen, |d| d.bg(active.opacity(0.18)))
                    .when(!chosen, |d| d.hover(move |d| d.bg(hover)))
                    .child(if times > 1 { format!("{text}×{times}") } else { text.clone() })
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(g, cx)))
                    .test_support()
            })
            .collect();
        let n = self.chosen.len();
        let wrap = || h_flex().flex_wrap().gap_1().px_1();
        v_flex()
            .gap_2()
            .w_full()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().child("部件字"))
                    .child(div().w(px(60.0)).child(Input::new(&self.component).small().id("part-component")))
                    .child(Button::new("set-component").small().label("设定").on_click(cx.listener(|this, _, window, cx| this.set_component(window, cx))))
                    .child(div().text_xs().text_color(muted).child("按汉字拆分（IDS）找含这个字的字；部首的变体（⺝ 与 月 等）也算")),
            )
            .child(match component {
                None => div().text_sm().text_color(muted).child("这个部件还没有部件字：填一个再设定").into_any_element(),
                Some(c) => v_flex()
                    .id("part-uses")
                    .h(px(360.0))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(border)
                    .rounded_md()
                    .p_1()
                    .child(header(format!("已链接到「{}」（{}）", part.name, linked.len())))
                    .child(wrap().children(linked.into_iter().map(plain)))
                    .child(header(format!("已写、有自己的笔画（{}）：打开后选中笔画，在部件标签页用“替换选中”", written.len())))
                    .child(wrap().children(written.into_iter().map(plain)))
                    .child(header(format!("含「{c}」的空字形（{}）：点选，再加进去", empty.len())))
                    .child(wrap().children(empty_chips))
                    .into_any_element(),
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("add-common")
                            .small()
                            .label(format!("加入含它的常用字（{missing}）"))
                            .tooltip("通用规范汉字表里含这个部件、工程里还没有的字")
                            .disabled(missing == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.add_common(window, cx))),
                    )
                    .child(
                        Button::new("compose-all")
                            .small()
                            .label(format!("用部件拼字（{composable}）"))
                            .tooltip("空字形里，能完全用已有部件拼出来的（如 清 = 氵 + 青），按拆分放好各部件（链接）")
                            .disabled(composable == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.compose_all(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(Button::new("close-part-uses").label("关闭").on_click(cx.listener(|_, _, window, cx| window.close_dialog(cx))))
                    .child(
                        Button::new("put-part")
                            .primary()
                            .label(format!("加进选中的 {n} 个字"))
                            .disabled(n == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.put_into_chosen(window, cx))),
                    ),
            )
            .into_any_element()
    }
}

/// Opens the dialog of where a part is used and could be.
pub fn open(document: Entity<Document>, part: u32, window: &mut Window, cx: &mut App) {
    let name = document.read(cx).project.part(part).map(|p| p.name.clone()).unwrap_or_default();
    let view = cx.new(|cx| PartUses::new(document, part, window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.title(format!("部件「{name}」用在哪些字")).w(px(640.0)).child(view.clone()));
}
