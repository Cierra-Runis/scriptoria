//! Scriptoria: makes handwriting fonts from Goodnotes PDFs.
//!
//! `scriptoria [project folder] [PDF…]`

// Release builds on Windows open no console window beside the app; debug builds keep it for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

extern crate gpui_kit as gpui;

use std::path::PathBuf;

use gpui::component::TitleBar;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use scriptoria::app;

fn main() {
    let start: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    gpui::application().with_assets(gpui::assets::Assets).run(move |cx: &mut App| {
        gpui::init(cx);
        app::init(cx);
        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..TitleBar::window_options() };
        gpui::open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| app::Workspace::new(start.clone(), window, cx));
            let focus = view.read(cx).focus_handle().clone();
            window.focus(&focus, cx);
            view
        })
        .expect("cannot open the window");
        cx.activate(true);
    });
}
