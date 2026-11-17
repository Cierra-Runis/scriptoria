//! Parts: strokes kept to be put into glyphs (a radical, a component), and glyphs kept like them.
//!
//! The strokes a glyph got from a part are linked to it ([`Link`]): they are the part's strokes
//! under one transform, in the glyph's writing order among its own (even between them, as 国's
//! closing stroke comes after what it closes). When the part changes, every glyph using it changes
//! with it ([`Project::sync_part`]); when a glyph's linked strokes change other than all together
//! by one transform, they are no longer the part's and are unlinked ([`Project::normalize_links`]).

use std::collections::{BTreeMap, BTreeSet};

use kurbo::{Affine, Rect};

use crate::{Glyph, GlyphStroke, Project};

/// Strokes kept to be put into glyphs.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub id: u32,
    pub name: String,
    /// The character it is in characters' decompositions (ideographic description sequences),
    /// if any: 氵 for the water radical.
    pub component: Option<char>,
    /// Where it is in the character it was taken from, by that character's decomposition: a box
    /// in fractions of the em (x from the left, y from the bottom), for putting it where it belongs
    /// in other characters.
    pub region: Option<Rect>,
    /// Its strokes, as a glyph of no character, placed as in the glyph they were taken from.
    pub glyph: Glyph,
}

/// A glyph's stroke put in from a part, and kept like it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    /// The part's id.
    pub part: u32,
    /// Which of the glyph's uses of the part (a glyph may use one more than once: 品).
    pub instance: u32,
    /// Which of the part's strokes.
    pub index: usize,
    /// Where the part is put in the glyph: the stroke's placement is this times the part stroke's.
    pub transform: Affine,
}

/// Uses of parts a change made no longer the parts' own: (part, instance).
pub type Unlinked = Vec<(u32, u32)>;

impl Part {
    /// A part of strokes (their links dropped), named.
    pub fn new(id: u32, name: &str, strokes: Vec<GlyphStroke>, units_per_em: f64) -> Part {
        let strokes = strokes.into_iter().map(|s| GlyphStroke { link: None, ..s }).collect();
        let component = {
            let mut chars = name.chars();
            chars.next().filter(|_| chars.next().is_none())
        };
        Part { id, name: name.to_string(), component, region: None, glyph: Glyph { name: name.to_string(), chars: Vec::new(), advance: units_per_em, strokes } }
    }

    /// Its stroke `index` as put in under `transform`, use `instance` of it.
    fn placed(&self, index: usize, instance: u32, transform: Affine) -> Option<GlyphStroke> {
        let s = self.glyph.strokes.get(index)?;
        Some(GlyphStroke { placement: transform * s.placement, link: Some(Link { part: self.id, instance, index, transform }), ..s.clone() })
    }
}

/// Whether two placements are the same, as far as rounding goes.
fn same(a: Affine, b: Affine) -> bool {
    let (a, b) = (a.as_coeffs(), b.as_coeffs());
    let scale = a.iter().chain(&b).fold(1.0f64, |m, v| m.max(v.abs()));
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-9 * scale)
}

impl Project {
    pub fn part(&self, id: u32) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    pub fn part_mut(&mut self, id: u32) -> Option<&mut Part> {
        self.parts.iter_mut().find(|p| p.id == id)
    }

    /// An id no part has.
    pub fn next_part_id(&self) -> u32 {
        self.parts.iter().map(|p| p.id + 1).max().unwrap_or(1)
    }

    /// The glyphs using a part, by index.
    pub fn glyphs_using(&self, part: u32) -> Vec<usize> {
        (0..self.glyphs.len()).filter(|&g| self.glyphs[g].strokes.iter().any(|s| s.link.is_some_and(|l| l.part == part))).collect()
    }

    /// Keeps some of a glyph's strokes (by index) as a new part, named; they stay in the glyph as
    /// its first use. Returns the part's id.
    pub fn make_part(&mut self, glyph: usize, strokes: &[usize], name: &str) -> Option<u32> {
        let g = self.glyphs.get(glyph)?;
        let chosen: Vec<GlyphStroke> = strokes.iter().filter_map(|&i| g.strokes.get(i).cloned()).collect();
        if chosen.is_empty() {
            return None;
        }
        let id = self.next_part_id();
        let instance = next_instance(g);
        self.parts.push(Part::new(id, name, chosen, self.font.units_per_em as f64));
        let g = &mut self.glyphs[glyph];
        let n = g.strokes.len();
        for (index, &i) in strokes.iter().filter(|&&i| i < n).enumerate() {
            g.strokes[i].link = Some(Link { part: id, instance, index, transform: Affine::IDENTITY });
        }
        Some(id)
    }

    /// Puts a part into a glyph under `transform`, its strokes at `at` in the writing order (after
    /// the last, without); returns the use.
    pub fn insert_part(&mut self, glyph: usize, part: u32, transform: Affine, at: Option<usize>) -> Option<u32> {
        let p = self.part(part)?.clone();
        let g = self.glyphs.get_mut(glyph)?;
        let instance = next_instance(g);
        let strokes: Vec<GlyphStroke> = (0..p.glyph.strokes.len()).filter_map(|i| p.placed(i, instance, transform)).collect();
        let at = at.unwrap_or(g.strokes.len()).min(g.strokes.len());
        g.strokes.splice(at..at, strokes);
        Some(instance)
    }

    /// Makes every glyph using a part like it again: each linked stroke replaced where it is, the
    /// part's new strokes after the use's last, strokes it no longer has taken out. Returns the
    /// glyphs changed.
    pub fn sync_part(&mut self, part: u32) -> Vec<usize> {
        let Some(p) = self.part(part).cloned() else { return Vec::new() };
        let n = p.glyph.strokes.len();
        let mut changed = Vec::new();
        for (gi, g) in self.glyphs.iter_mut().enumerate() {
            let uses: BTreeMap<u32, Affine> =
                g.strokes.iter().filter_map(|s| s.link.filter(|l| l.part == part)).map(|l| (l.instance, l.transform)).collect();
            if uses.is_empty() {
                continue;
            }
            let before = g.strokes.clone();
            for (&instance, &transform) in &uses {
                let mut strokes = Vec::with_capacity(g.strokes.len() + n);
                let mut have = BTreeSet::new();
                let mut last = None;
                for s in g.strokes.drain(..) {
                    match s.link.filter(|l| l.part == part && l.instance == instance) {
                        Some(l) if l.index < n && have.insert(l.index) => {
                            strokes.extend(p.placed(l.index, instance, transform));
                            last = Some(strokes.len());
                        }
                        Some(_) => {}
                        None => strokes.push(s),
                    }
                }
                let at = last.unwrap_or(strokes.len());
                let added: Vec<GlyphStroke> = (0..n).filter(|i| !have.contains(i)).filter_map(|i| p.placed(i, instance, transform)).collect();
                strokes.splice(at..at, added);
                g.strokes = strokes;
            }
            if g.strokes != before {
                changed.push(gi);
            }
        }
        changed
    }

    /// Keeps a glyph's uses of parts linked where their strokes are still the part's, all under
    /// one transform (taking that transform: the use moved, turned or scaled as a whole); unlinks
    /// the others (their strokes stay as they are). Returns the uses unlinked.
    pub fn normalize_links(&mut self, glyph: usize) -> Unlinked {
        let Some(g) = self.glyphs.get(glyph) else { return Vec::new() };
        let uses: BTreeSet<(u32, u32)> = g.strokes.iter().filter_map(|s| s.link).map(|l| (l.part, l.instance)).collect();
        let mut unlinked = Vec::new();
        let mut kept = Vec::new();
        for (part, instance) in uses {
            let mine: Vec<&GlyphStroke> = g.strokes.iter().filter(|s| s.link.is_some_and(|l| l.part == part && l.instance == instance)).collect();
            let whole = self.part(part).and_then(|p| {
                let n = p.glyph.strokes.len();
                let indices: BTreeSet<usize> = mine.iter().filter_map(|s| s.link.map(|l| l.index)).collect();
                if mine.len() != n || indices.len() != n || indices.iter().any(|&i| i >= n) {
                    return None;
                }
                let first = mine[0];
                let source = &p.glyph.strokes[first.link?.index];
                let transform = first.placement * source.placement.inverse();
                mine.iter()
                    .all(|s| {
                        let source = &p.glyph.strokes[s.link.expect("linked").index];
                        s.outline == source.outline && s.skeleton == source.skeleton && s.pen == source.pen && same(s.placement, transform * source.placement)
                    })
                    .then_some(transform)
            });
            match whole {
                Some(transform) => kept.push((part, instance, transform)),
                None => unlinked.push((part, instance)),
            }
        }
        let g = &mut self.glyphs[glyph];
        for s in &mut g.strokes {
            let Some(l) = &mut s.link else { continue };
            if let Some(&(_, _, transform)) = kept.iter().find(|(p, i, _)| *p == l.part && *i == l.instance) {
                l.transform = transform;
            } else if unlinked.contains(&(l.part, l.instance)) {
                s.link = None;
            }
        }
        unlinked
    }

    /// Unlinks a use of a part in a glyph: its strokes stay, as the glyph's own.
    pub fn unlink(&mut self, glyph: usize, part: u32, instance: u32) {
        if let Some(g) = self.glyphs.get_mut(glyph) {
            for s in &mut g.strokes {
                if s.link.is_some_and(|l| l.part == part && l.instance == instance) {
                    s.link = None;
                }
            }
        }
    }

    /// Forgets a part: the glyphs using it keep its strokes as their own. Returns them.
    pub fn remove_part(&mut self, part: u32) -> Vec<usize> {
        let users = self.glyphs_using(part);
        for &g in &users {
            for s in &mut self.glyphs[g].strokes {
                if s.link.is_some_and(|l| l.part == part) {
                    s.link = None;
                }
            }
        }
        self.parts.retain(|p| p.id != part);
        users
    }
}

/// A use number a glyph does not have yet.
fn next_instance(g: &Glyph) -> u32 {
    g.strokes.iter().filter_map(|s| s.link.map(|l| l.instance + 1)).max().unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Outline, OutlineKind, SkeletonPiece};
    use kurbo::{BezPath, Point, Vec2};

    /// A stroke whose skeleton is one circle at `(x, y)`.
    fn stroke(x: f64, y: f64) -> GlyphStroke {
        let mut path = BezPath::new();
        path.move_to((x, y));
        path.line_to((x + 1.0, y));
        GlyphStroke {
            outline: Outline { kind: OutlineKind::Filled, path },
            skeleton: vec![SkeletonPiece { circles: vec![(Point::new(x, y), 0.1)], spine: Vec::new() }],
            placement: Affine::scale(100.0),
            pen: 100.0,
            link: None,
        }
    }

    /// 氵 (three strokes) kept from 河, with 河's own two strokes after it; 清 empty.
    fn project() -> (Project, u32) {
        let mut p = Project::new("x");
        p.add_chars("河清");
        p.glyphs[0].strokes = vec![stroke(0.0, 0.0), stroke(0.0, 1.0), stroke(0.0, 2.0), stroke(5.0, 0.0), stroke(5.0, 1.0)];
        let id = p.make_part(0, &[0, 1, 2], "氵").unwrap();
        (p, id)
    }

    #[test]
    fn a_part_is_made_from_a_glyph_and_put_into_others() {
        let (mut p, id) = project();
        let part = p.part(id).unwrap();
        assert_eq!((part.name.as_str(), part.component, part.glyph.strokes.len()), ("氵", Some('氵'), 3));
        assert!(part.glyph.strokes.iter().all(|s| s.link.is_none()));
        assert_eq!(p.glyphs_using(id), [0]);
        // Into 清, moved right by 10.
        let at = Affine::translate((10.0, 0.0));
        let instance = p.insert_part(1, id, at, None).unwrap();
        let g = &p.glyphs[1];
        assert_eq!(g.strokes.len(), 3);
        assert_eq!(g.strokes[2].placement, at * Affine::scale(100.0));
        assert_eq!(g.strokes[2].link, Some(Link { part: id, instance, index: 2, transform: at }));
        assert_eq!(p.glyphs_using(id), [0, 1]);
    }

    #[test]
    fn glyphs_follow_their_parts() {
        let (mut p, id) = project();
        p.insert_part(1, id, Affine::translate((10.0, 0.0)), None);
        // 河 written with its own stroke between the part's: it stays where it is.
        p.glyphs[0].strokes.swap(2, 3);
        assert!(p.normalize_links(0).is_empty());
        // The part changes: its second stroke moves, a fourth is added.
        let part = p.part_mut(id).unwrap();
        part.glyph.strokes[1] = stroke(0.5, 1.0);
        part.glyph.strokes.push(stroke(0.0, 3.0));
        assert_eq!(p.sync_part(id), [0, 1]);
        let he = &p.glyphs[0].strokes;
        assert_eq!(he.len(), 6);
        assert_eq!(he[1].skeleton[0].circles[0].0, Point::new(0.5, 1.0));
        assert!(he[2].link.is_none(), "the glyph's own stroke stays between");
        // The new stroke after the use's last.
        assert_eq!(he[4].link.map(|l| l.index), Some(3));
        let qing = &p.glyphs[1].strokes;
        assert_eq!(qing[1].placement, Affine::translate((10.0, 0.0)) * Affine::scale(100.0));
        assert_eq!(qing.len(), 4);
        // Strokes the part no longer has go.
        p.part_mut(id).unwrap().glyph.strokes.truncate(2);
        p.sync_part(id);
        assert_eq!(p.glyphs[1].strokes.len(), 2);
        assert_eq!(p.glyphs[0].strokes.len(), 4);
    }

    #[test]
    fn uses_moved_whole_stay_linked_and_changed_ones_are_unlinked() {
        let (mut p, id) = project();
        let instance = p.insert_part(1, id, Affine::IDENTITY, None).unwrap();
        // Moved and scaled as a whole: still the part's, under the new transform.
        let t = Affine::translate(Vec2::new(30.0, 5.0)) * Affine::scale(0.8);
        p.glyphs[1].transform_strokes(&[0, 1, 2], t);
        assert!(p.normalize_links(1).is_empty());
        assert!(p.glyphs[1].strokes.iter().all(|s| s.link.is_some_and(|l| same(l.transform, t))));
        // One of its strokes moved on its own: no longer the part's.
        p.glyphs[1].transform_strokes(&[1], Affine::translate((1.0, 0.0)));
        assert_eq!(p.normalize_links(1), [(id, instance)]);
        assert!(p.glyphs[1].strokes.iter().all(|s| s.link.is_none()));
        // A stroke taken out of a use unlinks it too; forgetting a part unlinks every use.
        p.glyphs[0].remove_strokes(&[0]);
        assert_eq!(p.normalize_links(0).len(), 1);
        let (mut p, id) = project();
        assert_eq!(p.remove_part(id), [0]);
        assert!(p.glyphs[0].strokes.iter().all(|s| s.link.is_none()) && p.parts.is_empty());
    }
}
