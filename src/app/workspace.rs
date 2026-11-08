//! The main window: the menu bar, the glyph grid on the left, the opened documents and glyphs in
//! tabs on the right. File operations (new, open, save, import, export) and undo live here.

use std::path::{Path, PathBuf};

use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::AppMenuBar;
use gpui::component::notification::Notification;
use gpui::component::tab::{Tab, TabBar};
use gpui::component::{
    ActiveTheme as _, GlobalState, Sizable as _, TITLE_BAR_HEIGHT, TitleBar, WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui::TestSupportExt as _;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString, Subscription, Task, Window,
    actions, div, px,
};
use scriptoria_ink::{InkDocument, InkPage};
use scriptoria_project::Project;

use crate::app::document::{Document, DocumentEvent};
use crate::app::font_view::{FontView, FontViewEvent};
use crate::app::glyph_view::GlyphView;
use crate::app::outlines::Outlines;
use crate::app::sample_view::{SampleEvent, SampleView};
use crate::app::theme::{self, Appearance};
use crate::app::viewer::{Viewer, ViewerEvent};

actions!(
    scriptoria,
    [
        NewProject,
        OpenProject,
        SaveProject,
        SaveProjectAs,
        ImportPdf,
        ExportFont,
        FontInfo,
        MakeTemplate,
        Quit,
        Undo,
        Redo,
        ShowSample,
        FollowSystemAppearance,
        LightAppearance,
        DarkAppearance
    ]
);

/// Key bindings, menus and colors, once at startup.
pub fn init(cx: &mut App) {
    theme::init(cx);
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
    set_menus(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// The menus, for the window's menu bar and the system's.
fn set_menus(cx: &mut App) {
    let appearance = theme::appearance(cx);
    GlobalState::global_mut(cx).set_app_menus(menus(appearance).into_iter().map(Menu::owned).collect());
    cx.set_menus(menus(appearance));
}

fn menus(appearance: Appearance) -> Vec<Menu> {
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
                MenuItem::action("字体信息…", FontInfo),
                MenuItem::separator(),
                MenuItem::action("生成格子模板…", MakeTemplate),
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
        Menu {
            name: "视图".into(),
            items: vec![
                MenuItem::action("样张", ShowSample),
                MenuItem::separator(),
                MenuItem::submenu(Menu {
                    name: "外观".into(),
                    items: vec![
                        MenuItem::action("跟随系统", FollowSystemAppearance).checked(appearance == Appearance::System),
                        MenuItem::action("亮色（Catppuccin Latte）", LightAppearance).checked(appearance == Appearance::Light),
                        MenuItem::action("暗色（Catppuccin Mocha）", DarkAppearance).checked(appearance == Appearance::Dark),
                    ],
                    disabled: false,
                }),
            ],
            disabled: false,
        },
    ]
}

/// A document or glyph open in a tab.
enum Page {
    /// A handwritten document.
    Pdf { title: SharedString, view: Entity<Viewer> },
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
    /// Written practice sheets being read, on a background thread.
    reading: Option<Task<()>>,
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
                if let DocumentEvent::Reindexed(map) = event {
                    this.reindex(map, cx);
                }
                if matches!(event, DocumentEvent::Glyph(_) | DocumentEvent::GlyphList | DocumentEvent::Saved) {
                    this.refresh_pages(cx);
                }
            }),
            cx.subscribe_in(&font_view, window, |this, _, event: &FontViewEvent, window, cx| match event {
                FontViewEvent::Selected(_) => this.refresh_pages(cx),
                FontViewEvent::Open(glyph) => this.open_glyph(*glyph, window, cx),
                FontViewEvent::OpenSample => this.open_sample(window, cx),
                FontViewEvent::MakeTemplate(glyphs) => this.make_template_for(glyphs, window, cx),
                FontViewEvent::ExportTemplate(id) => this.export_template(*id, window, cx),
            }),
            // Light or dark as the system turns, unless one was picked.
            cx.observe_window_appearance(window, |_, window, cx| {
                if theme::appearance(cx) == Appearance::System {
                    theme::apply(window, cx);
                }
            }),
        ];
        theme::apply(window, cx);
        let mut this = Workspace {
            focus: cx.focus_handle(),
            document,
            outlines,
            font_view,
            pages: Vec::new(),
            active: 0,
            menu_bar: AppMenuBar::new(cx),
            exporting: None,
            reading: None,
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

    /// Follows glyphs added or removed: their tabs show the glyphs they showed; those of glyphs
    /// removed close.
    fn reindex(&mut self, map: &[Option<usize>], cx: &mut Context<Self>) {
        for i in (0..self.pages.len()).rev() {
            let Page::Glyph { glyph, .. } = &mut self.pages[i] else { continue };
            match map.get(*glyph).copied().flatten() {
                Some(g) => *glyph = g,
                None => self.close_tab(i, cx),
            }
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

    /// The window's title: the project's name, marked while it has unsaved changes.
    pub fn title(&self, cx: &App) -> String {
        format!("{} — Scriptoria", self.document.read(cx).title())
    }

    fn update_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(&self.title(cx));
        cx.notify();
    }

    /// Shows the window light, dark or as the system is, from now on.
    pub fn choose_appearance(&mut self, appearance: Appearance, window: &mut Window, cx: &mut Context<Self>) {
        theme::choose(appearance, window, cx);
        set_menus(cx);
        self.menu_bar.update(cx, |m, cx| m.reload(cx));
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

    /// Opens a handwritten PDF in a tab, to take strokes from (the project keeps no note of it).
    /// Pages written on the project's practice sheets are read at once: their cells' handwriting goes
    /// into the glyphs.
    pub fn import_pdf(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let opened = std::fs::read(path).map_err(anyhow::Error::from).and_then(|data| InkDocument::open(data).map_err(Into::into));
        match opened {
            Ok(ink) => {
                let sheets = self.sheet_pages(&ink, cx);
                let title: SharedString = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default().into();
                let view = cx.new(|cx| Viewer::new(ink, title.clone(), cx));
                self._subscriptions.push(cx.subscribe_in(&view, window, move |this, _, event: &ViewerEvent, window, cx| match event {
                    ViewerEvent::Assign { strokes } => this.assign(strokes, window, cx),
                }));
                self.pages.push(Page::Pdf { title, view });
                self.active = self.pages.len() - 1;
                self.refresh_pages(cx);
                if !sheets.is_empty() {
                    self.read_sheets(sheets, window, cx);
                }
                cx.notify();
            }
            Err(e) => window.push_notification(Notification::error(format!("导入 {} 失败：{e}", path.display())), cx),
        }
    }

    /// The pages of a document written on the project's practice sheets: (page, its strokes, the
    /// sheet's index in the project, the sheet's page).
    fn sheet_pages(&self, ink: &InkDocument, cx: &App) -> Vec<(usize, InkPage, usize, usize)> {
        let project = &self.document.read(cx).project;
        if project.templates.is_empty() {
            return Vec::new();
        }
        (0..ink.page_count())
            .filter_map(|i| {
                let page = ink.page(i)?;
                let (template, sheet_page) = crate::template::recognize(project, &page)?;
                Some((i, page, template, sheet_page))
            })
            .collect()
    }

    /// Whether written sheets are being read.
    pub fn reading_sheets(&self) -> bool {
        self.reading.is_some()
    }

    /// Reads written sheet pages on a background thread (skeletons take a while), then puts what
    /// was written into the glyphs, as one step to undo.
    fn read_sheets(&mut self, sheets: Vec<(usize, InkPage, usize, usize)>, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let (info, templates, reference) = (doc.project.font.clone(), doc.project.templates.clone(), doc.reference().cloned());
        window.push_notification(Notification::info(format!("认出 {} 页格子模板，正在读取…", sheets.len())), cx);
        let background = cx.background_executor().clone();
        self.reading = Some(cx.spawn_in(window, async move |this, cx| {
            let written = background
                .spawn(async move {
                    sheets
                        .iter()
                        .flat_map(|(_, ink, t, sheet_page)| crate::template::read_sheet_page(&info, &templates[*t], *sheet_page, ink, reference.as_deref()))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.reading = None;
                let n = written.len();
                let replaced = this.document.update(cx, |d, cx| d.write_cells(written, cx));
                let mut text = format!("从格子模板写入了 {n} 个字");
                if replaced > 0 {
                    text += &format!("，其中 {replaced} 个替换了原有的笔画（可撤销）");
                }
                window.push_notification(Notification::success(text), cx);
                this.refresh_pages(cx);
            })
            .ok();
        }));
    }

    fn make_template(&mut self, _: &MakeTemplate, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let selection = self.font_view.read(cx).selection();
        // The glyphs selected, or else those still without strokes.
        let glyphs: Vec<usize> = if selection.is_empty() {
            (0..doc.project.glyphs.len()).filter(|&g| doc.project.glyphs[g].strokes.is_empty()).collect()
        } else {
            selection
        };
        self.make_template_for(&glyphs, window, cx);
    }

    /// Asks where to write a practice sheet for glyphs, then writes it and opens it.
    fn make_template_for(&mut self, glyphs: &[usize], window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let chars: Vec<char> = glyphs.iter().filter_map(|&g| doc.project.glyphs.get(g)?.chars.first().copied()).collect();
        if chars.is_empty() {
            window.push_notification(Notification::warning("没有要写的字：先在左边加字，或选中要重写的字"), cx);
            return;
        }
        let start = doc.folder.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let name = format!("{} 格子模板.pdf", doc.project.font.family);
        let path = cx.prompt_for_new_path(&start, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = path.await {
                this.update_in(cx, |this, window, cx| this.make_template_to(path, &chars, true, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Asks where to write a practice sheet of the project's (by id) out again, then writes it.
    fn export_template(&mut self, id: u32, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let start = doc.folder.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_default();
        let name = format!("{} 格子模板 #{id}.pdf", doc.project.font.family);
        let path = cx.prompt_for_new_path(&start, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = path.await {
                this.update_in(cx, |this, window, cx| this.export_template_to(id, path, true, window, cx)).ok();
            }
        })
        .detach();
    }

    /// Writes a practice sheet of the project's (by id) to `path` again, as it was made (pages
    /// written on either copy are recognized); then opens it, if asked.
    pub fn export_template_to(&mut self, id: u32, path: PathBuf, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let Some(template) = doc.project.templates.iter().find(|t| t.id == id) else { return };
        let pdf = crate::template::sheet_pdf(&doc.project.font, template, doc.reference().map(|r| r.as_ref()));
        match std::fs::write(&path, pdf) {
            Ok(()) => {
                window.push_notification(Notification::success(format!("已导出格子模板 #{id} 到 {}", path.display())), cx);
                if open {
                    cx.open_with_system(&path);
                }
            }
            Err(e) => window.push_notification(Notification::error(format!("写不出 {}：{e}", path.display())), cx),
        }
    }

    /// Writes a practice sheet for characters to `path` (a PDF) and keeps it with the project;
    /// then opens it, if asked.
    pub fn make_template_to(&mut self, path: PathBuf, chars: &[char], open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let template = self.document.update(cx, |d, cx| d.new_template(chars, cx));
        let reference = self.document.read(cx).reference().cloned();
        let pdf = crate::template::sheet_pdf(&self.document.read(cx).project.font, &template, reference.as_deref());
        match std::fs::write(&path, pdf) {
            Ok(()) => {
                let pages = template.grid.pages(chars.len());
                window.push_notification(
                    Notification::success(format!("已生成格子模板：{} 个字，{pages} 页。写好后导入这个 PDF，字会自动归位", chars.len())),
                    cx,
                );
                if open {
                    cx.open_with_system(&path);
                }
            }
            Err(e) => {
                self.document.update(cx, |d, cx| d.discard_template(template.id, cx));
                window.push_notification(Notification::error(format!("写不出 {}：{e}", path.display())), cx);
            }
        }
    }

    /// Puts strokes into the glyph selected in the grid, then moves the selection on to the next
    /// glyph still without strokes.
    fn assign(&mut self, strokes: &[scriptoria_ink::Stroke], window: &mut Window, cx: &mut Context<Self>) {
        let Some(glyph) = self.font_view.read(cx).selected() else {
            window.push_notification(Notification::warning("先在左边的字形表里选一个字"), cx);
            return;
        };
        self.document.update(cx, |d, cx| d.assign(glyph, strokes, cx));
        self.font_view.update(cx, |f, cx| f.select_next_empty(cx));
    }

    /// Tells the pages which of their strokes are used and which glyph a selection would go to.
    fn refresh_pages(&mut self, cx: &mut Context<Self>) {
        let doc = self.document.read(cx);
        let target: Option<SharedString> =
            self.font_view.read(cx).selected().and_then(|g| doc.project.glyphs.get(g)).map(|g| g.chars.iter().collect::<String>().into());
        let used = doc.used_outlines();
        let views: Vec<_> = self
            .pages
            .iter()
            .filter_map(|p| match p {
                Page::Pdf { view, .. } => Some(view.clone()),
                _ => None,
            })
            .collect();
        for view in views {
            let (target, used) = (target.clone(), used.clone());
            view.update(cx, |v, cx| {
                v.set_used(used, cx);
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
            .on_action(cx.listener(Self::make_template))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(|this, _: &ShowSample, window, cx| this.open_sample(window, cx)))
            .on_action(cx.listener(|this, _: &FontInfo, window, cx| crate::app::font_info::open(this.document.clone(), window, cx)))
            .on_action(cx.listener(|this, _: &FollowSystemAppearance, window, cx| this.choose_appearance(Appearance::System, window, cx)))
            .on_action(cx.listener(|this, _: &LightAppearance, window, cx| this.choose_appearance(Appearance::Light, window, cx)))
            .on_action(cx.listener(|this, _: &DarkAppearance, window, cx| this.choose_appearance(Appearance::Dark, window, cx)))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                // The window has no frame of its own: its title is drawn over the middle of the title
                // bar (not taking the mouse, so the bar still drags the window).
                div().relative().flex_shrink_0().child(TitleBar::new().child(self.menu_bar.clone())).child(
                    h_flex()
                        .id("window-title")
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(TITLE_BAR_HEIGHT)
                        .justify_center()
                        .text_sm()
                        .child(div().child(self.document.read(cx).title()))
                        .child(div().text_color(theme.muted_foreground).child(" — Scriptoria"))
                        .test_support(),
                ),
            )
            .child(
                h_resizable("main")
                    .child(resizable_panel().size(px(560.0)).child(self.font_view.clone()))
                    .child(resizable_panel().child(self.render_pages(cx))),
            )
    }
}
