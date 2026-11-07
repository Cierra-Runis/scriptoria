mod mesh;
mod viewer;

use std::path::PathBuf;

use gpui::{
    App, AppContext, Application, Bounds, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};

use scriptoria_ink::InkDocument;
use crate::viewer::Viewer;

fn main() -> anyhow::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("test/test.pdf"));
    let data = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let doc = InkDocument::open(data)?;
    let file_name: SharedString = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .into();

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1200.0), px(820.0)), cx);
        let title = SharedString::from(format!("Scriptoria — {file_name}"));
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions { title: Some(title), ..Default::default() }),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Viewer::new(doc, file_name, cx));
                window.focus(view.read(cx).focus_handle());
                view
            },
        )
        .expect("cannot open the window");
        cx.activate(true);
    });
    Ok(())
}
