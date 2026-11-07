//! Scriptoria: makes handwriting fonts from Goodnotes PDFs.
//!
//! `scriptoria [project folder | PDF]`

// GPUI comes through gpui-kit (with its platform layer and GPUI Component); under this name the
// `gpui::` paths read as usual.
extern crate gpui_kit as gpui;

mod document;
mod font_view;
mod mesh;
mod viewer;
mod workspace;

use std::path::PathBuf;

use gpui::component::TitleBar;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    let start = std::env::args_os().nth(1).map(PathBuf::from);
    gpui::application().with_assets(gpui::assets::Assets).run(move |cx: &mut App| {
        gpui::init(cx);
        workspace::init(cx);
        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..TitleBar::window_options() };
        gpui::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| workspace::Workspace::new(start.clone(), window, cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        })
        .expect("cannot open the window");
        cx.activate(true);
    });
}
