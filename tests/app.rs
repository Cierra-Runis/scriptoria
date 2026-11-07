//! The application driven like a user would, in a headless window: typing characters into the
//! glyph grid, selecting strokes on a page of `test/test.pdf`, assigning them, editing a glyph.

use std::path::PathBuf;

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, point, px};
use scriptoria::app::glyph_view::GlyphView;
use scriptoria::app::{self, Workspace};
use scriptoria_ink::kurbo::{Point, Rect, Vec2};
use scriptoria_ink::{InkDocument, Stroke};
use scriptoria_project::Glyph;

fn test_pdf() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.pdf")
}

/// Where 世 is written, on page 3 (index 2) of `test/test.pdf`.
const SHI: Rect = Rect::new(212.9, 187.8, 217.9, 192.2);

/// The strokes of 世, in writing order.
fn shi_strokes() -> Vec<Stroke> {
    let doc = InkDocument::open(std::fs::read(test_pdf()).unwrap()).unwrap();
    doc.page(2).unwrap().strokes.into_iter().filter(|s| SHI.contains(s.bbox.center())).collect()
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
    let (from, to) = viewer.read_with(cx, |v, _| (v.page_to_window(Point::new(SHI.x0, SHI.y0)), v.page_to_window(Point::new(SHI.x1, SHI.y1))));
    let (from, to) = (from.expect("laid out"), to.expect("laid out"));
    cx.update(|window, cx| {
        window.drag(from, point(to.x, to.y), cx);
        window.press("enter", cx);
    });

    let expected: Vec<usize> = shi_strokes().iter().map(|s| s.order).collect();
    assert!(expected.len() >= 4, "the character's strokes: {expected:?}");
    let got: Vec<(usize, usize)> = workspace.read_with(cx, |w, cx| {
        w.document().read(cx).project.glyphs[0].strokes.iter().map(|s| s.origin.as_ref().map(|o| (o.page, o.index)).unwrap()).collect()
    });
    assert_eq!(got, expected.iter().map(|&i| (2, i)).collect::<Vec<_>>());
    // The page now marks them as used.
    let marked = viewer.read_with(cx, |v, _| expected.iter().all(|&i| v.assigned().contains(&(2, i))));
    assert!(marked);
}

/// The main window with 世 written into its first glyph, the glyph opened by a double click.
fn open_shi(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<GlyphView>, &mut VisualTestContext) {
    let (workspace, cx) = open(vec![], cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| {
        d.add_chars("世", cx);
        d.assign(0, "s1", 2, &shi_strokes(), cx);
    });
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.double_click(("cell", 0usize), cx);
    });
    let view = workspace.read_with(cx, |w, _| w.glyph_view(0)).expect("the glyph opens in a tab");
    // Lets the outlines be planned (in the background), then lays the view out.
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    assert!(view.read_with(cx, |v, _| v.ready()));
    (workspace, view, cx)
}

fn glyph(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Glyph {
    workspace.read_with(cx, |w, cx| w.document().read(cx).project.glyphs[0].clone())
}

/// Where each stroke came from: its index on the page.
fn order(g: &Glyph) -> Vec<usize> {
    g.strokes.iter().map(|s| s.origin.as_ref().unwrap().index).collect()
}

/// The pen-down point of stroke `i` (font units).
fn start(g: &Glyph, i: usize) -> Point {
    let s = &g.strokes[i];
    s.placement * s.skeleton[0].circles[0].0
}

fn moved(a: &Glyph, b: &Glyph) -> Vec2 {
    b.bounds().unwrap().origin() - a.bounds().unwrap().origin()
}

#[gpui_kit::test]
fn double_clicking_a_glyph_opens_it(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    assert_eq!(view.read_with(cx, |v, _| v.glyph_index()), 0);
    assert_eq!(workspace.read_with(cx, |w, _| w.active_tab()), Some(0));
    // Opening it again shows the same tab.
    cx.update(|window, cx| window.double_click(("cell", 0usize), cx));
    let same = workspace.read_with(cx, |w, _| w.glyph_view(0)).unwrap();
    assert_eq!(same.entity_id(), view.entity_id());
}

#[gpui_kit::test]
fn the_whole_glyph_moves_with_keys_and_the_mouse(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    let before = glyph(&workspace, cx);
    let n = before.strokes.len();
    cx.update(|window, cx| {
        window.press("ctrl-a", cx);
        window.press("right", cx);
        window.press("shift-up", cx);
    });
    assert_eq!(view.read_with(cx, |v, _| v.selection()), (0..n).collect::<Vec<_>>());
    let after = glyph(&workspace, cx);
    assert_eq!(moved(&before, &after), Vec2::new(10.0, 100.0));

    // Dragging any selected stroke moves them all, by whole font units.
    let from = view.read_with(cx, |v, _| v.font_to_window(start(&after, 0))).expect("laid out");
    cx.update(|window, cx| window.drag(from, from + point(px(60.0), px(0.0)), cx));
    let dragged = glyph(&workspace, cx);
    let d = moved(&after, &dragged);
    let expected = view.read_with(cx, |v, _| {
        let (a, b) = (v.font_to_window(Point::ZERO).unwrap(), v.font_to_window(Point::new(1000.0, 0.0)).unwrap());
        60.0 / (f32::from(b.x - a.x) as f64 / 1000.0)
    });
    assert!((d.x - expected).abs() <= 1.0 && d.y == 0.0 && d.x.fract() == 0.0, "moved {d:?}, expected {expected}");

    // One undo per edit: the drag, then the two key presses.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyph(&workspace, cx), after);
    cx.update(|window, cx| {
        window.press("ctrl-z", cx);
        window.press("ctrl-z", cx);
    });
    assert_eq!(glyph(&workspace, cx), before);
    cx.update(|window, cx| window.press("ctrl-y", cx));
    assert_eq!(moved(&before, &glyph(&workspace, cx)), Vec2::new(10.0, 0.0));
}

#[gpui_kit::test]
fn strokes_are_deleted_and_reordered(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    let before = order(&glyph(&workspace, cx));

    // Select the second stroke in the list; move it first in the writing order.
    cx.update(|window, cx| {
        window.click(("stroke", 1usize), cx);
        window.press("alt-up", cx);
    });
    let mut expected = before.clone();
    expected.swap(0, 1);
    assert_eq!(order(&glyph(&workspace, cx)), expected);
    assert_eq!(view.read_with(cx, |v, _| v.selection()), [0]);

    // Delete it, then take that back.
    cx.update(|window, cx| window.press("delete", cx));
    assert_eq!(order(&glyph(&workspace, cx)), expected[1..]);
    assert!(view.read_with(cx, |v, _| v.selection()).is_empty());
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(order(&glyph(&workspace, cx)), expected);
}

#[gpui_kit::test]
fn the_advance_is_typed(cx: &mut TestAppContext) {
    let (workspace, _view, cx) = open_shi(cx);
    cx.update(|window, cx| {
        window.click("advance", cx);
        window.press("ctrl-a", cx);
        window.input("987.4", cx);
        window.press("enter", cx);
    });
    assert_eq!(glyph(&workspace, cx).advance, 987.0);
    // Keys typed into the box do not edit the strokes.
    let n = glyph(&workspace, cx).strokes.len();
    cx.update(|window, cx| window.press("backspace", cx));
    assert_eq!(glyph(&workspace, cx).strokes.len(), n);
}
