//! Parts found again in other characters, by what the characters are made of (ideographic
//! description sequences, see [`scriptoria_ids`]): the glyphs whose character has a part's
//! component, the part put into them where the component sits, and characters put together from
//! parts (清 from 氵 and 青).
//!
//! Where a component sits is a box of the em in fractions (x from the left, y from the bottom; the
//! em from the descender to the ascender). A part remembers its box in the character it was taken
//! from ([`scriptoria_project::Part::region`]); put into another box, it moves with the box and is
//! scaled by how much larger the box is (alike across and up, so that it keeps its shape).

use std::collections::BTreeSet;

use scriptoria_ids::{Composition, Table, same_component};
use scriptoria_ink::kurbo::{Affine, Rect};
use scriptoria_project::{FontInfo, Project};

/// The em, in font units: across the advance of a full-width character, up from the descender to
/// the ascender.
fn em(font: &FontInfo) -> Rect {
    Rect::new(0.0, font.descender as f64, font.units_per_em as f64, font.ascender as f64)
}

/// A box of the em in fractions, in font units.
pub fn from_em(r: Rect, font: &FontInfo) -> Rect {
    let e = em(font);
    Rect::new(e.x0 + r.x0 * e.width(), e.y0 + r.y0 * e.height(), e.x0 + r.x1 * e.width(), e.y0 + r.y1 * e.height())
}

/// A box in font units, in fractions of the em.
pub fn to_em(r: Rect, font: &FontInfo) -> Rect {
    let e = em(font);
    Rect::new((r.x0 - e.x0) / e.width(), (r.y0 - e.y0) / e.height(), (r.x1 - e.x0) / e.width(), (r.y1 - e.y0) / e.height())
}

/// The part whose component `c` is (by its form: 月 for ⺝), if any.
pub fn part_for(project: &Project, c: char) -> Option<u32> {
    project.parts.iter().find(|p| p.component.is_some_and(|pc| same_component(pc, c))).map(|p| p.id)
}

/// Where a part (just made from a glyph of `chars`, as its first use) sits in the first of them,
/// by its decomposition: of the places its component has there, the one nearest its ink.
pub fn region_in(table: &Table, project: &Project, chars: &[char], part: u32) -> Option<Rect> {
    let p = project.part(part)?;
    let (c, component) = (*chars.first()?, p.component?);
    let ink = to_em(p.glyph.ink_bounds()?, &project.font).center();
    table.occurrences(c, component).into_iter().min_by(|a, b| (a.center() - ink).hypot2().total_cmp(&(b.center() - ink).hypot2()))
}

/// Where a part goes to sit in a box of the em (fractions): its own box (where it sat in the
/// character it came from; else its ink) moved onto that one, scaled by the geometric mean of how
/// much wider and taller it is.
pub fn placement(project: &Project, part: u32, region: Rect) -> Option<Affine> {
    let p = project.part(part)?;
    let font = &project.font;
    let from = match p.region {
        Some(r) => from_em(r, font),
        None => p.glyph.ink_bounds()?,
    };
    let to = from_em(region, font);
    if from.width() <= 0.0 || from.height() <= 0.0 {
        return None;
    }
    let k = (to.width() / from.width() * to.height() / from.height()).sqrt();
    Some(Affine::translate(to.center().to_vec2()) * Affine::scale(k) * Affine::translate(-from.center().to_vec2()))
}

/// How a glyph stands to a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum UseKind {
    /// It uses the part already.
    Linked,
    /// It has strokes of its own (the part could replace some of them, by hand).
    Written,
    /// It has no strokes yet.
    Empty,
}

/// A glyph whose character has a part's component.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub glyph: usize,
    pub kind: UseKind,
    /// Where the component sits in it (boxes of the em, fractions).
    pub regions: Vec<Rect>,
}

/// The project's glyphs whose character has a part's component, in the project's order.
pub fn uses(table: &Table, project: &Project, part: u32) -> Vec<Use> {
    let Some(component) = project.part(part).and_then(|p| p.component) else { return Vec::new() };
    let linked: BTreeSet<usize> = project.glyphs_using(part).into_iter().collect();
    project
        .glyphs
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let c = *g.chars.first()?;
            let regions = if same_component(c, component) { Vec::new() } else { table.occurrences(c, component) };
            let kind = if linked.contains(&i) {
                UseKind::Linked
            } else if !g.strokes.is_empty() {
                UseKind::Written
            } else {
                UseKind::Empty
            };
            (!regions.is_empty() || kind == UseKind::Linked).then_some(Use { glyph: i, kind, regions })
        })
        .collect()
}

/// Characters among `chars` (in their order) that have a part's component, the project having no
/// glyph for them.
pub fn missing_with(table: &Table, project: &Project, part: u32, chars: &[char]) -> Vec<char> {
    let Some(component) = project.part(part).and_then(|p| p.component) else { return Vec::new() };
    chars.iter().copied().filter(|&c| project.glyph_for(c).is_none() && table.contains(c, component)).collect()
}

/// Puts a part into a glyph wherever its character has the part's component (品 gets 口 three
/// times); returns how many times.
pub fn put_into(table: &Table, project: &mut Project, part: u32, glyph: usize) -> usize {
    let Some(component) = project.part(part).and_then(|p| p.component) else { return 0 };
    let Some(&c) = project.glyphs.get(glyph).and_then(|g| g.chars.first()) else { return 0 };
    let mut regions = table.occurrences(c, component);
    // In writing order, roughly: top to bottom, left to right.
    regions.sort_by(|a, b| b.y1.total_cmp(&a.y1).then(a.x0.total_cmp(&b.x0)));
    let mut n = 0;
    for r in regions {
        if let Some(t) = placement(project, part, r)
            && project.insert_part(glyph, part, t, None).is_some()
        {
            n += 1;
        }
    }
    n
}

/// How a glyph's character could be put together from the project's parts (the parts' components
/// covering its decomposition), if it can.
pub fn composable(table: &Table, project: &Project, glyph: usize) -> Option<Composition> {
    let &c = project.glyphs.get(glyph)?.chars.first()?;
    table.compose(c, |x| part_for(project, x).is_some())
}

/// Puts a glyph's character together from the project's parts, each where its component sits, in
/// the order of the decomposition. Returns how it was put together.
pub fn compose(table: &Table, project: &mut Project, glyph: usize) -> Option<Composition> {
    let composition = composable(table, project, glyph)?;
    for &(c, region) in &composition.pieces {
        let part = part_for(project, c)?;
        let t = placement(project, part, region)?;
        project.insert_part(glyph, part, t, None)?;
    }
    Some(composition)
}

/// Whether a glyph made of parts (two uses or more) puts one inside another by its character's
/// decomposition: then the writing order may not be the decomposition's (国's last stroke closes
/// it; 这's 辶 comes last).
pub fn order_to_check(table: &Table, project: &Project, glyph: usize) -> bool {
    let Some(g) = project.glyphs.get(glyph) else { return false };
    let uses: BTreeSet<(u32, u32)> = g.strokes.iter().filter_map(|s| s.link).map(|l| (l.part, l.instance)).collect();
    if uses.len() < 2 {
        return false;
    }
    let components: Vec<char> = uses.iter().filter_map(|&(p, _)| project.part(p)?.component).collect();
    g.chars.first().and_then(|&c| table.compose(c, |x| components.iter().any(|&pc| same_component(pc, x)))).is_some_and(|comp| comp.enclosed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scriptoria_ink::kurbo::{BezPath, Point};
    use scriptoria_project::{GlyphStroke, Outline, OutlineKind, SkeletonPiece};

    /// A dot of ink at (x, y), font units.
    fn dot(x: f64, y: f64) -> GlyphStroke {
        let mut path = BezPath::new();
        path.move_to((x, y));
        path.line_to((x + 1.0, y));
        GlyphStroke {
            outline: Outline { kind: OutlineKind::Filled, path },
            skeleton: vec![SkeletonPiece { circles: vec![(Point::new(x, y), 10.0)], spine: Vec::new() }],
            placement: Affine::IDENTITY,
            pen: 1.0,
            link: None,
        }
    }

    fn table() -> &'static Table {
        Table::builtin()
    }

    #[test]
    fn a_part_is_put_where_its_component_sits() {
        let mut p = Project::new("x");
        p.add_chars("清湖品口");
        // 清's 氵 on its left (three dots), 青 on the right.
        p.glyphs[0].strokes = vec![dot(150.0, 700.0), dot(130.0, 450.0), dot(160.0, 100.0), dot(700.0, 400.0)];
        let shui = p.make_part(0, &[0, 1, 2], "氵").unwrap();
        let region = region_in(table(), &p, &['清'], shui).unwrap();
        assert_eq!((region.x0, region.x1), (0.0, 0.5));
        p.part_mut(shui).unwrap().region = Some(region);
        // 湖 has it in the same place: put there as it is in 清.
        let found = uses(table(), &p, shui);
        assert_eq!(found.iter().map(|u| (u.glyph, u.kind)).collect::<Vec<_>>(), [(0, UseKind::Linked), (1, UseKind::Empty)]);
        assert_eq!(put_into(table(), &mut p, shui, 1), 1);
        let (qing, hu) = (&p.glyphs[0].strokes, &p.glyphs[1].strokes);
        assert_eq!(hu.len(), 3);
        for (a, b) in qing.iter().zip(hu) {
            assert!((a.placement.as_coeffs()[4] - b.placement.as_coeffs()[4]).abs() < 1e-6);
        }
        // 口 from 品's top into 品 again: three times, each where its 口 sits.
        p.glyphs[2].strokes = vec![dot(500.0, 700.0)];
        let kou = p.make_part(2, &[0], "口").unwrap();
        p.part_mut(kou).unwrap().region = region_in(table(), &p, &['品'], kou);
        p.glyphs[2].strokes.clear();
        assert_eq!(put_into(table(), &mut p, kou, 2), 3);
        let xs: Vec<f64> = p.glyphs[2].strokes.iter().map(|s| (s.placement * Point::new(500.0, 700.0)).x).collect();
        assert!((xs[0] - 500.0).abs() < 1e-6, "the top one first, where it was: {xs:?}");
        assert!(xs[1] < 500.0 && xs[2] > 500.0, "then left and right below: {xs:?}");
    }

    #[test]
    fn characters_are_put_together_from_parts() {
        let mut p = Project::new("x");
        p.add_chars("河请清国玉");
        p.glyphs[0].strokes = vec![dot(150.0, 700.0), dot(130.0, 450.0), dot(160.0, 100.0)];
        let shui = p.make_part(0, &[0, 1, 2], "氵").unwrap();
        p.part_mut(shui).unwrap().region = region_in(table(), &p, &['河'], shui);
        p.glyphs[1].strokes = vec![dot(150.0, 400.0), dot(700.0, 400.0)];
        let qing = p.make_part(1, &[1], "青").unwrap();
        p.part_mut(qing).unwrap().region = region_in(table(), &p, &['请'], qing);
        // 清 = 氵 + 青, in that order.
        let made = compose(table(), &mut p, 2).unwrap();
        assert!(!made.enclosed);
        let parts: Vec<u32> = p.glyphs[2].strokes.iter().map(|s| s.link.unwrap().part).collect();
        assert_eq!(parts, [shui, shui, shui, qing]);
        assert!(!order_to_check(table(), &p, 2));
        // 国 has no parts to be made of; given 囗 and 玉, it is, with its writing order to check.
        assert!(composable(table(), &p, 3).is_none());
        p.glyphs[4].strokes = vec![dot(500.0, 400.0)];
        let yu = p.make_part(4, &[0], "玉").unwrap();
        p.part_mut(yu).unwrap().region = Some(Rect::new(0.0, 0.0, 1.0, 1.0));
        p.glyphs[1].strokes.push(dot(500.0, 900.0));
        let wei = p.make_part(1, &[2], "囗").unwrap();
        p.part_mut(wei).unwrap().region = Some(Rect::new(0.0, 0.0, 1.0, 1.0));
        assert!(compose(table(), &mut p, 3).unwrap().enclosed);
        assert!(order_to_check(table(), &p, 3));
    }
}
