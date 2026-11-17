//! Editing a stroke's skeleton: the circles along its centerline (nodes, by piece and place in
//! the piece) are moved, made larger or smaller, removed, added, smoothed and thinned out. The
//! outline is made from the skeleton, so this is how a stroke is reshaped without writing it
//! again; the outline as written stays as it was. Coordinates are the page's, as the skeleton's.

use kurbo::{CubicBez, ParamCurve as _, Point, Vec2};

use crate::{GlyphStroke, SkeletonPiece};

/// A circle of a stroke's skeleton: (piece, its place in the piece).
pub type Node = (usize, usize);

impl SkeletonPiece {
    /// The centerline made again through the circles' centers (Catmull–Rom tangents: each
    /// center's tangent parallel to the line between its neighbors).
    pub fn respine(&mut self) {
        let c: Vec<Point> = self.circles.iter().map(|&(p, _)| p).collect();
        let n = c.len();
        let tangent = |i: usize| -> Vec2 {
            match (i.checked_sub(1), (i + 1 < n).then_some(i + 1)) {
                (Some(a), Some(b)) => (c[b] - c[a]) / 2.0,
                (None, Some(b)) => c[b] - c[i],
                (Some(a), None) => c[i] - c[a],
                (None, None) => Vec2::ZERO,
            }
        };
        self.spine = (0..n.saturating_sub(1)).map(|i| CubicBez::new(c[i], c[i] + tangent(i) / 3.0, c[i + 1] - tangent(i + 1) / 3.0, c[i + 1])).collect();
    }
}

impl GlyphStroke {
    /// Whether a node is there.
    pub fn has_node(&self, (p, i): Node) -> bool {
        self.skeleton.get(p).is_some_and(|piece| i < piece.circles.len())
    }

    /// Moves circles by `delta`, the centerline with them (its handles at those circles too).
    pub fn move_nodes(&mut self, nodes: &[Node], delta: Vec2) {
        for &(p, i) in nodes {
            let Some(piece) = self.skeleton.get_mut(p) else { continue };
            let Some(circle) = piece.circles.get_mut(i) else { continue };
            circle.0 += delta;
            if let Some(before) = i.checked_sub(1).and_then(|k| piece.spine.get_mut(k)) {
                before.p2 += delta;
                before.p3 += delta;
            }
            if let Some(after) = piece.spine.get_mut(i) {
                after.p0 += delta;
                after.p1 += delta;
            }
        }
    }

    /// Multiplies the radii of circles by `k`.
    pub fn scale_radii(&mut self, nodes: &[Node], k: f64) {
        for &(p, i) in nodes {
            if let Some(circle) = self.skeleton.get_mut(p).and_then(|piece| piece.circles.get_mut(i)) {
                circle.1 *= k;
            }
        }
    }

    /// Removes circles, the centerline joined over each (from the circle before to the one
    /// after, keeping their handles); a piece left without circles goes. Changes nothing and
    /// returns false if no circle would be left.
    pub fn remove_nodes(&mut self, nodes: &[Node]) -> bool {
        let gone = |p: usize, i: usize| nodes.contains(&(p, i));
        let left: usize = self.skeleton.iter().enumerate().map(|(p, piece)| (0..piece.circles.len()).filter(|&i| !gone(p, i)).count()).sum();
        if left == 0 {
            return false;
        }
        for (p, piece) in self.skeleton.iter_mut().enumerate() {
            // From the last circle back, so that places stay right.
            for i in (0..piece.circles.len()).rev().filter(|&i| gone(p, i)) {
                let n = piece.circles.len();
                piece.circles.remove(i);
                match (i, n) {
                    (_, 1) => {}
                    (0, _) => {
                        piece.spine.remove(0);
                    }
                    (i, n) if i == n - 1 => {
                        piece.spine.remove(i - 1);
                    }
                    (i, _) => {
                        let (a, b) = (piece.spine[i - 1], piece.spine[i]);
                        piece.spine[i - 1] = CubicBez::new(a.p0, a.p1, b.p2, b.p3);
                        piece.spine.remove(i);
                    }
                }
            }
        }
        self.skeleton.retain(|piece| !piece.circles.is_empty());
        true
    }

    /// Adds a circle on segment `k` of piece `p`'s centerline, at `t` along it (its radius between
    /// the two around it); returns the new node.
    pub fn insert_node(&mut self, p: usize, k: usize, t: f64) -> Option<Node> {
        let piece = self.skeleton.get_mut(p)?;
        let seg = *piece.spine.get(k)?;
        let t = t.clamp(0.01, 0.99);
        let (r0, r1) = (piece.circles[k].1, piece.circles[k + 1].1);
        piece.circles.insert(k + 1, (seg.eval(t), r0 + (r1 - r0) * t));
        piece.spine.splice(k..=k, [seg.subsegment(0.0..t), seg.subsegment(t..1.0)]);
        Some((p, k + 1))
    }

    /// Evens out the centers and radii (each with its neighbors in the piece, ends kept), the
    /// centerline made again through them.
    pub fn smooth_skeleton(&mut self) {
        for piece in &mut self.skeleton {
            let old = piece.circles.clone();
            for i in 1..old.len().saturating_sub(1) {
                let (a, b, c) = (old[i - 1], old[i], old[i + 1]);
                piece.circles[i] = (Point::new((a.0.x + 2.0 * b.0.x + c.0.x) / 4.0, (a.0.y + 2.0 * b.0.y + c.0.y) / 4.0), (a.1 + 2.0 * b.1 + c.1) / 4.0);
            }
            piece.respine();
        }
    }

    /// Removes the circles the skeleton does without: those within `tolerance` (page units) of
    /// the line between the circles kept around them, center and radius both; the centerline made
    /// again through the rest. Returns how many went.
    pub fn simplify_skeleton(&mut self, tolerance: f64) -> usize {
        let mut removed = 0;
        for piece in &mut self.skeleton {
            let n = piece.circles.len();
            if n < 3 {
                continue;
            }
            let mut keep = vec![false; n];
            keep[0] = true;
            keep[n - 1] = true;
            douglas_peucker(&piece.circles, 0, n - 1, tolerance, &mut keep);
            let kept: Vec<(Point, f64)> = piece.circles.iter().zip(&keep).filter(|(_, k)| **k).map(|(c, _)| *c).collect();
            removed += n - kept.len();
            if kept.len() < n {
                piece.circles = kept;
                piece.respine();
            }
        }
        removed
    }
}

/// Marks the circles between `a` and `b` that stray from the line between them by more than
/// `tolerance` (center and radius as one point in three dimensions), recursively.
fn douglas_peucker(circles: &[(Point, f64)], a: usize, b: usize, tolerance: f64, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let point = |i: usize| [circles[i].0.x, circles[i].0.y, circles[i].1];
    let (pa, pb) = (point(a), point(b));
    let d: [f64; 3] = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
    let len2 = d.iter().map(|v| v * v).sum::<f64>();
    let distance = |q: [f64; 3]| {
        let v = [q[0] - pa[0], q[1] - pa[1], q[2] - pa[2]];
        let t = if len2 > 0.0 { (v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / len2 } else { 0.0 }.clamp(0.0, 1.0);
        ((v[0] - t * d[0]).powi(2) + (v[1] - t * d[1]).powi(2) + (v[2] - t * d[2]).powi(2)).sqrt()
    };
    let (far, worst) = (a + 1..b).map(|i| (i, distance(point(i)))).fold((a, 0.0), |m, x| if x.1 > m.1 { x } else { m });
    if worst > tolerance {
        keep[far] = true;
        douglas_peucker(circles, a, far, tolerance, keep);
        douglas_peucker(circles, far, b, tolerance, keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Outline, OutlineKind};
    use kurbo::{Affine, BezPath};

    /// A stroke of one straight piece: circles at x = 0, 1, …, n − 1 (y = 0), radius 0.1 + x/100.
    fn line(n: usize) -> GlyphStroke {
        let mut piece = SkeletonPiece { circles: (0..n).map(|i| (Point::new(i as f64, 0.0), 0.1 + i as f64 / 100.0)).collect(), spine: Vec::new() };
        piece.respine();
        GlyphStroke { outline: Outline { kind: OutlineKind::Filled, path: BezPath::new() }, skeleton: vec![piece], placement: Affine::IDENTITY, pen: 1.0, link: None }
    }

    /// Whether every segment of the centerline runs between its two circles.
    fn joined(s: &GlyphStroke) -> bool {
        s.skeleton.iter().all(|p| p.spine.len() + 1 == p.circles.len().max(1) && p.spine.iter().enumerate().all(|(i, seg)| seg.p0 == p.circles[i].0 && seg.p3 == p.circles[i + 1].0))
    }

    #[test]
    fn nodes_move_with_their_handles() {
        let mut s = line(3);
        let before = s.skeleton[0].spine.clone();
        s.move_nodes(&[(0, 1)], Vec2::new(0.0, 0.5));
        assert_eq!(s.skeleton[0].circles[1].0, Point::new(1.0, 0.5));
        assert!(joined(&s));
        assert_eq!(s.skeleton[0].spine[0].p2, before[0].p2 + Vec2::new(0.0, 0.5));
        assert_eq!(s.skeleton[0].spine[0].p1, before[0].p1, "the far handle stays");
        s.scale_radii(&[(0, 0), (0, 2)], 2.0);
        let (r0, r2) = (s.skeleton[0].circles[0].1, s.skeleton[0].circles[2].1);
        assert!((r0 - 0.2).abs() < 1e-12 && (r2 - 0.24).abs() < 1e-12, "{r0} {r2}");
        // Nodes that are not there are left alone.
        s.move_nodes(&[(3, 0), (0, 9)], Vec2::new(1.0, 1.0));
        assert!(!s.has_node((0, 9)) && s.has_node((0, 2)));
    }

    #[test]
    fn nodes_are_removed_and_added() {
        let mut s = line(5);
        assert!(s.remove_nodes(&[(0, 0), (0, 2), (0, 4)]));
        assert_eq!(s.skeleton[0].circles.iter().map(|c| c.0.x).collect::<Vec<_>>(), [1.0, 3.0]);
        assert!(joined(&s));
        // Not the last ones.
        assert!(!s.remove_nodes(&[(0, 0), (0, 1)]));
        assert_eq!(s.skeleton[0].circles.len(), 2);
        let node = s.insert_node(0, 0, 0.5).unwrap();
        assert_eq!(node, (0, 1));
        let (c, r) = s.skeleton[0].circles[1];
        assert!((c - Point::new(2.0, 0.0)).hypot() < 1e-9 && (r - 0.12).abs() < 1e-9, "{c:?} {r}");
        assert!(joined(&s));
        // A piece left without circles goes.
        let mut two = line(2);
        two.skeleton.push(SkeletonPiece { circles: vec![(Point::new(5.0, 5.0), 0.2)], spine: Vec::new() });
        assert!(two.remove_nodes(&[(1, 0)]));
        assert_eq!(two.skeleton.len(), 1);
    }

    #[test]
    fn skeletons_are_smoothed_and_thinned_out() {
        // A zigzag smooths toward the line; ends stay.
        let mut s = line(5);
        s.move_nodes(&[(0, 2)], Vec2::new(0.0, 1.0));
        s.smooth_skeleton();
        let c = &s.skeleton[0].circles;
        assert!(c[2].0.y < 1.0 && c[2].0.y > 0.0 && c[0].0 == Point::ZERO && c[4].0 == Point::new(4.0, 0.0));
        assert!(joined(&s));
        // A straight piece with evenly growing radii needs its ends only; a bend is kept.
        let mut straight = line(9);
        assert_eq!(straight.simplify_skeleton(0.01), 7);
        assert_eq!(straight.skeleton[0].circles.len(), 2);
        assert!(joined(&straight));
        let mut bent = line(9);
        bent.move_nodes(&[(0, 4)], Vec2::new(0.0, 1.0));
        bent.simplify_skeleton(0.01);
        assert!(bent.skeleton[0].circles.iter().any(|c| c.0.y == 1.0));
    }
}
