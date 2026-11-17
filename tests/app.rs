//! The application driven like a user would, in a headless window: typing characters into the
//! glyph grid, selecting strokes on a page of `test/test.pdf`, assigning them, editing a glyph.

use std::path::PathBuf;

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, point, px};
use scriptoria::app::document::Target;
use scriptoria::app::glyph_view::GlyphView;
use scriptoria::app::{self, Workspace};
use scriptoria::groups::Group;
use scriptoria_ink::kurbo::{Affine, Point, Rect, Vec2};
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
        // As the app does: keys go to the main window until something else takes them.
        let focus = view.read(cx).focus_handle().clone();
        window.focus(&focus, cx);
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
    // The glyph has them, as written, in writing order.
    let got = order(&glyph(&workspace, cx));
    assert_eq!(got, expected);
    // The page now marks them (and only them) as used: their outlines are in a glyph.
    let marked: std::collections::BTreeSet<usize> = viewer.read_with(cx, |v, _| v.assigned().iter().copied().collect());
    assert_eq!(marked, expected.iter().copied().collect());
}

/// The main window with 世 written into its first glyph, the glyph opened by a double click.
fn open_shi(cx: &mut TestAppContext) -> (Entity<Workspace>, Entity<GlyphView>, &mut VisualTestContext) {
    let (workspace, cx) = open(vec![], cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| {
        d.add_chars("世", cx);
        d.assign(0, &shi_strokes(), cx);
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

/// Which of the page's strokes each stroke is (its index on page 3), known by its outline.
fn order(g: &Glyph) -> Vec<usize> {
    let page = shi_strokes();
    g.strokes.iter().map(|s| page.iter().find(|p| p.path == s.outline.path).expect("a stroke of the page").order).collect()
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

/// Collects the start of every contour of a drawn glyph.
#[derive(Default)]
struct Starts(Vec<Point>);

impl skrifa::outline::OutlinePen for Starts {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(Point::new(x as f64, y as f64));
    }
    fn line_to(&mut self, _: f32, _: f32) {}
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {}
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {}
    fn close(&mut self) {}
}

#[gpui_kit::test]
fn the_project_exports_as_a_font(cx: &mut TestAppContext) {
    use skrifa::MetadataProvider as _;

    let (workspace, _view, cx) = open_shi(cx);
    let dir = std::env::temp_dir().join(format!("scriptoria-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Hand.ttf");
    workspace.update_in(cx, |w, window, cx| w.export_to(path.clone(), false, window, cx));
    cx.run_until_parked();
    assert!(!workspace.read_with(cx, |w, _| w.exporting()));
    let bytes = std::fs::read(&path).expect("the font is written");
    assert!(dir.join("Hand.html").is_file(), "the preview page is written next to it");

    let font = skrifa::FontRef::new(&bytes).expect("a font");
    let axis = font.axes().get(0).expect("a weight axis");
    assert_eq!((axis.min_value(), axis.default_value(), axis.max_value()), (100.0, 400.0, 900.0));
    // At 900 the glyph is the strokes as written, placed as in the project: a contour per stroke
    // piece, in writing order, each starting where its stroke was put down.
    let g = glyph(&workspace, cx);
    let pen = workspace.read_with(cx, |w, cx| w.document().read(cx).project.font.pen_scale());
    let pieces: Vec<(Point, f64)> = g
        .strokes
        .iter()
        .flat_map(|s| s.skeleton.iter().map(move |p| (s.placement * p.circles[0].0, p.circles[0].1 * pen)))
        .collect();
    let gid = font.charmap().map('世').expect("世 is in the font");
    let location = font.axes().location([("wght", 900.0)]);
    let mut starts = Starts::default();
    let settings = skrifa::outline::DrawSettings::unhinted(skrifa::instance::Size::unscaled(), &location);
    font.outline_glyphs().get(gid).unwrap().draw(settings, &mut starts).unwrap();
    assert_eq!(starts.0.len(), pieces.len());
    for (i, (start, (pen_down, r))) in starts.0.iter().zip(&pieces).enumerate() {
        assert!((*start - *pen_down).hypot() <= r + 2.0, "contour {i} starts at {start:?}, its stroke at {pen_down:?} (radius {r:.1})");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn the_weight_of_the_handwriting_is_set_with_a_slider(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    let written = |cx: &mut VisualTestContext| workspace.read_with(cx, |w, cx| w.document().read(cx).project.font.written_weight);
    assert_eq!(written(cx), 900.0);
    assert_eq!(view.read_with(cx, |v, _| v.weight_range()).1, 1.0);

    // Drag the slider from its right end (900) to the middle of its range (600).
    let bounds = cx.update(|window, _| window.find("written-weight").bounds());
    let y = bounds.center().y;
    let (right, middle) = (point(bounds.right() - px(2.0), y), point(bounds.center().x, y));
    cx.update(|window, cx| window.drag(right, middle, cx));
    let w = written(cx);
    assert!((500.0..=700.0).contains(&w), "written at {w}");

    // The glyph view now makes outlines bolder than written, up to 900.
    cx.run_until_parked();
    let (thinnest, boldest) = view.read_with(cx, |v, _| v.weight_range());
    assert!(thinnest == 0.3 && boldest > 1.2, "{thinnest} – {boldest}");
    assert!(view.read_with(cx, |v, _| v.ready()));

    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(written(cx), 900.0);
}

/// A reference font with one glyph, 世, its box `SHI_FRAME`, written to a file.
const SHI_FRAME: Rect = Rect::new(100.0, -50.0, 900.0, 750.0);

fn reference_font(dir: &std::path::Path) -> PathBuf {
    use scriptoria_font::{Axis, FontSource, Glyph as FontGlyph, build};
    let mut outline = scriptoria_ink::kurbo::BezPath::new();
    let r = SHI_FRAME;
    outline.move_to((r.x0, r.y0));
    outline.line_to((r.x0, r.y1));
    outline.line_to((r.x1, r.y1));
    outline.line_to((r.x1, r.y0));
    outline.close_path();
    let glyph = FontGlyph { name: "uni4E16".into(), chars: vec!['世'], advance: 1000.0, masters: [100.0, 400.0, 900.0].map(|w| (w, outline.clone())).to_vec() };
    let source = FontSource {
        family: "Reference".into(),
        units_per_em: 1000,
        ascender: 880,
        descender: -120,
        axis: Axis { min: 100.0, default: 400.0, max: 900.0 },
        instances: vec![],
        glyphs: vec![glyph],
        accuracy: 0.5,
        tolerance: 0.0,
        names: Default::default(),
        lines: Default::default(),
    };
    let file = dir.join("reference.ttf");
    std::fs::write(&file, build(&source).unwrap()).unwrap();
    file
}

#[gpui_kit::test]
fn new_glyphs_take_the_reference_place(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    let dir = std::env::temp_dir().join(format!("scriptoria-reference-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = reference_font(&dir);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| {
        d.set_reference(Some(scriptoria_project::Reference { file, face: 0 }), cx).unwrap();
        d.add_chars("世", cx);
        d.assign(0, &shi_strokes(), cx);
    });
    let placed = |g: &Glyph| g.ink_bounds().unwrap();
    let b = placed(&glyph(&workspace, cx));
    assert!((b.center() - SHI_FRAME.center()).hypot() < 0.5, "{b:?}");
    assert!((b.area() / SHI_FRAME.area() - 1.0).abs() < 0.01, "{b:?}");
    assert_eq!(glyph(&workspace, cx).advance, 1000.0);

    // Moved and shrunk, the glyph goes back with "place by reference", in one undo step.
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.double_click(("cell", 0usize), cx);
    });
    cx.run_until_parked();
    let fitted = glyph(&workspace, cx);
    document.update(cx, |d, cx| d.update_glyph(0, |g| g.transform_strokes(&[0, 1, 2, 3, 4], scriptoria_ink::kurbo::Affine::scale(0.5)), cx));
    cx.update(|window, cx| window.click("fit-reference", cx));
    assert!((placed(&glyph(&workspace, cx)).center() - SHI_FRAME.center()).hypot() < 0.5);
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_ne!(glyph(&workspace, cx), fitted);
    std::fs::remove_dir_all(&dir).ok();
}

/// The main window with 世 written into two glyphs (世 and 丗, the same strokes).
fn two_glyphs(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
    let (workspace, cx) = open(vec![], cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| {
        d.add_chars("世丗", cx);
        d.assign(0, &shi_strokes(), cx);
        d.assign(1, &shi_strokes(), cx);
    });
    cx.update(|window, cx| window.render_frame(cx));
    (workspace, cx)
}

fn glyphs(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<Glyph> {
    workspace.read_with(cx, |w, cx| w.document().read(cx).project.glyphs.clone())
}

#[gpui_kit::test]
fn selected_glyphs_change_together(cx: &mut TestAppContext) {
    let (workspace, cx) = two_glyphs(cx);
    let before = glyphs(&workspace, cx);
    cx.update(|window, cx| {
        window.click(("cell", 0usize), cx);
        window.click_with_modifiers(("cell", 1usize), gpui_kit::Modifiers::control(), cx);
    });
    assert_eq!(workspace.read_with(cx, |w, cx| w.font_view().read(cx).selection()), [0, 1]);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("full-width-selection", cx);
    });
    let after = glyphs(&workspace, cx);
    assert!(after.iter().zip(&before).all(|(a, b)| a != b && a.advance == 1000.0));
    // One undo takes both back.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyphs(&workspace, cx), before);
}

#[gpui_kit::test]
fn the_grid_and_the_sample_show_the_font(cx: &mut TestAppContext) {
    let (workspace, cx) = two_glyphs(cx);
    // The grid asks for the outlines; once planned they are there.
    cx.run_until_parked();
    let outlines = workspace.read_with(cx, |w, _| w.outlines().clone());
    assert!(outlines.read_with(cx, |o, _| o.idle()));
    assert!(outlines.update(cx, |o, cx| o.mesh(0, 400.0, cx)).is_some());

    // The sample text sets both glyphs; a click on the second opens it.
    cx.update(|window, cx| window.click("open-sample", cx));
    let sample = workspace.read_with(cx, |w, _| w.sample_view()).expect("the sample text opens");
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    let boxes = sample.read_with(cx, |s, _| s.glyph_boxes());
    assert_eq!(boxes.iter().map(|b| b.1).collect::<Vec<_>>(), [0, 1]);
    assert!(boxes[0].0.right() <= boxes[1].0.left() + px(0.5), "set one after another: {boxes:?}");
    let center = boxes[1].0.center();
    cx.update(|window, cx| window.drag(center, center, cx));
    assert!(workspace.read_with(cx, |w, _| w.glyph_view(1)).is_some());
}

/// A practice sheet "written on": its first page drawn as the sheet does (a box for each character
/// to write over), with `strokes` filled in black on top, each set of strokes moved into the square
/// of the cell given.
fn written_sheet(template: &scriptoria_project::Template, strokes: &[(usize, Vec<Stroke>)]) -> Vec<u8> {
    use scriptoria_ink::kurbo::{Affine, PathEl, Shape as _};
    use scriptoria_template::pdf_writer::{Content, Pdf, Rect as PdfRect, Ref};
    use scriptoria_template::{FAINT, Lines, Model, Sheet};
    let grid = &template.grid;
    let chars: Vec<char> = template.chars.chars().collect();
    let model = |_| Some(Model { outline: scriptoria_ink::kurbo::Rect::new(100.0, -50.0, 900.0, 800.0).to_path(0.1), advance: 1000.0 });
    let sheet = Sheet { grid: *grid, id: template.id, chars: &chars, models: &model, lines: Lines::default() };
    let mut c = Content::new();
    sheet.draw_page(&mut c, 0);
    c.set_fill_rgb(0.1, 0.1, 0.1);
    for (cell, strokes) in strokes {
        let square = grid.cell(*cell).square;
        let bounds = strokes.iter().map(|s| s.bbox).reduce(|a, b| a.union(b)).unwrap();
        // Written at three quarters of the square, in its middle; then y up for the PDF.
        let k = 0.75 * square.height() / bounds.width().max(bounds.height());
        let place = Affine::translate((0.0, grid.height))
            * Affine::scale_non_uniform(1.0, -1.0)
            * Affine::translate(square.center().to_vec2())
            * Affine::scale(k)
            * Affine::translate(-bounds.center().to_vec2());
        for s in strokes {
            for el in (place * s.path.clone()).elements() {
                match *el {
                    PathEl::MoveTo(p) => {
                        c.move_to(p.x as f32, p.y as f32);
                    }
                    PathEl::LineTo(p) => {
                        c.line_to(p.x as f32, p.y as f32);
                    }
                    PathEl::QuadTo(q, p) => {
                        c.cubic_to(q.x as f32, q.y as f32, q.x as f32, q.y as f32, p.x as f32, p.y as f32);
                    }
                    PathEl::CurveTo(a, b, p) => {
                        c.cubic_to(a.x as f32, a.y as f32, b.x as f32, b.y as f32, p.x as f32, p.y as f32);
                    }
                    PathEl::ClosePath => {
                        c.close_path();
                    }
                }
            }
            c.fill_nonzero();
        }
    }
    let (catalog, tree, page, contents, faint) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4), Ref::new(5));
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    scriptoria_template::write_faint_state(&mut pdf, faint);
    pdf.page(page)
        .parent(tree)
        .media_box(PdfRect::new(0.0, 0.0, grid.width as f32, grid.height as f32))
        .contents(contents)
        .resources()
        .ext_g_states()
        .pair(FAINT, faint);
    pdf.stream(contents, &c.finish());
    pdf.finish()
}

#[gpui_kit::test]
fn written_sheets_fill_their_glyphs(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    let dir = std::env::temp_dir().join(format!("scriptoria-sheet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| d.add_chars("世丗", cx));

    // A sheet for both characters, kept with the project.
    let sheet = dir.join("sheet.pdf");
    workspace.update_in(cx, |w, window, cx| w.make_template_to(sheet.clone(), &['世', '丗'], false, window, cx));
    assert!(sheet.is_file());
    let template = document.read_with(cx, |d, _| d.project.templates[0].clone());

    // 世 written in its cell, 丗 left blank; the page imported.
    let written = dir.join("written.pdf");
    std::fs::write(&written, written_sheet(&template, &[(0, shi_strokes())])).unwrap();
    workspace.update_in(cx, |w, window, cx| w.import_pdf(&written, window, cx));
    cx.run_until_parked();
    assert!(!workspace.read_with(cx, |w, _| w.reading_sheets()));
    let gs = glyphs(&workspace, cx);
    assert_eq!(gs[0].strokes.len(), shi_strokes().len(), "世 is filled from its cell");
    assert!(gs[1].strokes.is_empty(), "丗 was left blank");
    // The cell is the em: written at three quarters of the square, 世 is three quarters of the em,
    // in its middle; and its strokes keep the cell's pen.
    let b = gs[0].ink_bounds().unwrap();
    let em = scriptoria_ink::kurbo::Rect::new(0.0, -120.0, 1000.0, 880.0);
    assert!((b.width().max(b.height()) - 750.0).abs() < 30.0, "{b:?}");
    assert!((b.center() - em.center()).hypot() < 30.0, "{b:?}");
    // (The page size went through the PDF as f32.)
    assert!(gs[0].strokes.iter().all(|s| (s.pen * template.grid.cell / 1000.0 - 1.0).abs() < 1e-6));
    assert_eq!(gs[0].advance, 1000.0);
    // The sheet's own drawing is not handwriting: only 世's strokes count as used.
    let viewer = workspace.read_with(cx, |w, _| w.pdf_view(0)).unwrap();
    assert_eq!(viewer.read_with(cx, |v, _| v.assigned().len()), shi_strokes().len());

    // One undo takes the sheet's writing back.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(glyphs(&workspace, cx)[0].strokes.is_empty());
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn glyphs_are_grouped_and_removed_together(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    // Characters typed, and a range of code points.
    cx.update(|window, cx| {
        window.click("chars", cx);
        window.input("世あA U+3041-U+3043", cx);
        window.press("enter", cx);
    });
    let chars = |cx: &mut VisualTestContext| -> String { glyphs(&workspace, cx).iter().flat_map(|g| g.chars.clone()).collect() };
    assert_eq!(chars(cx), "世あAぁぃ");

    // The hiragana shown alone, then by code point.
    let font_view = workspace.read_with(cx, |w, _| w.font_view().clone());
    let groups = font_view.update(cx, |f, cx| f.groups(cx));
    let hiragana = groups.iter().position(|l| l.group == Group::Block(0x3040)).expect("a group for the hiragana");
    assert_eq!((groups[hiragana].name.as_str(), groups[hiragana].glyphs), ("平假名", 3));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("group", hiragana), cx);
    });
    assert_eq!(font_view.update(cx, |f, cx| f.glyphs(cx)), [1, 3, 4]);
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("by-code", cx);
    });
    assert_eq!(font_view.update(cx, |f, cx| f.glyphs(cx)), [3, 1, 4], "ぁ あ ぃ");

    // With A open in a tab, the hiragana removed at once: the tab still shows A.
    workspace.update_in(cx, |w, window, cx| w.open_glyph(2, window, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("select-all", cx);
        window.render_frame(cx);
        window.click("remove-selection", cx);
        window.render_frame(cx);
    });
    assert_eq!(chars(cx), "世A");
    assert_eq!(workspace.read_with(cx, |w, cx| w.glyph_view(1).map(|v| v.read(cx).glyph_index())), Some(1));
    // Its group gone, all glyphs are shown.
    assert_eq!(font_view.read_with(cx, |f, _| f.group()), Group::All);
    // One undo puts them back where they were.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(chars(cx), "世あAぁぃ");
    assert!(workspace.read_with(cx, |w, _| w.glyph_view(2)).is_some());

    // A practice sheet is a group too, and is forgotten (and remembered) like a change.
    let dir = std::env::temp_dir().join(format!("scriptoria-groups-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    workspace.update_in(cx, |w, window, cx| w.make_template_to(dir.join("sheet.pdf"), &['ぃ', '世'], false, window, cx));
    let groups = font_view.update(cx, |f, cx| f.groups(cx));
    let sheet = groups.iter().position(|l| l.group == Group::Template(1)).expect("a group for the sheet");
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("group", sheet), cx);
    });
    assert_eq!(font_view.update(cx, |f, cx| f.glyphs(cx)), [4, 0], "in the sheet's order");
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("remove-template", cx);
    });
    let templates = |cx: &mut VisualTestContext| workspace.read_with(cx, |w, cx| w.document().read(cx).project.templates.len());
    assert_eq!(templates(cx), 0);
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(templates(cx), 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[gpui_kit::test]
fn the_hands_size_is_typed_and_sheets_follow_it(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    let font = |cx: &mut VisualTestContext| workspace.read_with(cx, |w, cx| w.document().read(cx).project.font.clone());
    assert_eq!(font(cx).pt_per_em, scriptoria_project::DEFAULT_PT_PER_EM);
    cx.update(|window, cx| {
        window.click("pt-per-em", cx);
        window.press("ctrl-a", cx);
        window.input("2.8", cx);
        window.press("enter", cx);
    });
    assert_eq!(font(cx).pt_per_em, 2.8);
    // Not a size: the value stays.
    cx.update(|window, cx| {
        window.press("ctrl-a", cx);
        window.input("big", cx);
        window.press("enter", cx);
    });
    assert_eq!(font(cx).pt_per_em, 2.8);
    // A sheet's squares are the em at that size.
    let dir = std::env::temp_dir().join(format!("scriptoria-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    workspace.update_in(cx, |w, window, cx| w.make_template_to(dir.join("sheet.pdf"), &['口'], false, window, cx));
    let cell = workspace.read_with(cx, |w, cx| w.document().read(cx).project.templates[0].grid.cell);
    assert!((cell - 2.8).abs() < 1e-9, "{cell}");
    // Undo takes the size back (after the sheet).
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(font(cx).pt_per_em, scriptoria_project::DEFAULT_PT_PER_EM);
    std::fs::remove_dir_all(&dir).ok();
}

/// The linear part of a placement: [a, b, c, d] (x' = a x + c y, y' = b x + d y).
fn linear(s: &scriptoria_project::GlyphStroke) -> [f64; 4] {
    let [a, b, c, d, _, _] = s.placement.as_coeffs();
    [a, b, c, d]
}

/// `m` × `l` for linear parts.
fn times(m: [f64; 4], l: [f64; 4]) -> [f64; 4] {
    [m[0] * l[0] + m[2] * l[1], m[1] * l[0] + m[3] * l[1], m[0] * l[2] + m[2] * l[3], m[1] * l[2] + m[3] * l[3]]
}

fn close(a: [f64; 4], b: [f64; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6 * (1.0 + y.abs()))
}

#[gpui_kit::test]
fn strokes_turn_flip_and_stretch_keeping_their_width(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    let before = glyph(&workspace, cx);

    // One stroke turned a quarter counterclockwise; the others stay; pens stay.
    cx.update(|window, cx| {
        window.click(("stroke", 1usize), cx);
        window.click("turn-left", cx);
    });
    let turned = glyph(&workspace, cx);
    assert!(close(linear(&turned.strokes[1]), times([0.0, 1.0, -1.0, 0.0], linear(&before.strokes[1]))));
    assert_eq!(turned.strokes[0], before.strokes[0]);
    assert!(turned.strokes.iter().zip(&before.strokes).all(|(a, b)| a.pen == b.pen));
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyph(&workspace, cx), before);

    // The whole glyph flipped across: as wide as before, where it was.
    cx.update(|window, cx| {
        window.press("escape", cx);
        window.click("flip-across", cx);
    });
    let flipped = glyph(&workspace, cx);
    assert!(close(linear(&flipped.strokes[2]), times([-1.0, 0.0, 0.0, 1.0], linear(&before.strokes[2]))));
    // (Flipped about the middle of the outlines shown, which is near the ink's.)
    let (a, b) = (before.ink_bounds().unwrap(), flipped.ink_bounds().unwrap());
    assert!((a.width() - b.width()).abs() < 1e-6 && (a.center() - b.center()).hypot() < 10.0, "{a:?} {b:?}");
    cx.update(|window, cx| window.press("ctrl-z", cx));

    // Stretched across by half as much again, typed.
    cx.update(|window, cx| {
        window.click("scale-x", cx);
        window.press("ctrl-a", cx);
        window.input("150", cx);
        window.click("apply-transform", cx);
    });
    let stretched = glyph(&workspace, cx);
    assert!(close(linear(&stretched.strokes[0]), times([1.5, 0.0, 0.0, 1.0], linear(&before.strokes[0]))));
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyph(&workspace, cx), before);

    // With the mouse: everything selected, the right edge dragged out stretches across only...
    cx.update(|window, cx| {
        window.click(("stroke", 0usize), cx);
        window.press("ctrl-a", cx);
        window.render_frame(cx);
    });
    let (_, edges, turn) = view.read_with(cx, |v, cx| v.handles(cx)).expect("a selection box");
    cx.update(|window, cx| window.drag(edges[1], edges[1] + point(px(40.0), px(0.0)), cx));
    let wider = glyph(&workspace, cx);
    let k = linear(&wider.strokes[0])[0] / linear(&before.strokes[0])[0];
    assert!(k > 1.05 && close(linear(&wider.strokes[0]), times([k, 0.0, 0.0, 1.0], linear(&before.strokes[0]))), "{k}");
    // ...and the handle above turns it, sizes kept.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.drag(turn, turn + point(px(60.0), px(20.0)), cx);
    });
    let turned = glyph(&workspace, cx);
    let (l0, l1) = (linear(&before.strokes[0]), linear(&turned.strokes[0]));
    let det = |l: [f64; 4]| l[0] * l[3] - l[1] * l[2];
    assert!((det(l0) - det(l1)).abs() < 1e-6 * det(l0).abs() && !close(l0, l1), "{l0:?} {l1:?}");
    assert!(turned.strokes.iter().zip(&before.strokes).all(|(a, b)| a.pen == b.pen));

    // The outline's points, as the font will have them.
    cx.update(|window, cx| {
        window.click("nodes-quadratic", cx);
        window.render_frame(cx);
    });
    assert_eq!(view.read_with(cx, |v, _| v.nodes()), scriptoria::app::glyph_view::Nodes::Quadratic);
}

#[gpui_kit::test]
fn sets_of_characters_are_added_at_once(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    let count = |cx: &mut VisualTestContext| workspace.read_with(cx, |w, cx| w.document().read(cx).project.glyphs.len());
    // The printable ASCII characters, chosen in the dialog (filtered down to them first).
    let ascii = scriptoria_charsets::standards().iter().position(|s| s.name.starts_with("ASCII")).unwrap();
    cx.update(|window, cx| {
        window.click("add-charset", cx);
        window.render_frame(cx);
        window.click("charset-filter", cx);
        window.input("ascii", cx);
        window.render_frame(cx);
        window.click(("standard", ascii), cx);
        window.render_frame(cx);
        window.click("add-charsets", cx);
    });
    assert_eq!(count(cx), 94);
    let first = workspace.read_with(cx, |w, cx| w.document().read(cx).project.glyphs[0].chars.clone());
    assert_eq!(first, ['!']);
    // One step to undo.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(count(cx), 0);
}

#[gpui_kit::test]
fn skeletons_are_edited_circle_by_circle(cx: &mut TestAppContext) {
    use scriptoria::app::glyph_view::Mode;
    let (workspace, view, cx) = open_shi(cx);
    let before = glyph(&workspace, cx);
    let circles = |g: &Glyph| g.strokes[0].skeleton[0].circles.clone();
    cx.update(|window, cx| {
        window.click("mode-skeleton", cx);
        window.render_frame(cx);
    });
    assert_eq!(view.read_with(cx, |v, _| v.mode()), Mode::Skeleton);

    // A circle dragged: it moves (in the page's units), the others stay, the pen stays.
    let at = view.read_with(cx, |v, cx| v.circle_to_window((0, 0, 1), cx)).expect("laid out");
    cx.update(|window, cx| window.drag(at, at + point(px(30.0), px(0.0)), cx));
    let dragged = glyph(&workspace, cx);
    let (a, b) = (circles(&before), circles(&dragged));
    assert!((b[1].0 - a[1].0).hypot() > 0.0 && b[0] == a[0] && b[2] == a[2] && b[1].1 == a[1].1);
    assert_eq!(dragged.strokes[0].pen, before.strokes[0].pen);
    assert_eq!(view.read_with(cx, |v, _| v.selected_circles()), [(0, 0, 1)]);

    // Its radius made larger; then it is removed.
    cx.update(|window, cx| window.press("]", cx));
    let larger = glyph(&workspace, cx);
    assert!((circles(&larger)[1].1 / a[1].1 - 1.1).abs() < 1e-9);
    cx.update(|window, cx| window.press("delete", cx));
    assert_eq!(circles(&glyph(&workspace, cx)).len(), a.len() - 1);
    // Each one step back.
    cx.update(|window, cx| {
        window.press("ctrl-z", cx);
        window.press("ctrl-z", cx);
        window.press("ctrl-z", cx);
    });
    assert_eq!(glyph(&workspace, cx), before);

    // A double click on the centerline between the first two circles adds one there.
    let middle = {
        let s = &before.strokes[0];
        s.placement * scriptoria_ink::kurbo::ParamCurve::eval(&s.skeleton[0].spine[0], 0.5)
    };
    let at = view.read_with(cx, |v, _| v.font_to_window(middle)).unwrap();
    cx.update(|window, cx| {
        window.render_frame(cx);
        let canvas = window.find("glyph-canvas").bounds();
        window.click_with_options("glyph-canvas", gpui_kit::test::ClickOptions::new().with_offset(at - canvas.origin).with_count(2), cx);
    });
    assert_eq!(circles(&glyph(&workspace, cx)).len(), a.len() + 1);
    assert_eq!(view.read_with(cx, |v, _| v.selected_circles()), [(0, 0, 1)]);
    cx.update(|window, cx| window.press("ctrl-z", cx));

    // Smoothing evens the skeleton out (the ends stay).
    cx.update(|window, cx| {
        window.press("escape", cx);
        window.click("smooth-skeleton", cx);
    });
    let smooth = glyph(&workspace, cx);
    assert_ne!(smooth.strokes, before.strokes);
    assert_eq!(circles(&smooth)[0], a[0]);
    cx.update(|window, cx| window.press("ctrl-z", cx));

    // A whole stroke made thicker: its pen, nothing else.
    cx.update(|window, cx| {
        window.click("mode-strokes", cx);
        window.click(("stroke", 1usize), cx);
        window.click("pen-thicker", cx);
    });
    let thicker = glyph(&workspace, cx);
    assert!((thicker.strokes[1].pen / before.strokes[1].pen - 1.1).abs() < 1e-9);
    assert_eq!(thicker.strokes[1].skeleton, before.strokes[1].skeleton);
    assert_eq!(thicker.strokes[0], before.strokes[0]);
}

#[gpui_kit::test]
fn strokes_are_copied_and_kept_as_parts(cx: &mut TestAppContext) {
    let (workspace, view, cx) = open_shi(cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| d.add_chars("丗", cx));
    let shi = glyph(&workspace, cx);

    // Two strokes of 世 copied, pasted into 丗 where they were.
    cx.update(|window, cx| {
        window.click(("stroke", 0usize), cx);
        window.click_with_modifiers(("stroke", 2usize), gpui_kit::Modifiers::control(), cx);
        window.press("ctrl-c", cx);
    });
    workspace.update_in(cx, |w, window, cx| w.open_glyph(1, window, cx));
    let other = workspace.read_with(cx, |w, _| w.glyph_view(1)).expect("丗 opens");
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("ctrl-v", cx);
    });
    let pasted = glyphs(&workspace, cx)[1].clone();
    assert_eq!(pasted.strokes, [shi.strokes[0].clone(), shi.strokes[2].clone()]);
    assert_eq!(other.read_with(cx, |v, _| v.selection()), [0, 1], "the pasted strokes are selected");
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(glyphs(&workspace, cx)[1].strokes.is_empty());

    // In 世, its second stroke kept as a part, named.
    workspace.update_in(cx, |w, window, cx| w.open_glyph(0, window, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("stroke", 1usize), cx);
    });
    view.update(cx, |v, cx| v.show_parts(true, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("part-name", cx);
        window.input("竖", cx);
        window.click("save-part", cx);
    });
    let parts = document.read_with(cx, |d, _| d.project.parts.clone());
    assert_eq!(parts.len(), 1);
    assert_eq!((parts[0].name.as_str(), parts[0].glyph.strokes.clone()), ("竖", vec![shi.strokes[1].clone()]));
    // The stroke stays in 世, linked to the part.
    let link = glyphs(&workspace, cx)[0].strokes[1].link.expect("linked");
    assert_eq!((link.part, link.index, link.transform), (parts[0].id, 0, Affine::IDENTITY));

    // Put into 丗 with a click: where it was, linked.
    workspace.update_in(cx, |w, window, cx| w.open_glyph(1, window, cx));
    other.update(cx, |v, cx| v.show_parts(true, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("part", 0usize), cx);
    });
    let put = glyphs(&workspace, cx)[1].strokes.clone();
    assert_eq!(put.len(), 1);
    assert_eq!((put[0].placement, &put[0].skeleton), (shi.strokes[1].placement, &shi.strokes[1].skeleton));
    assert_eq!(put[0].link.map(|l| l.part), Some(parts[0].id));
    // Undo takes the insertion back, then the part (and the link in 世).
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(glyphs(&workspace, cx)[1].strokes.is_empty());
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(document.read_with(cx, |d, _| d.project.parts.is_empty()));
    assert_eq!(glyphs(&workspace, cx)[0], shi);
}

/// 世 written and opened, 丗 added; 世's second stroke kept as the part 竖 and put into 丗. Returns
/// the part's id.
fn shi_with_a_part(cx: &mut TestAppContext) -> (Entity<Workspace>, u32, &mut VisualTestContext) {
    let (workspace, _, cx) = open_shi(cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    let id = document.update(cx, |d, cx| {
        d.add_chars("丗", cx);
        let id = d.make_part(0, &[1], "竖", cx).unwrap();
        d.insert_part(1, id, Affine::IDENTITY, None, cx);
        id
    });
    (workspace, id, cx)
}

#[gpui_kit::test]
fn glyphs_follow_the_parts_they_use(cx: &mut TestAppContext) {
    let (workspace, id, cx) = shi_with_a_part(cx);
    let before = glyphs(&workspace, cx);
    // The part opened in a tab of its own, its stroke moved right.
    workspace.update_in(cx, |w, window, cx| w.open_part(id, window, cx));
    let part_view = workspace.read_with(cx, |w, _| w.target_view(Target::Part(id))).expect("the part opens in a tab");
    assert_eq!(part_view.read_with(cx, |v, _| v.target()), Target::Part(id));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("ctrl-a", cx);
        window.press("right", cx);
    });
    // Both glyphs using it follow; the strokes of their own stay.
    let after = glyphs(&workspace, cx);
    let x = |s: &scriptoria_project::GlyphStroke| s.placement.as_coeffs()[4];
    assert!((x(&after[0].strokes[1]) - x(&before[0].strokes[1]) - 10.0).abs() < 1e-9);
    assert!((x(&after[1].strokes[0]) - x(&before[1].strokes[0]) - 10.0).abs() < 1e-9);
    assert_eq!(after[0].strokes[0], before[0].strokes[0]);
    // One undo puts the part and both glyphs back.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyphs(&workspace, cx), before);
}

#[gpui_kit::test]
fn uses_of_parts_moved_whole_stay_linked_and_changed_ones_do_not(cx: &mut TestAppContext) {
    let (workspace, id, cx) = shi_with_a_part(cx);
    workspace.update_in(cx, |w, window, cx| w.open_glyph(1, window, cx));
    cx.run_until_parked();
    // 丗's use of the part moved as a whole: still the part's, moved.
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("stroke", 0usize), cx);
        window.press("right", cx);
    });
    let link = glyphs(&workspace, cx)[1].strokes[0].link.expect("still linked");
    assert_eq!(link.part, id);
    let moved = link.transform.as_coeffs();
    assert!(Affine::translate((10.0, 0.0)).as_coeffs().iter().zip(moved).all(|(a, b)| (a - b).abs() < 1e-6), "{moved:?}");
    // Made thicker, it is no longer the part's (世's use stays).
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("pen-thicker", cx);
    });
    let now = glyphs(&workspace, cx);
    assert!(now[1].strokes[0].link.is_none());
    assert!(now[0].strokes[1].link.is_some());
    // Undone, linked again.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(glyphs(&workspace, cx)[1].strokes[0].link.is_some());
}

/// A project of 河 (its strokes 世's), its first two strokes kept as the part 氵, where 氵 sits
/// in 河; and `more` added, empty.
fn he_with_water<'a>(more: &str, cx: &'a mut TestAppContext) -> (Entity<Workspace>, u32, &'a mut VisualTestContext) {
    let (workspace, cx) = open(vec![], cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    let id = document.update(cx, |d, cx| {
        d.add_chars("河", cx);
        d.assign(0, &shi_strokes(), cx);
        let id = d.make_part(0, &[0, 1], "氵", cx).unwrap();
        let region = scriptoria::components::region_in(scriptoria_ids::Table::builtin(), &d.project, &['河'], id);
        assert_eq!(region.map(|r| (r.x0, r.x1)), Some((0.0, 0.5)));
        d.update_project(&[], |p| p.part_mut(id).unwrap().region = region, cx);
        d.add_chars(more, cx);
        id
    });
    (workspace, id, cx)
}

#[gpui_kit::test]
fn parts_are_put_into_the_characters_that_have_them(cx: &mut TestAppContext) {
    let (workspace, id, cx) = he_with_water("湖海世", cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    cx.update(|window, cx| {
        scriptoria::app::part_uses::open(document.clone(), id, window, cx);
        window.render_frame(cx);
        window.click("put-part", cx);
    });
    let gs = glyphs(&workspace, cx);
    // 湖 and 海 (not 世) get 氵 where 河 has it: on the left, as it is there.
    for g in &gs[1..3] {
        assert_eq!(g.strokes.len(), 2, "{}", g.name);
        for (s, t) in g.strokes.iter().zip(&gs[0].strokes) {
            assert!(s.link.is_some_and(|l| l.part == id));
            assert!((s.placement.as_coeffs()[4] - t.placement.as_coeffs()[4]).abs() < 1e-6);
        }
    }
    assert!(gs[3].strokes.is_empty());
    // As one step.
    cx.update(|window, cx| window.click("close-part-uses", cx));
    document.update(cx, |d, cx| d.undo(cx));
    assert!(glyphs(&workspace, cx)[1..].iter().all(|g| g.strokes.is_empty()));
}

#[gpui_kit::test]
fn characters_are_put_together_from_parts(cx: &mut TestAppContext) {
    let (workspace, water, cx) = he_with_water("青清", cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    // 青 written (with 世's strokes), kept whole as a part.
    let qing = document.update(cx, |d, cx| {
        d.assign(1, &shi_strokes(), cx);
        let n = d.project.glyphs[1].strokes.len();
        let id = d.make_part(1, &(0..n).collect::<Vec<_>>(), "青", cx).unwrap();
        let region = scriptoria::components::region_in(scriptoria_ids::Table::builtin(), &d.project, &['青'], id);
        d.update_project(&[], |p| p.part_mut(id).unwrap().region = region, cx);
        id
    });
    cx.update(|window, cx| {
        scriptoria::app::part_uses::open(document.clone(), water, window, cx);
        window.render_frame(cx);
        window.click("compose-all", cx);
    });
    // 清 = 氵 + 青, in that order; 青 on the right half, smaller.
    let gs = glyphs(&workspace, cx);
    let parts: Vec<u32> = gs[2].strokes.iter().map(|s| s.link.expect("linked").part).collect();
    let n = gs[1].strokes.len();
    assert_eq!(parts, [vec![water; 2], vec![qing; n]].concat());
    let (right, whole) = (Glyph { strokes: gs[2].strokes[2..].to_vec(), ..gs[2].clone() }.ink_bounds().unwrap(), gs[1].ink_bounds().unwrap());
    assert!(right.width() < whole.width() && right.center().x > whole.center().x - 1.0);
}

#[gpui_kit::test]
fn the_fonts_settings_are_edited_in_one_place(cx: &mut TestAppContext) {
    let (workspace, cx) = open(vec![], cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    let before = document.read_with(cx, |d, _| d.project.font.clone());
    let retype = |window: &mut gpui_kit::Window, cx: &mut gpui_kit::App, id: &'static str, text: &str| {
        window.click(id, cx);
        window.press("ctrl-a", cx);
        window.input(text, cx);
    };
    cx.update(|window, cx| {
        scriptoria::app::font_info::open(document.clone(), window, cx);
        window.render_frame(cx);
        retype(window, cx, "info-family", "测试手写体");
        retype(window, cx, "info-english", "Test Hand");
        retype(window, cx, "info-line-gap", "100");
        retype(window, cx, "info-license", "OFL-1.1");
        window.click("font-info-ok", cx);
    });
    let after = document.read_with(cx, |d, _| d.project.font.clone());
    assert_eq!((after.family.as_str(), after.english_name.as_str(), after.line_gap, after.license.as_str()), ("测试手写体", "Test Hand", 100, "OFL-1.1"));
    assert_eq!(after.ascender, before.ascender);
    // The font is named so when exported.
    let names = document.read_with(cx, |d, _| scriptoria::font::export(&d.project).source.names);
    assert_eq!(names.english, "Test Hand");

    // Settings that make no sense are not taken.
    cx.update(|window, cx| {
        scriptoria::app::font_info::open(document.clone(), window, cx);
        window.render_frame(cx);
        retype(window, cx, "info-descender", "50");
        window.click("font-info-ok", cx);
    });
    assert_eq!(document.read_with(cx, |d, _| d.project.font.descender), before.descender);

    // One undo puts all back.
    cx.update(|window, cx| {
        window.click("font-info-cancel", cx);
        window.press("ctrl-z", cx);
    });
    assert_eq!(document.read_with(cx, |d, _| d.project.font.clone()), before);
}

#[gpui_kit::test]
fn glyphs_with_problems_are_found(cx: &mut TestAppContext) {
    use scriptoria::problems::Problem;
    let (workspace, cx) = two_glyphs(cx);
    let font_view = workspace.read_with(cx, |w, _| w.font_view().clone());
    let problem_groups = |cx: &mut VisualTestContext| -> Vec<Group> {
        font_view.update(cx, |f, cx| f.groups(cx)).into_iter().map(|l| l.group).filter(|g| matches!(g, Group::Problem(_))).collect()
    };
    assert!(!problem_groups(cx).contains(&Group::Problem(Problem::Duplicate)));
    // A stroke of 丗 there twice.
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    document.update(cx, |d, cx| d.update_glyph(1, |g| g.strokes.push(g.strokes[0].clone()), cx));
    assert!(problem_groups(cx).contains(&Group::Problem(Problem::Duplicate)));
    let at = font_view.update(cx, |f, cx| f.groups(cx)).iter().position(|l| l.group == Group::Problem(Problem::Duplicate)).unwrap();
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("group", at), cx);
    });
    assert_eq!(font_view.update(cx, |f, cx| f.glyphs(cx)), [1]);
    // Fixed (undone), the problem's group goes.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert!(!problem_groups(cx).contains(&Group::Problem(Problem::Duplicate)));
}

#[gpui_kit::test]
fn strokes_are_made_one_width(cx: &mut TestAppContext) {
    let (workspace, cx) = two_glyphs(cx);
    let document = workspace.read_with(cx, |w, _| w.document().clone());
    // 丗 written heavier: its pen doubled.
    document.update(cx, |d, cx| d.update_glyph(1, |g| g.strokes.iter_mut().for_each(|s| s.pen *= 2.0), cx));
    let before = glyphs(&workspace, cx);
    cx.update(|window, cx| {
        window.click(("cell", 0usize), cx);
        window.click_with_modifiers(("cell", 1usize), gpui_kit::Modifiers::control(), cx);
        window.render_frame(cx);
        window.click("even-widths", cx);
    });
    let after = glyphs(&workspace, cx);
    // Their strokes are nearer one width; nothing else changed.
    assert!(after.iter().zip(&before).all(|(a, b)| a.strokes.iter().zip(&b.strokes).all(|(s, t)| s.placement == t.placement && s.skeleton == t.skeleton)));
    let spread = |gs: &[Glyph]| {
        let w: Vec<f64> = gs.iter().flat_map(|g| g.strokes.iter().filter_map(|s| s.width())).collect();
        w.iter().cloned().fold(0.0, f64::max) / w.iter().cloned().fold(f64::INFINITY, f64::min)
    };
    assert!(spread(&after) < spread(&before), "{} < {}", spread(&after), spread(&before));
    // One step back.
    cx.update(|window, cx| window.press("ctrl-z", cx));
    assert_eq!(glyphs(&workspace, cx), before);
}
