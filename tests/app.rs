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
        d.assign(0, "s1", 2, &shi_strokes(), cx);
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
        d.assign(0, "s1", 2, &shi_strokes(), cx);
        d.assign(1, "s1", 2, &shi_strokes(), cx);
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

/// A practice sheet "written on": its first page drawn as the sheet does, with `strokes` filled in
/// black on top, each set of strokes moved into the square of the cell given.
fn written_sheet(template: &scriptoria_project::Template, strokes: &[(usize, Vec<Stroke>)]) -> Vec<u8> {
    use scriptoria_ink::kurbo::{Affine, PathEl};
    use scriptoria_template::pdf_writer::{Content, Pdf, Rect as PdfRect, Ref};
    let grid = &template.grid;
    let chars: Vec<char> = template.chars.chars().collect();
    let mut c = Content::new();
    scriptoria_template::draw_page(&mut c, grid, template.id, &chars, 0, &|_| None);
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
    let (catalog, tree, page, contents) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    pdf.page(page).parent(tree).media_box(PdfRect::new(0.0, 0.0, grid.width as f32, grid.height as f32)).contents(contents).resources();
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
