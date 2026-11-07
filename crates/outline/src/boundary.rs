//! The boundary of a union of disks, exactly, as circle arcs.
//!
//! For every disk, the parts of its circle covered by other disks are angular intervals; what is
//! left are the arcs on the union's boundary. Walking the boundary means following an arc to its
//! end, where it meets the circle of the disk that covers what comes next, and continuing on that
//! circle. Walking **clockwise** (decreasing angles, y up) keeps the union on the right: outer
//! boundaries come out clockwise and holes counter-clockwise, the orientation of a stroke contour.

use std::f64::consts::TAU;

use kurbo::{Point, Vec2};

/// Angles within this of an interval's end count as on the boundary.
const ANGLE_EPS: f64 = 1e-9;

pub(crate) struct Union {
    disks: Vec<(Point, f64)>,
    /// Per disk: the covered parts of its circle as disjoint intervals `[lo, hi]` with `lo` in
    /// `[0, 2π)` and `hi` up to `lo + 2π` (an interval may run past angle 0), each with the disk
    /// whose circle bounds it at `hi` (a clockwise walk reaching `hi` continues on that disk);
    /// `None` when the disk lies entirely inside another.
    covered: Vec<Option<Vec<(f64, f64, usize)>>>,
}

impl Union {
    /// The union of `disks`. With `along` (each disk's position along the stroke's centerline),
    /// only *local* coverage counts: disk j covers disk i only if they lie within two diameters of
    /// each other along the stroke. Inner turns and small circles inside a blob are local; where
    /// a stroke crosses itself, it simply passes over the other part (overlap is fine to fill).
    pub fn new(disks: Vec<(Point, f64)>, along: Option<&[f64]>) -> Self {
        let neighbors = Grid::new(&disks);
        let local = |i: usize, j: usize| along.is_none_or(|s| (s[i] - s[j]).abs() <= 2.0 * (disks[i].1 + disks[j].1));
        let covered = (0..disks.len()).map(|i| coverage(&disks, i, neighbors.near(&disks, i).filter(|&j| local(i, j)))).collect();
        Self { disks, covered }
    }

    /// The point at angle `a` on disk `i`'s circle.
    pub fn point(&self, i: usize, a: f64) -> Point {
        let (c, r) = self.disks[i];
        c + Vec2::new(a.cos(), a.sin()) * r
    }

    /// The angle of `p` seen from disk `i`'s center, in `[0, 2π)`.
    pub fn angle(&self, i: usize, p: Point) -> f64 {
        let v = p - self.disks[i].0;
        v.y.atan2(v.x).rem_euclid(TAU)
    }

    /// Whether the point at angle `a` on disk `i`'s circle lies inside another disk.
    pub fn hidden(&self, i: usize, a: f64) -> bool {
        let a = a.rem_euclid(TAU);
        let inside = |lo: f64, hi: f64, x: f64| x > lo + ANGLE_EPS && x < hi - ANGLE_EPS;
        match &self.covered[i] {
            None => true,
            Some(ivs) => ivs.iter().any(|&(lo, hi, _)| inside(lo, hi, a) || inside(lo, hi, a + TAU)),
        }
    }

    /// The boundary from angle `a` on disk `i` clockwise to angle `b` on disk `k`, as points at
    /// most `step` (radians) apart along each arc, both ends included. `None` if the walk does not
    /// get there (the two points lie on different boundary loops, or are not on the boundary).
    pub fn walk(&self, (i, a): (usize, f64), (k, b): (usize, f64), step: f64) -> Option<Vec<Point>> {
        let (mut cur, mut ang) = (i, a.rem_euclid(TAU));
        let mut out = vec![self.point(cur, ang)];
        for _ in 0..4 * self.disks.len() + 8 {
            let ivs = self.covered[cur].as_ref()?;
            // The arc ends where the nearest covered interval clockwise from here begins (its upper
            // end, brought to the last turn not past `ang`); a circle covered nowhere is all
            // boundary.
            let (end, next) = ivs
                .iter()
                .map(|&(_, hi, j)| (hi - TAU * ((hi - ang - ANGLE_EPS) / TAU).ceil(), Some(j)))
                .max_by(|x, y| x.0.total_cmp(&y.0))
                .unwrap_or((ang - TAU, None));
            if cur == k {
                // Is the target on this arc? Bring its angle into (end, ang] by whole turns.
                let mut target = b.rem_euclid(TAU);
                while target > ang + ANGLE_EPS {
                    target -= TAU;
                }
                if target >= end - ANGLE_EPS {
                    self.arc(&mut out, cur, ang, target, step);
                    return Some(out);
                }
            }
            self.arc(&mut out, cur, ang, end, step);
            let next = next?;
            let p = self.point(cur, end);
            (cur, ang) = (next, self.angle(next, p));
        }
        None
    }

    /// Appends points of disk `i`'s circle from angle `from` down to `to` (excluding `from`).
    fn arc(&self, out: &mut Vec<Point>, i: usize, from: f64, to: f64, step: f64) {
        let n = ((from - to) / step).ceil().max(1.0) as usize;
        for s in 1..=n {
            out.push(self.point(i, from + (to - from) * s as f64 / n as f64));
        }
    }
}

/// The covered intervals of disk `i`'s circle (see [`Union::covered`]).
fn coverage(disks: &[(Point, f64)], i: usize, near: impl Iterator<Item = usize>) -> Option<Vec<(f64, f64, usize)>> {
    let (ci, ri) = disks[i];
    let mut raw: Vec<(f64, f64, usize)> = Vec::new();
    for j in near {
        let (cj, rj) = disks[j];
        let d = (cj - ci).hypot();
        let tol = 1e-12 * (ri + rj);
        if j == i || d >= ri + rj || d + rj <= ri + tol {
            continue; // apart, or j inside i (covers nothing of i's circle)
        }
        if d + ri <= rj + tol {
            if d <= tol && (ri - rj).abs() <= tol && j > i {
                continue; // an identical disk: the first one stands for both
            }
            return None; // i lies inside j
        }
        let phi = (cj - ci).y.atan2((cj - ci).x);
        let gamma = ((ri * ri + d * d - rj * rj) / (2.0 * ri * d)).clamp(-1.0, 1.0).acos();
        let lo = (phi - gamma).rem_euclid(TAU);
        raw.push((lo, lo + 2.0 * gamma, j));
    }
    raw.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64, usize)> = Vec::new();
    for iv in raw {
        match merged.last_mut() {
            Some(last) if iv.0 <= last.1 + ANGLE_EPS => {
                if iv.1 > last.1 {
                    (last.1, last.2) = (iv.1, iv.2);
                }
            }
            _ => merged.push(iv),
        }
    }
    // Intervals running past angle 0 into the first ones: merge them across.
    while merged.len() > 1 {
        let (first, last) = (merged[0], merged[merged.len() - 1]);
        if last.1 + ANGLE_EPS < first.0 + TAU {
            break;
        }
        let n = merged.len();
        if first.1 + TAU > last.1 {
            (merged[n - 1].1, merged[n - 1].2) = (first.1 + TAU, first.2);
        }
        merged.remove(0);
    }
    // Covered all around.
    if merged.iter().any(|iv| iv.1 - iv.0 >= TAU - 2.0 * ANGLE_EPS) {
        return None;
    }
    Some(merged)
}

/// A uniform grid over the disk centers, to find the disks that can overlap a given one.
struct Grid {
    cell: f64,
    cells: std::collections::HashMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(disks: &[(Point, f64)]) -> Self {
        let cell = 2.0 * disks.iter().map(|d| d.1).fold(0.0, f64::max).max(1e-9);
        let mut cells: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
        for (i, (c, _)) in disks.iter().enumerate() {
            cells.entry(((c.x / cell).floor() as i64, (c.y / cell).floor() as i64)).or_default().push(i);
        }
        Self { cell, cells }
    }

    /// Disks whose centers lie within one cell of disk `i`'s (two largest radii: every disk that
    /// can overlap it).
    fn near<'a>(&'a self, disks: &[(Point, f64)], i: usize) -> impl Iterator<Item = usize> + 'a {
        let c = disks[i].0;
        let (x, y) = ((c.x / self.cell).floor() as i64, (c.y / self.cell).floor() as i64);
        (-1..=1).flat_map(move |dx| (-1..=1).map(move |dy| (x + dx, y + dy))).flat_map(|k| self.cells.get(&k).into_iter().flatten().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Two equal disks side by side: the walk from the top of the left one goes clockwise over the
    /// top to the right one.
    #[test]
    fn walks_over_two_disks() {
        let u = Union::new(vec![(Point::new(0.0, 0.0), 1.0), (Point::new(1.0, 0.0), 1.0)], None);
        // The left disk's rightmost point is inside the right one; its leftmost is not.
        assert!(u.hidden(0, 0.0));
        assert!(!u.hidden(0, PI));
        // From the left disk's top-left (135°) clockwise to the right disk's bottom-right (−45°).
        let path = u.walk((0, 0.75 * PI), (1, 1.75 * PI), 0.05).unwrap();
        assert!((path[0] - Point::new(-0.5f64.sqrt(), 0.5f64.sqrt())).hypot() < 1e-12);
        assert!((*path.last().unwrap() - Point::new(1.0 + 0.5f64.sqrt(), -0.5f64.sqrt())).hypot() < 1e-12);
        // It passes the junction at the top, (0.5, √3/2), and stays on the boundary.
        assert!(path.iter().any(|p| (*p - Point::new(0.5, 0.75f64.sqrt())).hypot() < 1e-9));
        for p in &path {
            let d = (*p - Point::new(0.0, 0.0)).hypot().min((*p - Point::new(1.0, 0.0)).hypot());
            assert!((d - 1.0).abs() < 1e-9, "{p:?} off the boundary");
        }
    }

    /// A disk inside another is hidden everywhere.
    #[test]
    fn swallowed_disk_is_hidden() {
        let u = Union::new(vec![(Point::new(0.0, 0.0), 1.0), (Point::new(0.2, 0.0), 0.3)], None);
        assert!(u.hidden(1, 0.0) && u.hidden(1, PI));
        assert!(!u.hidden(0, 0.0));
    }

    /// A ring of disks around a hole: the inner boundary is walked counter-clockwise around the
    /// center (clockwise along each circle), and a walk does not jump from the outer loop to it.
    #[test]
    fn ring_has_a_hole() {
        let disks: Vec<(Point, f64)> = (0..24).map(|i| (Point::new(5.0 * (i as f64 * TAU / 24.0).cos(), 5.0 * (i as f64 * TAU / 24.0).sin()), 1.0)).collect();
        let u = Union::new(disks, None);
        // Outer point of disk 0 (angle 0) and inner point of disk 0 (angle π) are both visible.
        assert!(!u.hidden(0, 0.0) && !u.hidden(0, PI));
        // From the inner point of disk 0 to the inner point of disk 6 (a quarter turn): along the
        // hole: every point between 4 (arcs) and 4.2 (where circles meet) from the center, while the
        // outer loop is beyond 5.8.
        let inner = u.walk((0, PI), (6, 6.0 * TAU / 24.0 + PI), 0.05).unwrap();
        assert!(inner.iter().all(|p| p.to_vec2().hypot() < 5.0));
        // The outer loop never reaches the inner one.
        assert!(u.walk((0, 0.0), (6, 6.0 * TAU / 24.0 + PI), 0.05).is_none());
    }
}
