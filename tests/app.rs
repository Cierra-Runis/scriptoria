//! The application driven like a user would, in a headless window: typing characters into the
//! glyph grid, selecting strokes on a page of `test/test.pdf`, assigning them.

use std::path::PathBuf;

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, point};
use scriptoria::app::{self, Workspace};
use scriptoria_ink::kurbo::{Point, Rect};

fn test_pdf() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.pdf")
}

/// The main window, opened on `start`, inside the component library's root (for notifications).
fn open(start: Vec<PathBuf>, cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        app::init(cx);
    });
    let mut workspace = None;
    let (_, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| Workspace::new(start, window, cx));
        workspace = Some(view.clone());
        Root::new(view, window, cx)
    });
    cx.update(|window, cx| window.render_frame(cx));
    (workspace.unwrap(), cx)
}

#[gpui_kit::test]
fn typed_characters_become_glyphs(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    cx.update(|window, cx| {
        window.click("chars", cx);
        window.input("世界世 A", cx);
        window.press("enter", cx);
    });
    let names: Vec<String> = workspace.read_with(cx, |w, cx| w.document().read(cx).project.glyphs.iter().map(|g| g.name.clone()).collect());
    assert_eq!(names, ["uni4E16", "uni754C", "A"]);
    // The first new glyph is selected, ready for strokes.
    assert_eq!(workspace.read_with(cx, |w, cx| w.font_view().read(cx).selected()), Some(0));
}

#[gpui_kit::test]
fn strokes_boxed_on_a_page_go_into_the_selected_glyph(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![test_pdf()], cx);
    cx.update(|window, cx| {
        window.click("chars", cx);
        window.input("世", cx);
        window.press("enter", cx);
    });
    let viewer = workspace.read_with(cx, |w, _| w.pdf_view(0)).expect("the PDF is open");
    // 世 is written on page 3: focus the page, go there.
    cx.update(|window, cx| {
        window.click("page", cx);
        window.press("pagedown", cx);
        window.press("pagedown", cx);
    });
    assert_eq!(viewer.read_with(cx, |v, _| v.page_index()), 2);

    // Box the character, assign with Enter.
    let rect = Rect::new(212.9, 187.8, 217.9, 192.2);
    let (from, to) = viewer.read_with(cx, |v, _| (v.page_to_window(Point::new(rect.x0, rect.y0)), v.page_to_window(Point::new(rect.x1, rect.y1))));
    let (from, to) = (from.expect("laid out"), to.expect("laid out"));
    cx.update(|window, cx| {
        window.drag(from, point(to.x, to.y), cx);
        window.press("enter", cx);
    });

    let expected: Vec<usize> = {
        let doc = scriptoria_ink::InkDocument::open(std::fs::read(test_pdf()).unwrap()).unwrap();
        doc.page(2).unwrap().strokes.iter().filter(|s| rect.contains(s.bbox.center())).map(|s| s.order).collect()
    };
    assert!(expected.len() >= 4, "the character's strokes: {expected:?}");
    let got: Vec<(usize, usize)> = workspace.read_with(cx, |w, cx| {
        w.document().read(cx).project.glyphs[0].strokes.iter().map(|s| s.origin.as_ref().map(|o| (o.page, o.index)).unwrap()).collect()
    });
    assert_eq!(got, expected.iter().map(|&i| (2, i)).collect::<Vec<_>>());
    // The page now marks them as used.
    let marked = viewer.read_with(cx, |v, _| expected.iter().all(|&i| v.assigned().contains(&(2, i))));
    assert!(marked);
}
