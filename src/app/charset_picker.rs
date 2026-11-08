//! Adding characters a set at a time (in a dialog): national standards' sets and Unicode blocks
//! (a block's characters that the reference font draws, so that no unassigned code point is
//! added). Each set shows how many of its characters the project has; the list is filtered by
//! name or code point.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use gpui::TestSupportExt as _;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::notification::Notification;
use gpui::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui::prelude::*;
use gpui::{App, Context, Entity, Subscription, Window, div, px};
use scriptoria_charsets::{Block, Charset};

use crate::app::document::Document;

/// A set chosen: a standard's (by its place in the list) or a block (by its first code point).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Choice {
    Standard(usize),
    Block(u32),
}

pub struct CharsetPicker {
    document: Entity<Document>,
    filter: Entity<InputState>,
    standards: Rc<Vec<Charset>>,
    blocks: Rc<Vec<Block>>,
    chosen: BTreeSet<Choice>,
    /// Per block: how many of its characters the reference font draws.
    drawn: RefCell<HashMap<u32, usize>>,
    _subscriptions: Vec<Subscription>,
}

impl CharsetPicker {
    pub fn new(document: Entity<Document>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("筛选：名称或码位，如 汉字、Hiragana、4E00"));
        let subscriptions = vec![cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify())];
        CharsetPicker {
            document,
            filter,
            standards: Rc::new(scriptoria_charsets::standards()),
            blocks: Rc::new(scriptoria_charsets::blocks()),
            chosen: BTreeSet::new(),
            drawn: RefCell::default(),
            _subscriptions: subscriptions,
        }
    }

    /// Chooses a set, or lets it go.
    pub fn toggle(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if !self.chosen.remove(&choice) {
            self.chosen.insert(choice);
        }
        cx.notify();
    }

    /// The characters of the sets chosen, in the order of the list, each once.
    pub fn chosen_chars(&self, cx: &App) -> String {
        let reference = self.document.read(cx).reference().cloned();
        let mut seen = HashSet::new();
        let mut out = String::new();
        for choice in &self.chosen {
            let chars: Box<dyn Iterator<Item = char>> = match *choice {
                Choice::Standard(i) => Box::new(self.standards[i].chars.clone().into_iter()),
                Choice::Block(start) => match (scriptoria_charsets::block(start), &reference) {
                    (Some(b), Some(r)) => {
                        let r = r.clone();
                        Box::new(b.chars().filter(move |&c| r.has(c)))
                    }
                    _ => Box::new(std::iter::empty()),
                },
            };
            out.extend(chars.filter(|c| seen.insert(*c)));
        }
        out
    }

    /// Adds glyphs for the characters chosen (one step to undo); tells how many, closes the dialog.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.chosen_chars(cx);
        let added = self.document.update(cx, |d, cx| d.add_chars(&text, cx));
        window.close_dialog(cx);
        window.push_notification(Notification::success(format!("加入了 {added} 个字形（可撤销）")), cx);
    }

    /// How many of a block's characters the reference font draws (counted once).
    fn drawn(&self, b: &Block, cx: &App) -> Option<usize> {
        let reference = self.document.read(cx).reference()?;
        Some(*self.drawn.borrow_mut().entry(b.start).or_insert_with(|| b.chars().filter(|&c| reference.has(c)).count()))
    }

    fn row(&self, id: (&'static str, usize), name: String, have: usize, total: usize, choice: Choice, disabled: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, active, hover) = (theme.muted_foreground, theme.primary, theme.secondary_hover);
        let chosen = self.chosen.contains(&choice);
        h_flex()
            .id(id)
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_sm()
            .when(!disabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
            .when(chosen, |d| d.bg(active.opacity(0.18)))
            .when(disabled, |d| d.text_color(muted))
            .child(div().w(px(16.0)).child(if chosen { "☑" } else { "☐" }))
            .child(div().flex_1().min_w_0().truncate().child(name))
            .child(div().flex_shrink_0().text_xs().text_color(muted).child(if total == 0 { "参考字体里没有".to_string() } else { format!("已有 {have} / {total}") }))
            .when(!disabled, |d| d.on_click(cx.listener(move |this, _, _, cx| this.toggle(choice, cx))))
            .test_support()
    }
}

impl Render for CharsetPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        let code = u32::from_str_radix(filter.trim_start_matches("u+"), 16).ok().filter(|_| filter.len() >= 2);
        let matches = |name: &str, english: &str, block: Option<&Block>| {
            filter.is_empty() || name.to_lowercase().contains(&filter) || english.to_lowercase().contains(&filter) || code.zip(block).is_some_and(|(c, b)| (b.start..=b.end).contains(&c))
        };
        let doc = self.document.read(cx);
        let have: HashSet<char> = doc.project.glyphs.iter().flat_map(|g| g.chars.iter().copied()).collect();
        let has_reference = doc.reference().is_some();
        let standards = self.standards.clone();
        let blocks = self.blocks.clone();
        let mut rows = Vec::new();
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let header = |text: &'static str| div().px_2().pt_2().pb_1().text_xs().text_color(muted).child(text);
        let standard_rows: Vec<_> = standards
            .iter()
            .enumerate()
            .filter(|(_, s)| matches(&s.name, "", None))
            .map(|(i, s)| {
                let n = s.chars.iter().filter(|c| have.contains(c)).count();
                self.row(("standard", i), s.name.clone(), n, s.chars.len(), Choice::Standard(i), false, cx).into_any_element()
            })
            .collect();
        if !standard_rows.is_empty() {
            rows.push(header("国家标准与常用集").into_any_element());
            rows.extend(standard_rows);
        }
        let block_rows: Vec<_> = blocks
            .iter()
            .filter(|b| matches(&b.name(), b.english, Some(b)))
            .map(|b| {
                let total = self.drawn(b, cx).unwrap_or(0);
                let n = b.chars().filter(|c| have.contains(c)).count();
                let name = format!("{}  U+{:04X}–{:04X}", b.name(), b.start, b.end);
                self.row(("block", b.start as usize), name, n, total, Choice::Block(b.start), total == 0, cx).into_any_element()
            })
            .collect();
        if !block_rows.is_empty() {
            rows.push(header(if has_reference { "Unicode 区块（只加参考字体有的字）" } else { "Unicode 区块（需要参考字体）" }).into_any_element());
            rows.extend(block_rows);
        }
        let count = self.chosen_chars(cx).chars().filter(|c| !have.contains(c)).count();
        v_flex()
            .gap_2()
            .w_full()
            .child(Input::new(&self.filter).id("charset-filter"))
            .child(div().id("charsets").h(px(380.0)).overflow_y_scroll().border_1().border_color(border).rounded_md().p_1().children(rows))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().text_sm().text_color(muted).child(format!("已选 {} 个字集，新字 {count} 个", self.chosen.len())))
                    .child(Button::new("cancel-charsets").label("取消").on_click(cx.listener(|_, _, window, cx| window.close_dialog(cx))))
                    .child(
                        Button::new("add-charsets")
                            .primary()
                            .label("加入")
                            .disabled(count == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    ),
            )
    }
}

/// Opens the dialog adding sets of characters to a project.
pub fn open(document: Entity<Document>, window: &mut Window, cx: &mut App) {
    let picker = cx.new(|cx| CharsetPicker::new(document, window, cx));
    window.open_dialog(cx, move |dialog, _, _| dialog.title("加入字符集").w(px(600.0)).child(picker.clone()));
}
