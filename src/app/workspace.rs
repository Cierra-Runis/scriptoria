//! The main window: the menu bar, the glyph grid on the left, the opened documents and glyphs in
//! tabs on the right. File operations (new, open, save, import, export) and undo live here.

use std::path::{Path, PathBuf};

use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::AppMenuBar;
use gpui::component::notification::Notification;
use gpui::component::tab::{Tab, TabBar};
use gpui::component::{ActiveTheme as _, GlobalState, Sizable as _, TitleBar, WindowExt as _, h_resizable, resizable_panel, v_flex};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString, Subscription, Task, Window,
    actions, div, px,
};
use scriptoria_ink::InkDocument;
use scriptoria_project::Project;

use crate::app::document::{Document, DocumentEvent};
use crate::app::font_view::{FontView, FontViewEvent};
use crate::app::glyph_view::GlyphView;
use crate::app::outlines::Outlines;
use crate::app::sample_view::{SampleEvent, SampleView};
use crate::app::viewer::{Viewer, ViewerEvent};

actions!(scriptoria, [NewProject, OpenProject, SaveProject, SaveProjectAs, ImportPdf, ExportFont, Quit, Undo, Redo, ShowSample]);

/// Key bindings and menus, once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewProject, None),
        KeyBinding::new("ctrl-o", OpenProject, None),
        KeyBinding::new("ctrl-s", SaveProject, None),
        KeyBinding::new("ctrl-shift-s", SaveProjectAs, None),
        KeyBinding::new("ctrl-i", ImportPdf, None),
        KeyBinding::new("ctrl-e", ExportFont, None),
        KeyBinding::new("ctrl-q", Quit, None),
        // Text inputs keep their own undo (their bindings are more specific).
        KeyBinding::new("ctrl-z", Undo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("ctrl-shift-z", Redo, None),
        KeyBinding::new("ctrl-t", ShowSample, None),
    ]);
    GlobalState::global_mut(cx).set_app_menus(menus().into_iter().map(Menu::owned).collect());
    cx.set_menus(menus());
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "文件".into(),
            items: vec![
                MenuItem::action("新建工程", NewProject),
                MenuItem::action("打开工程…", OpenProject),
                MenuItem::separator(),
                MenuItem::action("保存", SaveProject),
                MenuItem::action("另存为…", SaveProjectAs),
                MenuItem::separator(),
                MenuItem::action("导入 PDF…", ImportPdf),
                MenuItem::action("导出字体…", ExportFont),
                MenuItem::separator(),
                MenuItem::action("退出", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "编辑".into(),
            items: vec![MenuItem::action("撤销", Undo), MenuItem::action("重做", Redo)],
            disabled: false,
        },
        Menu { name: "视图".into(), items: vec![MenuItem::action("样张", ShowSample)], disabled: false },
    ]
}

/// A document or glyph open in a tab.
enum Page {
    /// A handwritten document, with the id of its source in the project.
    Pdf { title: SharedString, source: String, view: Entity<Viewer> },
    /// A glyph, by its index in the project.
    Glyph { title: SharedString, glyph: usize, view: Entity<GlyphView> },
    /// The sample text.
    Sample { view: Entity<SampleView> },
}

impl Page {
    fn title(&self) -> SharedString {
        match self {
            Page::Pdf { title, .. } | Page::Glyph { title, .. } => title.clone(),
            Page::Sample { .. } => "样张".into(),
        }
    }
}

pub struct Workspace {
    focus: FocusHandle,
    document: Entity<Document>,
    /// The glyphs' outlines, for the grid and the sample text.
    outlines: Entity<Outlines>,
    font_view: Entity<FontView>,
    pages: Vec<Page>,
    active: usize,
    menu_bar: Entity<AppMenuBar>,
    /// The font being exported, on a background thread.
    exporting: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// `start`: a project folder to open and PDFs to import, in any order.
    pub fn new(start: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let document = cx.new(|_| Document::new(Project::new("我的手写体"), None));
        let outlines = cx.new(|cx| Outlines::new(document.clone(), cx));
        let font_view = cx.new(|cx| FontView::new(document.clone(), outlines.clone(), window, cx));
        let subscriptions = vec![
            cx.observe_in(&document, window, |this, _, window, cx| this.update_title(window, cx)),
            cx.subscribe_in(&document, window, |this, _, event: &DocumentEvent, window, cx| {
                if let DocumentEvent::Saved = event {
                    this.update_title(window, cx);
                }
            }),
            cx.subscribe_in(&document, window, |this, _, event: &DocumentEvent, _, cx| {
                if matches!(event, DocumentEvent::Glyph(_) | DocumentEvent::GlyphList | DocumentEvent::Saved) {
                    this.refresh_pages(cx);
                }
            }),
            cx.subscribe_in(&font_view, window, |this, _, event: &FontViewEvent, window, cx| match event {
                FontViewEvent::Selected(_) => this.refresh_pages(cx),
                FontViewEvent::Open(glyph) => this.open_glyph(*glyph, window, cx),
                FontViewEvent::OpenSample => this.open_sample(window, cx),
            }),
        ];
        let mut this = Workspace {
            focus: cx.focus_handle(),
            document,
            outlines,
            font_view,
            pages: Vec::new(),
            active: 0,
            menu_bar: AppMenuBar::new(cx),
            exporting: None,
            _subscriptions: subscriptions,
        };
        let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) = start.into_iter().partition(|p| p.join("project.toml").is_file());
        if let Some(folder) = folders.into_iter().next() {
            this.open_folder(folder, window, cx);
        }
        for p in files {
            if p.is_file() {
                this.import_pdf(&p, window, cx);
            } else {
                window.push_notification(Notification::error(format!("打不开 {}：不是工程文件夹，也不是 PDF", p.display())), cx);
            }
        }
        this.update_title(window, cx);
        this
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub fn document(&self) -> &Entity<Document> {
        &self.document
    }

    pub fn font_view(&self) -> &Entity<FontView> {
        &self.font_view
    }

    pub fn outlines(&self) -> &Entity<Outlines> {
        &self.outlines
    }

    /// The viewer of the PDF in tab `index`.
    pub fn pdf_view(&self, index: usize) -> Option<Entity<Viewer>> {
        match self.pages.get(index)? {
            Page::Pdf { view, .. } => Some(view.clone()),
            _ => None,
        }
    }

    /// The sample text, if open in a tab.
    pub fn sample_view(&self) -> Option<Entity<SampleView>> {
        self.pages.iter().find_map(|p| match p {
            Page::Sample { view } => Some(view.clone()),
            _ => None,
        })
    }

    /// Shows the sample text in its tab, opened if needed.
    pub fn open_sample(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active = match self.pages.iter().position(|p| matches!(p, Page::Sample { .. })) {
            Some(i) => i,
            None => {
                let view = cx.new(|cx| SampleView::new(self.document.clone(), self.outlines.clone(), window, cx));
                self._subscriptions.push(cx.subscribe_in(&view, window, |this, _, event: &SampleEvent, window, cx| match event {
                    SampleEvent::Open(glyph) => this.open_glyph(*glyph, window, cx),
                }));
                self.pages.push(Page::Sample { view });
                self.pages.len() - 1
            }
        };
        cx.notify();
    }

    /// The view of a glyph, if open in a tab.
    pub fn glyph_view(&self, glyph: usize) -> Option<Entity<GlyphView>> {
        self.pages.iter().find_map(|p| match p {
            Page::Glyph { glyph: g, view, .. } if *g == glyph => Some(view.clone()),
            _ => None,
        })
    }

    /// The tab shown.
    pub fn active_tab(&self) -> Option<usize> {
        (!self.pages.is_empty()).then_some(self.active)
    }

    /// Shows a glyph in its tab, opened if needed.
    pub fn open_glyph(&mut self, glyph: usize, window: &mut Window, cx: &mut Context<Self>) {
        let existing = self.pages.iter().position(|p| matches!(p, Page::Glyph { glyph: g, .. } if *g == glyph));
        let index = match existing {
            Some(i) => i,
            None => {
                let Some(g) = self.document.read(cx).project.glyphs.get(glyph) else { return };
                let chars: String = g.chars.iter().collect();
                let title: SharedString = if chars.is_empty() { g.name.clone().into() } else { format!("「{chars}」").into() };
                let view = cx.new(|cx| GlyphView::new(self.document.clone(), glyph, window, cx));
                self.pages.push(Page::Glyph { title, glyph, view });
                self.pages.len() - 1
            }
        };
        self.active = index;
        if let Some(Page::Glyph { view, .. }) = self.pages.get(index) {
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.pages.len() {
            return;
        }
        self.pages.remove(index);
        if index < self.active || self.active >= self.pages.len() {
            self.active = self.active.saturating_sub(1);
        }
        cx.notify();
    }

    fn update_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(&format!("{} — Scriptoria", self.document.read(cx).title()));
        cx.notify();
    }

    /// Replaces the open project (keeping the same document entity, so the views stay attached).
    fn replace(&mut self, project: Project, folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        self.document.update(cx, |d, cx| {
            *d = Document::new(project, folder);
            cx.emit(DocumentEvent::GlyphList);
            cx.emit(DocumentEvent::Saved);
            cx.notify();
        });
        self.pages.clear();
        self.active = 0;
        self.update_title(window, cx);
    }

    fn open_folder(&mut self, folder: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match Project::load(&folder) {
            Ok(project) => self.replace(project, Some(folder), window, cx),
            Err(e) => window.push_notification(Notification::error(format!("打开工程失败：{e}")), cx),
        }
    }

    fn import_pdf(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        // Sources are kept by absolute path, so the same file is the same source however it was named.
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let path = absolute.as_path();
        let opened = std::fs::read(path).map_err(anyhow::Error::from).and_then(|data| InkDocument::open(data).map_err(Into::into));
        match opened {
            Ok(ink) => {
                let title: SharedString = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into();
                let view = cx.new(|cx| Viewer::new(ink, title.clone(), cx));
                let source = self.document.update(cx, |d, cx| {
                    cx.notify();
                    d.project.source_for(path)
                });
                let id = source.clone();
                self._subscriptions.push(cx.subscribe_in(&view, window, move |this, _, event: &ViewerEvent, window, cx| match event {
                    ViewerEvent::Assign { page, strokes } => this.assign(&id, *page, strokes, window, cx),
                }));
                self.pages.push(Page::Pdf { title, source, view });
                self.active = self.pages.len() - 1;
                self.refresh_pages(cx);
                cx.notify();
            }
            Err(e) => window.push_notification(Notification::error(format!("导入 {} 失败：{e}", path.display())), cx),
        }
    }

    /// Puts strokes into the glyph selected in the grid, then moves the selection on to the next
    /// glyph still without strokes.
    fn assign(&mut self, source: &str, page: usize, strokes: &[scriptoria_ink::Stroke], window: &mut Window, cx: &mut Context<Self>) {
        let Some(glyph) = self.font_view.read(cx).selected() else {
            window.push_notification(Notification::warning("先在左边的字形表里选一个字"), cx);
            return;
        };
        self.document.update(cx, |d, cx| d.assign(glyph, source, page, strokes, cx));
        self.font_view.update(cx, |f, cx| f.select_next_empty(cx));
    }

    /// Tells the pages which of their strokes are used and which glyph a selection would go to.
    fn refresh_pages(&mut self, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let target: Option<SharedString> =
            self.font_view.read(cx).selected().and_then(|g| doc.project.glyphs.get(g)).map(|g| g.chars.iter().collect::<String>().into());
        let updates: Vec<_> = self
            .pages
            .iter()
            .filter_map(|p| match p {
                Page::Pdf { source, view, .. } => Some((view.clone(), doc.used_strokes(source))),
                _ => None,
            })
            .collect();
        for (view, used) in updates {
            let target = target.clone();
            view.update(cx, |v, cx| {
                v.set_assigned(used, cx);
                v.set_target(target, cx);
            });
        }
    }

    fn new_project(&mut self, _: &NewProject, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.read(cx).modified {
            window.push_notification(Notification::warning("当前工程有未保存的修改：已新建，旧工程未保存"), cx);
        }
        self.replace(Project::new("我的手写体"), None, window, cx);
    }

    fn open_project(&mut self, _: &OpenProject, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("打开工程".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(folder) = paths.into_iter().next()
            {
                this.update_in(cx, |this, window, cx| this.open_folder(folder, window, cx)).ok();
            }
        })
        .detach();
    }

    fn save_project(&mut self, _: &SaveProject, window: &mut Window, cx: &mut Context<Self>) {
        match self.document.read(cx).folder.clone() {
            Some(folder) => self.save_to(&folder, window, cx),
            None => self.save_project_as(&SaveProjectAs, window, cx),
        }
    }

    fn save_project_as(&mut self, _: &SaveProjectAs, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let start = doc.folder.as_ref().and_then(|f| f.parent().map(Path::to_path_buf)).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let name = doc.project.font.family.clone();
        let path = cx.prompt_for_new_path(&start, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(folder))) = path.await {
                this.update_in(cx, |this, window, cx| this.save_to(&folder, window, cx)).ok();
            }
        })
        .detach();
    }

    fn save_to(&mut self, folder: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match self.document.update(cx, |d, cx| d.save(folder, cx)) {
            Ok(()) => window.push_notification(Notification::success(format!("已保存到 {}", folder.display())), cx),
            Err(e) => window.push_notification(Notification::error(format!("保存失败：{e}")), cx),
        }
    }

    fn import(&mut self, _: &ImportPdf, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: Some("导入 PDF".into()) });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                this.update_in(cx, |this, window, cx| {
                    for p in paths {
                        this.import_pdf(&p, window, cx);
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn export_font(&mut self, _: &ExportFont, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        if doc.project.glyphs.iter().all(|g| g.strokes.is_empty()) {
            window.push_notification(Notification::warning("还没有写好的字：先在手写稿上选笔画，分配给字"), cx);
            return;
        }
        let start = doc.folder.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let name = format!("{}.ttf", doc.project.font.family);
        let path = cx.prompt_for_new_path(&start, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = path.await {
                this.update_in(cx, |this, window, cx| this.export_to(path, true, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Whether a font is being exported.
    pub fn exporting(&self) -> bool {
        self.exporting.is_some()
    }

    /// Writes the project's font to `path` and a page to try it in next to it (`.html`), on a
    /// background thread; then opens the page in the browser, if asked.
    pub fn export_to(&mut self, path: PathBuf, open_preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.exporting.is_some() {
            window.push_notification(Notification::warning("上一次导出还没完成"), cx);
            return;
        }
        let project = self.document.read(cx).project.clone();
        let n = project.glyphs.iter().filter(|g| !g.strokes.is_empty()).count();
        window.push_notification(Notification::info(format!("正在导出 {n} 个字形…")), cx);
        let background = cx.background_executor().clone();
        self.exporting = Some(cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let result = background.spawn(async move { crate::font::write_font(&project, &target) }).await;
            this.update_in(cx, |this, window, cx| {
                this.exporting = None;
                match result {
                    Ok(w) => {
                        let mut text = format!("已导出 {} 个字形到 {}（{} KB，每字 {:.1} 个母版）", w.glyphs, path.display(), w.bytes.div_ceil(1024), w.masters_per_glyph);
                        if !w.empty.is_empty() {
                            text += &format!("；{} 个字还没有笔画，没有导出", w.empty.len());
                        }
                        window.push_notification(Notification::success(text), cx);
                        if open_preview {
                            cx.open_with_system(&w.preview);
                        }
                    }
                    Err(e) => window.push_notification(Notification::error(format!("导出失败：{e}")), cx),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if !self.document.update(cx, |d, cx| d.undo(cx)) {
            window.push_notification(Notification::info("没有可以撤销的修改"), cx);
        }
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if !self.document.update(cx, |d, cx| d.redo(cx)) {
            window.push_notification(Notification::info("没有可以重做的修改"), cx);
        }
    }

    fn render_pages(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        if self.pages.is_empty() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child("还没有导入手写稿")
                .child("文件 → 导入 PDF…（Ctrl+I）")
                .into_any_element();
        }
        let tabs = TabBar::new("pages")
            .children(self.pages.iter().enumerate().map(|(i, p)| {
                let close = Button::new(("close-tab", i)).ghost().xsmall().label("×").on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.close_tab(i, cx);
                }));
                Tab::new().label(p.title()).suffix(close)
            }))
            .selected_index(self.active)
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                if *index < this.pages.len() {
                    this.active = *index;
                    cx.notify();
                }
            }));
        let content = match &self.pages[self.active.min(self.pages.len() - 1)] {
            Page::Pdf { view, .. } => view.clone().into_any_element(),
            Page::Glyph { view, .. } => view.clone().into_any_element(),
            Page::Sample { view } => view.clone().into_any_element(),
        };
        v_flex().size_full().child(tabs).child(div().flex_1().min_h_0().child(content)).into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::open_project))
            .on_action(cx.listener(Self::save_project))
            .on_action(cx.listener(Self::save_project_as))
            .on_action(cx.listener(Self::import))
            .on_action(cx.listener(Self::export_font))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &ShowSample, window, cx| this.open_sample(window, cx)))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child(self.menu_bar.clone()))
            .child(
                h_resizable("main")
                    .child(resizable_panel().size(px(380.0)).child(self.font_view.clone()))
                    .child(resizable_panel().child(self.render_pages(cx))),
            )
    }
}
