//! Projects saved to a folder and opened again.

use std::path::PathBuf;

use kurbo::{Affine, BezPath, CubicBez, Point, Rect};
use scriptoria_project::{
    Error, FORMAT, Glyph, GlyphStroke, Outline, OutlineKind, Part, Project, SkeletonPiece, fingerprint, initial_placement,
};

/// A fresh, empty folder under the system's temporary directory.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("scriptoria-project-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn stroke(x: f64, kind: OutlineKind) -> GlyphStroke {
    let mut path = BezPath::new();
    path.move_to((x, 1.0 / 3.0));
    path.curve_to((x + 0.1, 0.2), (x + 0.3, 0.25), (x + 0.4, 1.0));
    path.close_path();
    let (a, b, c) = (Point::new(x, 0.5), Point::new(x + 0.2, 0.6), Point::new(x + 0.4, 0.55));
    let piece = SkeletonPiece {
        circles: vec![(a, 0.1), (b, 0.12345678901234567), (c, 0.09)],
        spine: vec![CubicBez::new(a, Point::new(x + 0.05, 0.55), Point::new(x + 0.15, 0.6), b), CubicBez::new(b, Point::new(x + 0.25, 0.6), Point::new(x + 0.35, 0.56), c)],
    };
    let dot = SkeletonPiece { circles: vec![(Point::new(x, 2.0), 0.2)], spine: vec![] };
    GlyphStroke {
        outline: Outline { kind, path },
        skeleton: vec![piece, dot],
        placement: initial_placement(Rect::new(x, 0.0, x + 4.5, 4.5), &Default::default()) * Affine::rotate(0.01),
        pen: 222.25,
        link: None,
    }
}

fn sample() -> Project {
    let mut p = Project::new("Test Hand");
    p.add_chars("世A a");
    p.glyphs[0].strokes = vec![
        stroke(1.0, OutlineKind::Filled),
        stroke(2.0, OutlineKind::Line { width: 0.25 }),
    ];
    p.glyphs[1].strokes = vec![stroke(3.0, OutlineKind::Filled)];
    p.glyphs[1].advance = 612.5;
    // Parts, whatever their names: one kept from 世's strokes (linked there), with where it sits;
    // one of a stroke of its own, used in A, moved.
    let id = p.make_part(0, &[1], "氵").unwrap();
    p.part_mut(id).unwrap().region = Some(kurbo::Rect::new(0.0, 0.0, 0.5, 1.0));
    let other = p.next_part_id();
    p.parts.push(Part::new(other, "left / 左", vec![stroke(6.0, OutlineKind::Line { width: 0.3 })], 1000.0));
    p.insert_part(1, other, Affine::translate((12.5, -3.0)) * Affine::scale(0.75), Some(0));
    // A practice sheet: its id and grid come back too.
    let first = p.new_template(&['世', 'A']).id;
    assert_eq!(p.new_template(&['a']).id, first + 1);
    p
}

#[test]
fn projects_come_back_as_they_were_saved() {
    let dir = folder("round-trip");
    let project = sample();
    project.save(&dir).unwrap();
    let back = Project::load(&dir).unwrap();
    assert_eq!(back, project);
    // "A" and "a" differ only in case: two files, the empty "a" none.
    let files: Vec<String> = std::fs::read_dir(dir.join("glyphs")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(files.len(), 2, "{files:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn files_of_glyphs_without_strokes_are_removed() {
    let dir = folder("stale");
    let mut project = sample();
    project.save(&dir).unwrap();
    project.glyphs[1].strokes.clear();
    project.save(&dir).unwrap();
    assert_eq!(std::fs::read_dir(dir.join("glyphs")).unwrap().count(), 1);
    assert_eq!(Project::load(&dir).unwrap(), project);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn other_format_versions_are_refused() {
    let dir = folder("version");
    sample().save(&dir).unwrap();
    let toml = dir.join("project.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace(&format!("format = {FORMAT}"), "format = 99");
    std::fs::write(&toml, text).unwrap();
    assert!(matches!(Project::load(&dir), Err(Error::Format(99))));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn projects_from_before_the_thinnest_weight_open() {
    // Early projects gave three radius scales instead (thinnest, default, boldest).
    let dir = folder("thinnest");
    let mut p = sample();
    p.font.thinnest = 0.5;
    p.save(&dir).unwrap();
    let toml = dir.join("project.toml");
    let text = std::fs::read_to_string(&toml).unwrap().replace("thinnest = 0.5", "weight_scales = [0.3, 1.0, 3.0]");
    std::fs::write(&toml, text).unwrap();
    let font = Project::load(&dir).unwrap().font;
    assert_eq!((font.thinnest, font.written_weight), (0.3, 900.0));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_broken_glyph_file_names_itself() {
    let dir = folder("broken");
    sample().save(&dir).unwrap();
    let file = std::fs::read_dir(dir.join("glyphs")).unwrap().next().unwrap().unwrap().path();
    std::fs::write(&file, "<svg><g class=\"stroke\" transform=\"rotate(3)\"></g></svg>").unwrap();
    match Project::load(&dir) {
        Err(Error::Svg(path, message)) => {
            assert_eq!(path, file);
            assert!(message.contains("transform"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_glyphs_have_no_strokes() {
    let g = Glyph::for_char('x', &Default::default());
    assert!(g.bounds().is_none());
}

#[test]
fn projects_that_named_their_sources_open() {
    // Projects before 2026-11-08 noted the documents strokes came from, and where in them.
    let dir = folder("sources");
    let p = sample();
    p.save(&dir).unwrap();
    let toml = dir.join("project.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, text.replace("[[glyph]]", "[[source]]\nid = \"s1\"\nfile = 'notes/page.pdf'\n\n[[glyph]]")).unwrap();
    for entry in std::fs::read_dir(dir.join("glyphs")).unwrap() {
        let file = entry.unwrap().path();
        let svg = std::fs::read_to_string(&file).unwrap().replace("class=\"stroke\"", "class=\"stroke\" data-origin=\"s1 2 1617\"");
        std::fs::write(&file, svg).unwrap();
    }
    assert_eq!(Project::load(&dir).unwrap(), p);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn strokes_are_known_by_their_outlines() {
    let (a, b) = (stroke(1.0, OutlineKind::Filled), stroke(2.0, OutlineKind::Filled));
    assert_eq!(fingerprint(&a.outline.path), fingerprint(&a.clone().outline.path));
    assert_ne!(fingerprint(&a.outline.path), fingerprint(&b.outline.path));
    // Read back from a project, a stroke is the same stroke.
    let dir = folder("fingerprint");
    sample().save(&dir).unwrap();
    let back = Project::load(&dir).unwrap();
    assert_eq!(fingerprint(&back.glyphs[0].strokes[0].outline.path), fingerprint(&sample().glyphs[0].strokes[0].outline.path));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn parts_come_back_and_go() {
    let dir = folder("parts");
    let mut p = sample();
    p.save(&dir).unwrap();
    assert_eq!(Project::load(&dir).unwrap(), p, "parts and the links to them come back");
    assert_eq!(Project::load(&dir).unwrap().parts, p.parts);
    let files = |dir: &PathBuf| std::fs::read_dir(dir.join("parts")).map_or(0, |d| d.count());
    assert_eq!(files(&dir), 2);
    // A part no longer kept loses its file.
    p.parts.remove(0);
    p.save(&dir).unwrap();
    assert_eq!(files(&dir), 1);
    assert_eq!(Project::load(&dir).unwrap().parts, p.parts);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn parts_kept_before_they_had_ids_open() {
    // Projects before 2026-11-17 kept parts by name only: they are numbered in order.
    let dir = folder("old-parts");
    let mut p = sample();
    for g in &mut p.glyphs {
        g.strokes.iter_mut().for_each(|s| s.link = None);
    }
    p.parts.iter_mut().for_each(|part| part.region = None);
    p.save(&dir).unwrap();
    let toml = dir.join("project.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    // The fields the [[part]] tables have since, taken out.
    let mut in_part = false;
    let mut old = String::new();
    for l in text.lines() {
        if l.starts_with('[') {
            in_part = l == "[[part]]";
        }
        if !(in_part && (l.starts_with("id = ") || l.starts_with("component = "))) {
            old += l;
            old.push('\n');
        }
    }
    std::fs::write(&toml, old).unwrap();
    let back = Project::load(&dir).unwrap();
    assert_eq!(back.parts.iter().map(|p| (p.id, p.name.as_str())).collect::<Vec<_>>(), [(1, "氵"), (2, "left / 左")]);
    assert_eq!(back.parts[0].component, Some('氵'));
    std::fs::remove_dir_all(&dir).unwrap();
}
