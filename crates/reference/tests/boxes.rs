//! A font made here (with scriptoria-font) read as a reference: glyph boxes and advances, scaled to
//! another em.

use kurbo::{BezPath, Rect};
use scriptoria_font::{Axis, FontSource, Glyph, build};
use scriptoria_reference::{ReferenceFont, default_face};

/// A clockwise rectangle (y up).
fn rect(r: Rect) -> BezPath {
    let mut p = BezPath::new();
    p.move_to((r.x0, r.y0));
    p.line_to((r.x0, r.y1));
    p.line_to((r.x1, r.y1));
    p.line_to((r.x1, r.y0));
    p.close_path();
    p
}

fn font() -> Vec<u8> {
    let axis = Axis { min: 100.0, default: 400.0, max: 900.0 };
    let glyph = |name: &str, c: char, advance: f64, r: Rect| Glyph {
        name: name.into(),
        chars: vec![c],
        advance,
        masters: vec![(100.0, rect(r)), (400.0, rect(r)), (900.0, rect(r))],
    };
    let source = FontSource {
        family: "Reference Test".into(),
        units_per_em: 1000,
        ascender: 880,
        descender: -120,
        axis,
        instances: vec![("Regular".into(), 400.0)],
        glyphs: vec![glyph("A", 'A', 600.0, Rect::new(50.0, 0.0, 550.0, 700.0)), glyph("uni4E00", '一', 1000.0, Rect::new(60.0, 360.0, 940.0, 420.0))],
        accuracy: 0.5,
        tolerance: 0.0,
    };
    build(&source).expect("font")
}

#[test]
fn glyph_boxes_are_scaled_to_the_em_wanted() {
    let reference = ReferenceFont::from_bytes(font(), None).expect("a font");
    assert_eq!(reference.face().family, "Reference Test");
    let a = reference.glyph('A', 2000.0).expect("A");
    assert_eq!((a.bounds, a.advance), (Rect::new(100.0, 0.0, 1100.0, 1400.0), 1200.0));
    let one = reference.glyph('一', 1000.0).expect("一");
    assert_eq!((one.bounds, one.advance), (Rect::new(60.0, 360.0, 940.0, 420.0), 1000.0));
    // Characters the font lacks have no glyph.
    assert!(reference.glyph('B', 1000.0).is_none());
}

#[test]
fn a_single_font_has_one_face() {
    let reference = ReferenceFont::from_bytes(font(), Some(0)).expect("a font");
    assert_eq!(default_face(std::slice::from_ref(reference.face())), 0);
    assert!(ReferenceFont::from_bytes(font(), Some(1)).is_err());
    assert!(ReferenceFont::from_bytes(b"not a font".to_vec(), None).is_err());
}
