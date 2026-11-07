//! The main window: the menu bar, the glyph grid on the left, the opened documents in tabs on the
//! right. File operations (new, open, save, import) live here.

use std::path::{Path, PathBuf};

use gpui::component::menu::AppMenuBar;
use gpui::component::tab::{Tab, TabBar};
use gpui::component::{ActiveTheme as _, GlobalState, TitleBar, WindowExt as _, h_resizable, resizable_panel, v_flex};
use gpui::component::notification::Notification;
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, FocusHandle, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString, Subscription, Window,
    actions, div, px,
};
use scriptoria_ink::InkDocument;
use scriptoria_project::Project;

use crate::app::document::{Document, DocumentEvent};
use crate::app::font_view::{FontView, FontViewEvent};
use crate::app::viewer::{Viewer, ViewerEvent};

actions!(scriptoria, [NewProject, OpenProject, SaveProject, SaveProjectAs, ImportPdf, Quit]);

/// Key bindings and menus, once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-n", NewProject, None),
        KeyBinding::new("ctrl-o", OpenProject, None),
        KeyBinding::new("ctrl-s", SaveProject, None),
        KeyBinding::new("ctrl-shift-s", SaveProjectAs, None),
        KeyBinding::new("ctrl-i", ImportPdf, None),
        KeyBinding::new("ctrl-q", Quit, None),
    ]);
    GlobalState::global_mut(cx).set_app_menus(menus().into_iter().map(Menu::owned).collect());
    cx.set_menus(menus());
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn menus() -> Vec<Menu> {
    vec![Menu {
        name: "文件".into(),
        items: vec![
            MenuItem::action("新建工程", NewProject),
            MenuItem::action("打开工程…", OpenProject),
            MenuItem::separator(),
            MenuItem::action("保存", SaveProject),
            MenuItem::action("另存为…", SaveProjectAs),
            MenuItem::separator(),
            MenuItem::action("导入 PDF…", ImportPdf),
            MenuItem::separator(),
            MenuItem::action("退出", Quit),
        ],
        disabled: false,
    }]
}

/// A document open in a tab.
enum Page {
    /// A handwritten document, with the id of its source in the project.
    Pdf { title: SharedString, source: String, view: Entity<Viewer> },
}

impl Page {
    fn title(&self) -> SharedString {
        match self {
            Page::Pdf { title, .. } => title.clone(),
        }
    }
}

pub struct Workspace {
    focus: FocusHandle,
    document: Entity<Document>,
    font_view: Entity<FontView>,
    pages: Vec<Page>,
    active: usize,
    menu_bar: Entity<AppMenuBar>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// `start`: a project folder to open and PDFs to import, in any order.
    pub fn new(start: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let document = cx.new(|_| Document::new(Project::new("我的手写体"), None));
        let font_view = cx.new(|cx| FontView::new(document.clone(), window, cx));
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
                FontViewEvent::Open(_) => window.push_notification(Notification::info("单字视图在下一步加入"), cx),
            }),
        ];
        let mut this = Workspace {
            focus: cx.focus_handle(),
            document,
            font_view,
            pages: Vec::new(),
            active: 0,
            menu_bar: AppMenuBar::new(cx),
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

    /// The viewer of the PDF in tab `index`.
    pub fn pdf_view(&self, index: usize) -> Option<Entity<Viewer>> {
        match self.pages.get(index)? {
            Page::Pdf { view, .. } => Some(view.clone()),
        }
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
        let updates: Vec<_> = self.pages.iter().map(|p| match p {
            Page::Pdf { source, view, .. } => (view.clone(), doc.used_strokes(source)),
        }).collect();
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
            .children(self.pages.iter().map(|p| Tab::new().label(p.title())))
            .selected_index(self.active)
            .on_click(cx.listener(|this, index: &usize, _, cx| {
                this.active = *index;
                cx.notify();
            }));
        let content = match &self.pages[self.active.min(self.pages.len() - 1)] {
            Page::Pdf { view, .. } => view.clone().into_any_element(),
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
