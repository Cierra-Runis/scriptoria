//! The font's settings in a dialog (as FontForge's Font Info): its names, its lines, its weights,
//! how large the hand writes, the reference font (a file and a face of it) and how closely glyphs
//! take its sizes. All are applied together, as one step to undo.

use std::path::PathBuf;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputState};
use gpui::component::notification::Notification;
use gpui::component::{ActiveTheme as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{App, Context, Entity, PathPromptOptions, SharedString, Window, div, px};
use scriptoria_project::{FontInfo, Reference};
use scriptoria_reference::Face;

use crate::app::document::Document;

/// A setting typed: what it is, and how it is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Family,
    English,
    Version,
    Copyright,
    Designer,
    License,
    Ascender,
    Descender,
    LineGap,
    XHeight,
    CapHeight,
    PtPerEm,
    Thinnest,
    FitStrength,
}

impl Field {
    const ALL: [Field; 14] = [
        Field::Family,
        Field::English,
        Field::Version,
        Field::Copyright,
        Field::Designer,
        Field::License,
        Field::Ascender,
        Field::Descender,
        Field::LineGap,
        Field::XHeight,
        Field::CapHeight,
        Field::PtPerEm,
        Field::Thinnest,
        Field::FitStrength,
    ];

    /// Its label, the id of its box, and a note beside it.
    fn label(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Field::Family => ("字族名", "info-family", "字体菜单里显示的名字"),
            Field::English => ("英文名", "info-english", "拉丁字母的名字，可留空"),
            Field::Version => ("版本", "info-version", "如 1.000"),
            Field::Copyright => ("版权", "info-copyright", ""),
            Field::Designer => ("设计者", "info-designer", ""),
            Field::License => ("许可", "info-license", "如 SIL Open Font License 1.1"),
            Field::Ascender => ("上伸", "info-ascender", "字身框顶（字体单位）"),
            Field::Descender => ("下伸", "info-descender", "字身框底，负数"),
            Field::LineGap => ("行距", "info-line-gap", "行与行之间另加的空白"),
            Field::XHeight => ("x 高度", "info-x-height", "小写字母高，0 为不写"),
            Field::CapHeight => ("大写高度", "info-cap-height", "大写字母高，0 为不写"),
            Field::PtPerEm => ("手写字号", "info-pt-per-em", "pt / em：手写一个 em 多大"),
            Field::Thinnest => ("最细倍数", "info-thinnest", "wght 100 时笔宽是原稿的几倍"),
            Field::FitStrength => ("贴合强度", "info-fit-strength", "0 保持手写大小，1 完全照参考"),
        }
    }

    fn get(self, f: &FontInfo) -> String {
        match self {
            Field::Family => f.family.clone(),
            Field::English => f.english_name.clone(),
            Field::Version => format!("{:.3}", f.version),
            Field::Copyright => f.copyright.clone(),
            Field::Designer => f.designer.clone(),
            Field::License => f.license.clone(),
            Field::Ascender => f.ascender.to_string(),
            Field::Descender => f.descender.to_string(),
            Field::LineGap => f.line_gap.to_string(),
            Field::XHeight => f.x_height.to_string(),
            Field::CapHeight => f.cap_height.to_string(),
            Field::PtPerEm => f.pt_per_em.to_string(),
            Field::Thinnest => f.thinnest.to_string(),
            Field::FitStrength => f.fit_strength.to_string(),
        }
    }

    /// Puts what was typed into the settings; an error says what is wrong with it.
    fn set(self, f: &mut FontInfo, text: &str) -> Result<(), String> {
        let text = text.trim();
        let (label, _, _) = self.label();
        let number = |lo: f64, hi: f64| -> Result<f64, String> {
            text.parse::<f64>().ok().filter(|v| v.is_finite() && (lo..=hi).contains(v)).ok_or_else(|| format!("{label}要填 {lo} 到 {hi} 之间的数"))
        };
        let units = |lo: f64, hi: f64| number(lo, hi).map(|v| v.round() as i16);
        match self {
            Field::Family if text.is_empty() => return Err("字族名不能空着".into()),
            Field::Family => f.family = text.to_string(),
            Field::English => f.english_name = text.to_string(),
            Field::Version => f.version = number(0.001, 999.0)?,
            Field::Copyright => f.copyright = text.to_string(),
            Field::Designer => f.designer = text.to_string(),
            Field::License => f.license = text.to_string(),
            Field::Ascender => f.ascender = units(1.0, 4000.0)?,
            Field::Descender => f.descender = units(-4000.0, 0.0)?,
            Field::LineGap => f.line_gap = units(0.0, 4000.0)?,
            Field::XHeight => f.x_height = units(0.0, 4000.0)?,
            Field::CapHeight => f.cap_height = units(0.0, 4000.0)?,
            Field::PtPerEm => f.pt_per_em = number(0.5, 500.0)?,
            Field::Thinnest => f.thinnest = number(0.05, 1.0)?,
            Field::FitStrength => f.fit_strength = number(0.0, 1.0)?,
        }
        Ok(())
    }
}

pub struct FontInfoForm {
    document: Entity<Document>,
    fields: Vec<(Field, Entity<InputState>)>,
    /// The reference font chosen (none: no reference), and the faces of its file.
    reference: Option<Reference>,
    faces: Vec<Face>,
}

impl FontInfoForm {
    pub fn new(document: Entity<Document>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let info = document.read(cx).project.font.clone();
        let fields = Field::ALL.iter().map(|&f| (f, cx.new(|cx| InputState::new(window, cx).default_value(f.get(&info))))).collect();
        let faces = info.reference.as_ref().and_then(|r| scriptoria_reference::faces(&r.file).ok()).unwrap_or_default();
        FontInfoForm { document, fields, reference: info.reference, faces }
    }

    /// The settings as typed; or what is wrong with them.
    pub fn settings(&self, cx: &App) -> Result<FontInfo, String> {
        let mut info = self.document.read(cx).project.font.clone();
        for (field, input) in &self.fields {
            field.set(&mut info, &input.read(cx).value())?;
        }
        if info.descender >= info.ascender {
            return Err("下伸要低于上伸".into());
        }
        info.reference = self.reference.clone();
        Ok(info)
    }

    /// Uses a file's face as the reference.
    pub fn choose_reference(&mut self, file: PathBuf, face: Option<u32>, cx: &mut Context<Self>) {
        match scriptoria_reference::faces(&file) {
            Ok(faces) => {
                let face = face.unwrap_or_else(|| scriptoria_reference::default_face(&faces));
                self.faces = faces;
                self.reference = Some(Reference { file, face });
            }
            Err(_) => {
                self.faces.clear();
                self.reference = None;
            }
        }
        cx.notify();
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("选择参考字体".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(file) = paths.into_iter().next()
            {
                this.update(cx, |this, cx| this.choose_reference(file, None, cx)).ok();
            }
        })
        .detach();
    }

    /// The reference's x-height and cap height (of x and H), in the font's units, into their boxes.
    fn heights_from_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let Some(reference) = doc.reference().cloned() else {
            window.push_notification(Notification::warning("没有参考字体"), cx);
            return;
        };
        let upm = doc.project.font.units_per_em as f64;
        let top = |c: char| reference.glyph(c, upm).map_or(0.0, |g| g.bounds.y1.round());
        for (field, value) in [(Field::XHeight, top('x')), (Field::CapHeight, top('H'))] {
            if let Some((_, input)) = self.fields.iter().find(|(f, _)| *f == field) {
                input.update(cx, |s, cx| s.set_value(value.to_string(), window, cx));
            }
        }
    }

    /// Applies the settings (one step to undo) and closes the dialog; or says what is wrong.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.settings(cx).and_then(|info| self.document.update(cx, |d, cx| d.replace_font(info, cx)).map_err(|e| format!("参考字体打不开：{e}")));
        match result {
            Ok(()) => window.close_dialog(cx),
            Err(e) => window.push_notification(Notification::warning(e), cx),
        }
    }

    fn row(&self, field: Field, input: &Entity<InputState>, cx: &App) -> impl IntoElement + use<> {
        let (label, id, note) = field.label();
        h_flex()
            .gap_2()
            .text_sm()
            .child(div().w(px(72.0)).flex_shrink_0().child(label))
            .child(div().flex_1().child(Input::new(input).small().id(id)))
            .child(div().w(px(180.0)).flex_shrink_0().text_xs().text_color(cx.theme().muted_foreground).child(note))
    }
}

impl Render for FontInfoForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, active) = (theme.muted_foreground, theme.border, theme.primary);
        let section = |title: &'static str| div().pt_2().text_xs().text_color(muted).child(title);
        let rows = |fields: &[Field], this: &Self, cx: &App| -> Vec<gpui::AnyElement> {
            fields.iter().filter_map(|f| this.fields.iter().find(|(g, _)| g == f)).map(|(f, input)| this.row(*f, input, cx).into_any_element()).collect()
        };
        let names = rows(&[Field::Family, Field::English, Field::Version, Field::Copyright, Field::Designer, Field::License], self, cx);
        let lines = rows(&[Field::Ascender, Field::Descender, Field::LineGap, Field::XHeight, Field::CapHeight], self, cx);
        let hand = rows(&[Field::PtPerEm, Field::Thinnest, Field::FitStrength], self, cx);
        let file: SharedString = self.reference.as_ref().map_or("（不用参考字体）".into(), |r| r.file.display().to_string().into());
        let chosen = self.reference.as_ref().map(|r| r.face);
        let faces: Vec<_> = self
            .faces
            .iter()
            .map(|face| {
                let index = face.index;
                let current = chosen == Some(index);
                h_flex()
                    .id(("face", index as usize))
                    .px_2()
                    .py_0p5()
                    .text_xs()
                    .rounded_sm()
                    .cursor_pointer()
                    .when(current, |d| d.bg(active.opacity(0.18)))
                    .child(format!("{} {}", face.family, face.style))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(r) = &mut this.reference {
                            r.face = index;
                        }
                        cx.notify();
                    }))
                    .test_support()
            })
            .collect();
        v_flex()
            .gap_1()
            .w_full()
            .child(div().id("font-info").max_h(px(520.0)).overflow_y_scroll().pr_2().child(
                v_flex()
                    .gap_1()
                    .child(section("名称"))
                    .children(names)
                    .child(section("度量（字体单位）"))
                    .children(lines)
                    .child(
                        h_flex().justify_end().child(
                            Button::new("heights-from-reference")
                                .xsmall()
                                .label("x 高度和大写高度照参考字体")
                                .on_click(cx.listener(|this, _, window, cx| this.heights_from_reference(window, cx))),
                        ),
                    )
                    .child(section("手写与字重"))
                    .children(hand)
                    .child(section("参考字体"))
                    .child(div().text_xs().child(file))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(Button::new("reference-file").xsmall().label("选择字体文件…").on_click(cx.listener(|this, _, window, cx| this.pick_file(window, cx))))
                            .child(Button::new("reference-system").xsmall().label("用系统里的思源黑体").on_click(cx.listener(|this, _, _, cx| {
                                if let Some(file) = scriptoria_reference::system_default() {
                                    this.choose_reference(file, None, cx);
                                }
                            })))
                            .child(Button::new("reference-none").xsmall().ghost().label("不用参考字体").on_click(cx.listener(|this, _, _, cx| {
                                this.reference = None;
                                this.faces.clear();
                                cx.notify();
                            }))),
                    )
                    .when(self.faces.len() > 1, |d| {
                        d.child(div().text_xs().text_color(muted).child("字体集里的字面（点选）："))
                            .child(div().id("faces").max_h(px(140.0)).overflow_y_scroll().border_1().border_color(border).rounded_md().p_1().children(faces))
                    }),
            ))
            .child(
                h_flex()
                    .gap_2()
                    .pt_2()
                    .justify_end()
                    .child(Button::new("font-info-cancel").label("取消").on_click(cx.listener(|_, _, window, cx| window.close_dialog(cx))))
                    .child(Button::new("font-info-ok").primary().label("确定").on_click(cx.listener(|this, _, window, cx| this.apply(window, cx)))),
            )
    }
}

/// Opens the font's settings.
pub fn open(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| FontInfoForm::new(document, window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.title("字体信息").w(px(640.0)).child(form.clone()));
}
