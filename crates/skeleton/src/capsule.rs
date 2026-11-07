//! Geometry of a single Goodnotes subpath: two circles joined by two side curves.
//!
//! - [`analyze`] splits a subpath's boundary into "points on the start circle", "points on the end
//!   circle" and the two sides;
//! - [`capsule_outline`] builds the hull of two circles (synthetic test data, visualization);
//! - [`capsule_sdf`] is the signed distance to such a hull.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use kurbo::{BezPath, CubicBez, Line, ParamCurve, PathEl, PathSeg, Point, Rect, Vec2};

use crate::circle::{self, Circle};

/// A segment is straight when its tangent turns less than this. Goodnotes often draws the sides
/// as slightly bent cubics (up to ~12°), while cap arcs turn 45°–90° per segment.
const STRAIGHT_TURN: f64 = 20.0 * PI / 180.0;
/// A segment is straight when its control points stay this close to the chord (relative to it).
const STRAIGHT_BULGE: f64 = 0.08;
/// Zero-length `L` segments: Goodnotes inserts one after each side, right before a cap arc. They
/// are zero up to coordinate rounding; anything shorter than this fraction of the subpath's extent
/// counts (genuine connector lines are a hundred times longer).
const MARKER_FRACTION: f64 = 1e-3;
/// Fast candidates scoring worse than this trigger the exhaustive run search.
const GOOD_SCORE: f64 = 0.02;
/// A run of segments counts as a cap arc when its circle fits this well (relative RMS)…
const RUN_RMS: f64 = 0.02;
/// …and its points span at least this angle around the circle.
const RUN_COVER: f64 = 60.0 * PI / 180.0;
/// A blob (whole subpath on one circle) must wrap at least this far around it.
const BLOB_COVER: f64 = 270.0 * PI / 180.0;

/// How a subpath's boundary splits between its two circles.
#[derive(Debug, Clone)]
pub enum SubpathShape {
    /// `start`: points on the start circle; `end`: points on the end circle.
    /// `sides`: the two side curves, the first from the start circle to the end circle, the second
    /// back. An end without a cap arc (a pointed or flat stroke end) contributes its corner points,
    /// which [`end_circle`] turns into a small or flat circle.
    Capsule { start: Vec<Point>, end: Vec<Point>, sides: Option<(PathSeg, PathSeg)> },
    /// No two sides found (a dot, or one circle containing the other): all boundary points belong
    /// to one circle.
    Blob(Vec<Point>),
}

/// The circle through a run of boundary points. Runs without a usable arc (pointed or flat
/// stroke ends) become the circle spanning their two farthest points. Returns the circle and a
/// score (relative RMS; degenerate ends get a fixed small penalty).
pub fn end_circle(pts: &[Point]) -> (Circle, f64) {
    // Relative RMS alone favors huge circles through nearly straight points, so the points must
    // also wrap around the circle (cap arcs span about 180°).
    if let Some(c) = circle::fit(pts) {
        let rms = c.relative_rms(pts);
        if rms <= 0.1 && coverage(pts, c.c) >= RUN_COVER {
            return (c, rms);
        }
    }
    let (mut a, mut b, mut best) = (pts.first().copied().unwrap_or_default(), pts.first().copied().unwrap_or_default(), -1.0);
    for (i, p) in pts.iter().enumerate() {
        for q in &pts[i + 1..] {
            let d = (*p - *q).hypot();
            if d > best {
                (a, b, best) = (*p, *q, d);
            }
        }
    }
    (Circle { c: a.midpoint(b), r: (best.max(0.0) / 2.0).max(1e-12) }, 0.05)
}

/// Splits a closed subpath's boundary between its start and end circles.
///
/// Structure (checked on `test/test.pdf`): side → end-circle arc → side → start-circle arc. Sides
/// may consist of several pieces and may bend with the centerline.
///
/// Candidates, the one whose circles fit best wins:
/// 1. **Markers**: every pair of zero-length `L` segments, i.e. `[side] L0 [arc…] [side] L0 [arc…]`;
///    each side is extended backwards over aligned straight pieces.
/// 2. **Geometry**: straight pieces joined into chains; pairs of the three longest chains as sides.
/// 3. **Blob**: everything on one circle (the pen barely moved).
/// 4. If none of these fits well, an exhaustive **run search** over contiguous segment runs
///    (stroke ends, strongly bent pieces).
///
/// The side containing the subpath's first point is the start circle (Goodnotes starts drawing
/// at a tangent point of the start circle); callers re-check the orientation against neighbors.
pub fn analyze(segs: &[PathSeg]) -> SubpathShape {
    let n = segs.len();
    if n < 3 {
        return SubpathShape::Blob(boundary_points(segs));
    }
    let extent = segs.iter().map(|s| s.start()).fold(Rect::from_points(segs[0].start(), segs[0].start()), |r, p| r.union_pt(p));
    let marker_len = MARKER_FRACTION * (extent.width().hypot(extent.height()));
    let markers: Vec<usize> =
        (0..n).filter(|&i| matches!(segs[i], PathSeg::Line(_)) && chord(&segs[i]) < marker_len).collect();
    let outline = subpath_path(segs);

    let mut best: Option<(f64, SubpathShape)> = None;
    let consider = |best: &mut Option<(f64, SubpathShape)>, shape: SubpathShape, bias: f64| {
        let s = score(&shape, &outline) + bias;
        if best.as_ref().is_none_or(|(b, _)| s < *b) {
            *best = Some((s, shape));
        }
    };
    for (i, &m1) in markers.iter().enumerate() {
        for &m2 in &markers[i + 1..] {
            if let Some(shape) = by_markers(segs, m1, m2, marker_len) {
                consider(&mut best, shape, 0.0);
            }
        }
    }
    // Geometry and blob candidates are less reliable than markers: a small bias lets markers win ties.
    for shape in by_geometry(segs) {
        consider(&mut best, shape, 1e-3);
    }
    consider(&mut best, SubpathShape::Blob(boundary_points(segs)), 2e-3);
    if best.as_ref().is_some_and(|(s, _)| *s > GOOD_SCORE)
        && let Some(shape) = by_run_search(segs)
    {
        consider(&mut best, shape, 0.0);
    }
    best.map(|(_, s)| s).unwrap()
}

/// The subpath as a closed path.
fn subpath_path(segs: &[PathSeg]) -> BezPath {
    let mut path = BezPath::new();
    if let Some(first) = segs.first() {
        path.move_to(first.start());
    }
    for s in segs {
        path.push(s.as_path_el());
    }
    path.close_path();
    path
}

/// Whether a circle sticks out of the subpath by more than `tol` × its radius. Both circles of a
/// subpath are part of the region it outlines, so a circle reaching outside is a wrong split.
fn sticks_out(c: Circle, outline: &BezPath, tol: f64) -> bool {
    use kurbo::{ParamCurveNearest, Shape};
    (0..16).any(|i| {
        let a = TAU * i as f64 / 16.0;
        let p = c.c + Vec2::new(a.cos(), a.sin()) * c.r;
        outline.winding(p) == 0
            && outline.segments().map(|s| s.nearest(p, 1e-6).distance_sq).fold(f64::INFINITY, f64::min).sqrt() > tol * c.r
    })
}

/// Worse of the two circles' fit scores (lower is better).
fn score(shape: &SubpathShape, outline: &BezPath) -> f64 {
    match shape {
        SubpathShape::Capsule { start, end, .. } => {
            let ((a, sa), (b, sb)) = (end_circle(start), end_circle(end));
            // Neighboring circles have similar widths; a much larger one is usually a bad split.
            // (Not applied to degenerate ends: a pointed tip is legitimately tiny.)
            let ratio = a.r.max(b.r) / a.r.min(b.r).max(1e-9);
            let both_fitted = sa < 0.05 && sb < 0.05;
            // Fitted circles must stay inside; degenerate ends are rough stand-ins (already
            // penalized), so they may only protrude a little.
            let tol = |s: f64| if s < 0.05 { 0.05 } else { 1.0 };
            let outside = sticks_out(a, outline, tol(sa)) as u8 as f64 + sticks_out(b, outline, tol(sb)) as u8 as f64;
            sa.max(sb) + if both_fitted { 0.05 * (ratio - 3.0).max(0.0) } else { 0.0 } + outside
        }
        // A blob must be (almost) a full circle.
        SubpathShape::Blob(pts) => circle::fit(pts)
            .filter(|c| coverage(pts, c.c) >= BLOB_COVER)
            .map_or(f64::INFINITY, |c| c.relative_rms(pts) + sticks_out(c, outline, 0.05) as u8 as f64),
    }
}

/// A run of consecutive segments (indices modulo the segment count) on one circle.
struct Run {
    /// Index of the first segment; its start point is the run's first point.
    begin: usize,
    segs: Vec<usize>,
}

impl Run {
    fn points(&self, all: &[PathSeg], arc: impl Fn(usize) -> bool) -> Vec<Point> {
        let mut pts = vec![all[self.begin % all.len()].start()];
        for &k in &self.segs {
            if arc(k) {
                for t in [0.25, 0.5, 0.75] {
                    pts.push(all[k].eval(t));
                }
            }
            pts.push(all[k].end());
        }
        pts
    }
}

/// Builds a capsule from the two runs between the sides. `y_to_x` is the side going from run y
/// to run x, `x_to_y` the other one.
fn capsule_from_runs(
    segs: &[PathSeg],
    x: Run,
    y: Run,
    arc: impl Fn(usize) -> bool + Copy,
    y_to_x: PathSeg,
    x_to_y: PathSeg,
) -> SubpathShape {
    let (px, py) = (x.points(segs, arc), y.points(segs, arc));
    // The subpath starts at a tangent point of the start circle: the run closer to it is the start.
    // (Geometric test, not index based: the closing segment may be an extra zero-length piece.)
    let origin = segs[0].start();
    let near = |pts: &[Point]| pts.iter().map(|p| (*p - origin).hypot()).fold(f64::INFINITY, f64::min);
    if near(&px) < near(&py) {
        SubpathShape::Capsule { start: px, end: py, sides: Some((x_to_y, y_to_x)) }
    } else {
        SubpathShape::Capsule { start: py, end: px, sides: Some((y_to_x, x_to_y)) }
    }
}

fn by_markers(segs: &[PathSeg], m1: usize, m2: usize, marker_len: f64) -> Option<SubpathShape> {
    let n = segs.len();
    let end1 = (m1 + n - 1) % n; // last piece of side 1
    let end2 = (m2 + n - 1) % n;
    if end1 == m2 || end2 == m1 {
        return None; // adjacent markers: not this structure
    }
    // Sides extend backwards over aligned straight pieces; those are not part of the cap arcs.
    let first1 = extend_back(segs, end1, m2, marker_len);
    let first2 = extend_back(segs, end2, m1, marker_len);
    // After a marker comes a cap arc, up to the next side. Side 1 sits right before marker m1 and
    // ends where arc x begins, so it runs from y to x; side 2 runs from x to y.
    let x = Run { begin: m1 + 1, segs: cyclic_range(m1 + 1, first2, n) };
    let y = Run { begin: m2 + 1, segs: cyclic_range(m2 + 1, first1, n) };
    let arc = |k: usize| !matches!(segs[k], PathSeg::Line(_));
    Some(capsule_from_runs(segs, x, y, arc, join(segs, first1, end1), join(segs, first2, end2)))
}

/// From a side's last piece `last`, walks backwards over aligned straight pieces until `stop`
/// (the other marker). Returns the side's first piece.
fn extend_back(segs: &[PathSeg], last: usize, stop: usize, marker_len: f64) -> usize {
    let n = segs.len();
    let mut first = last;
    loop {
        let prev = (first + n - 1) % n;
        if prev == stop || prev == last {
            return first;
        }
        let aligned = match (end_dir(&segs[prev]), start_dir(&segs[first])) {
            (Some(a), Some(b)) => turn(a, b) < STRAIGHT_TURN,
            _ => false,
        };
        if !(is_straight(&segs[prev]) && aligned && chord(&segs[prev]) > marker_len) {
            return first;
        }
        first = prev;
    }
}

/// Joins pieces `first..=last` (cyclic) into one segment: a single piece is returned as is;
/// several become one cubic keeping the first piece's start tangent and the last piece's end tangent.
fn join(segs: &[PathSeg], first: usize, last: usize) -> PathSeg {
    if first == last {
        return segs[first];
    }
    let (a, b) = (segs[first].start(), segs[last].end());
    let len = (b - a).hypot();
    let unit = |v: Option<Vec2>| v.map(|v| v / v.hypot()).unwrap_or((b - a) / len.max(1e-12));
    let (t0, t1) = (unit(start_dir(&segs[first])), unit(end_dir(&segs[last])));
    PathSeg::Cubic(CubicBez::new(a, a + t0 * (len / 3.0), b - t1 * (len / 3.0), b))
}

fn start_dir(seg: &PathSeg) -> Option<Vec2> {
    let v = match *seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => first_nonzero(&[q.p1 - q.p0, q.p2 - q.p0]),
        PathSeg::Cubic(c) => first_nonzero(&[c.p1 - c.p0, c.p2 - c.p0, c.p3 - c.p0]),
    };
    (v.hypot2() > 1e-18).then_some(v)
}

fn end_dir(seg: &PathSeg) -> Option<Vec2> {
    let v = match *seg {
        PathSeg::Line(l) => l.p1 - l.p0,
        PathSeg::Quad(q) => first_nonzero(&[q.p2 - q.p1, q.p2 - q.p0]),
        PathSeg::Cubic(c) => first_nonzero(&[c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0]),
    };
    (v.hypot2() > 1e-18).then_some(v)
}

/// Geometry candidates: pairs among the three longest straight chains as the sides.
fn by_geometry(segs: &[PathSeg]) -> Vec<SubpathShape> {
    let n = segs.len();
    let straight: Vec<bool> = segs.iter().map(is_straight).collect();
    if straight.iter().all(|&s| s) {
        return Vec::new();
    }
    let chains = straight_chains(segs, &straight);
    let mut by_len: Vec<&Chain> = chains.iter().collect();
    by_len.sort_by(|a, b| b.length.total_cmp(&a.length));
    by_len.truncate(3);
    let mut out = Vec::new();
    for i in 0..by_len.len() {
        for j in i + 1..by_len.len() {
            let (mut s1, mut s2) = (by_len[i], by_len[j]);
            if s1.first > s2.first {
                std::mem::swap(&mut s1, &mut s2);
            }
            let (e1, e2) = (s1.first + s1.count, s2.first + s2.count);
            let x = Run { begin: e1, segs: cyclic_range(e1, s2.first, n) };
            let y = Run { begin: e2, segs: cyclic_range(e2, s1.first, n) };
            // Run x follows s1, so s1 goes from y to x; s2 goes from x to y.
            out.push(capsule_from_runs(segs, x, y, |k| !straight[k], s1.as_seg(segs), s2.as_seg(segs)));
        }
    }
    out
}

/// Exhaustive search: every contiguous run of segments containing a curved piece is fitted with a
/// circle; the two disjoint runs that fit well and cover the most of their circles are the caps.
/// When only one cap arc exists (pointed or flat stroke end), the other end is the stretch between
/// the two longest remaining pieces (the sides), as corner points.
fn by_run_search(segs: &[PathSeg]) -> Option<SubpathShape> {
    let n = segs.len();
    if n > 32 {
        return None;
    }
    let curved: Vec<bool> = segs.iter().map(|s| !is_straight(s)).collect();
    struct Fit {
        begin: usize,
        len: usize,
        cover: f64,
        rms: f64,
        circle: Circle,
    }
    let mut fits = Vec::new();
    for begin in 0..n {
        for len in 1..n - 1 {
            if !(0..len).any(|i| curved[(begin + i) % n]) {
                continue;
            }
            let run = Run { begin, segs: (0..len).map(|i| (begin + i) % n).collect() };
            let pts = run.points(segs, |k| curved[k]);
            let Some(c) = circle::fit(&pts) else { continue };
            let rms = c.relative_rms(&pts);
            let cover = coverage(&pts, c.c);
            // Cap arcs only contain short straight connectors; a long straight piece means the run
            // swallowed part of a side (a huge circle through a side and an arc can still "fit").
            let only_short_lines = run.segs.iter().all(|&k| curved[k] || chord(&segs[k]) < 0.25 * c.r);
            if rms <= RUN_RMS && cover >= RUN_COVER && only_short_lines {
                fits.push(Fit { begin, len, cover, rms, circle: c });
            }
        }
    }
    // Two runs with at least one segment between them on both sides, on clearly different circles
    // (two arcs of the same circle are not the two ends of a capsule).
    let separated = |a: &Fit, b: &Fit| {
        let (ca, cb) = (a.circle, b.circle);
        let distinct = (ca.c - cb.c).hypot() >= 0.25 * ca.r.min(cb.r) || (ca.r - cb.r).abs() >= 0.25 * ca.r.max(cb.r);
        let gap1 = (b.begin + n - (a.begin + a.len) % n) % n;
        let gap2 = (a.begin + n - (b.begin + b.len) % n) % n;
        distinct && gap1 >= 1 && gap2 >= 1 && a.len + b.len + gap1 + gap2 == n
    };
    let mut best: Option<(f64, usize, usize)> = None;
    for (i, a) in fits.iter().enumerate() {
        for (j, b) in fits.iter().enumerate().skip(i + 1) {
            if separated(a, b) {
                let s = a.cover + b.cover - 5.0 * (a.rms + b.rms);
                if best.is_none_or(|(bs, _, _)| s > bs) {
                    best = Some((s, i, j));
                }
            }
        }
    }
    let run_of = |f: &Fit| Run { begin: f.begin, segs: (0..f.len).map(|i| (f.begin + i) % n).collect() };
    let between = |from: usize, to: usize| {
        // Pieces from `from` up to (excluding) `to`, joined as a side.
        let last = (to + n - 1) % n;
        join(segs, from % n, last)
    };
    if let Some((_, i, j)) = best {
        let (a, b) = (&fits[i], &fits[j]);
        let (x, y) = (run_of(b), run_of(a)); // side from a to b runs y → x
        let y_to_x = between(a.begin + a.len, b.begin);
        let x_to_y = between(b.begin + b.len, a.begin);
        return Some(capsule_from_runs(segs, x, y, |k| curved[k], y_to_x, x_to_y));
    }

    // One cap arc: the opposite end lies between the two longest remaining pieces.
    let a = fits.iter().max_by(|p, q| (p.cover - 5.0 * p.rms).total_cmp(&(q.cover - 5.0 * q.rms)))?;
    let rest: Vec<usize> = (0..n - a.len).map(|i| (a.begin + a.len + i) % n).collect();
    if rest.len() < 2 {
        return None;
    }
    let mut longest: Vec<usize> = rest.clone();
    longest.sort_by(|p, q| chord(&segs[*q]).total_cmp(&chord(&segs[*p])));
    let (mut s1, mut s2) = (longest[0], longest[1]);
    // Order the two sides as they appear after the arc.
    let pos = |k: usize| rest.iter().position(|&r| r == k).unwrap();
    if pos(s1) > pos(s2) {
        std::mem::swap(&mut s1, &mut s2);
    }
    // The end is the stretch strictly between the sides (possibly empty: a pointed tip).
    let tip = Run { begin: s1 + 1, segs: cyclic_range(s1 + 1, s2, n) };
    let arc = run_of(a);
    // Side s1 leaves the arc, side s2 comes back to it.
    Some(capsule_from_runs(segs, tip, arc, |k| curved[k], segs[s1], segs[s2]))
}

/// Angle (radians) spanned by points around a center: 2π minus the largest angular gap.
fn coverage(pts: &[Point], c: Point) -> f64 {
    let mut a: Vec<f64> = pts.iter().map(|p| (p.y - c.y).atan2(p.x - c.x)).collect();
    if a.len() < 2 {
        return 0.0;
    }
    a.sort_by(f64::total_cmp);
    let mut gap = a[0] + TAU - a[a.len() - 1];
    for w in a.windows(2) {
        gap = gap.max(w[1] - w[0]);
    }
    TAU - gap
}

struct Chain {
    first: usize,
    count: usize,
    length: f64,
}

impl Chain {
    /// The chain as one segment: a single piece as is, several as the chord.
    fn as_seg(&self, segs: &[PathSeg]) -> PathSeg {
        let n = segs.len();
        if self.count == 1 {
            return segs[self.first];
        }
        PathSeg::Line(Line::new(segs[self.first].start(), segs[(self.first + self.count - 1) % n].end()))
    }
}

/// All (cyclic) runs of consecutive straight pieces whose directions agree at the joints.
fn straight_chains(segs: &[PathSeg], straight: &[bool]) -> Vec<Chain> {
    let n = segs.len();
    // Start scanning after a curved piece so a chain wrapping around index 0 stays whole.
    let Some(anchor) = straight.iter().position(|&s| !s) else { return Vec::new() };
    let mut chains: Vec<Chain> = Vec::new();
    let mut prev_dir: Option<Vec2> = None;
    let mut open = false;
    for i in 1..=n {
        let k = (anchor + i) % n;
        if !straight[k] {
            open = false;
            prev_dir = None;
            continue;
        }
        let dir = direction(&segs[k]);
        // Zero-length pieces do not affect the direction; a corner breaks the chain.
        let continues = open
            && match (prev_dir, dir) {
                (Some(a), Some(b)) => turn(a, b) < STRAIGHT_TURN,
                _ => true,
            };
        if continues {
            let c = chains.last_mut().unwrap();
            c.count += 1;
            c.length += chord(&segs[k]);
        } else {
            chains.push(Chain { first: k, count: 1, length: chord(&segs[k]) });
            open = true;
        }
        if dir.is_some() {
            prev_dir = dir;
        }
    }
    chains
}

/// Direction of a piece (`None` when it has zero length).
fn direction(seg: &PathSeg) -> Option<Vec2> {
    let d = seg.end() - seg.start();
    (d.hypot() > 1e-9).then_some(d)
}

fn cyclic_range(from: usize, to: usize, n: usize) -> Vec<usize> {
    let (from, to) = (from % n, to % n);
    let mut v = Vec::new();
    let mut i = from;
    while i != to {
        v.push(i);
        i = (i + 1) % n;
    }
    v
}

fn boundary_points(segs: &[PathSeg]) -> Vec<Point> {
    let mut pts = Vec::new();
    for seg in segs {
        pts.push(seg.start());
        if !is_straight(seg) {
            for t in [0.25, 0.5, 0.75] {
                pts.push(seg.eval(t));
            }
        }
    }
    pts
}

fn chord(seg: &PathSeg) -> f64 {
    (seg.end() - seg.start()).hypot()
}

fn is_straight(seg: &PathSeg) -> bool {
    match *seg {
        PathSeg::Line(_) => true,
        PathSeg::Quad(q) => bulge(q.p0, q.p2, &[q.p1]) && turn(q.p1 - q.p0, q.p2 - q.p1) < STRAIGHT_TURN,
        PathSeg::Cubic(c) => {
            let t0 = first_nonzero(&[c.p1 - c.p0, c.p2 - c.p0, c.p3 - c.p0]);
            let t1 = first_nonzero(&[c.p3 - c.p2, c.p3 - c.p1, c.p3 - c.p0]);
            bulge(c.p0, c.p3, &[c.p1, c.p2]) && turn(t0, t1) < STRAIGHT_TURN
        }
    }
}

fn first_nonzero(vs: &[Vec2]) -> Vec2 {
    vs.iter().copied().find(|v| v.hypot2() > 1e-18).unwrap_or(Vec2::ZERO)
}

fn turn(a: Vec2, b: Vec2) -> f64 {
    if a.hypot2() < 1e-18 || b.hypot2() < 1e-18 {
        return 0.0;
    }
    a.cross(b).atan2(a.dot(b)).abs()
}

/// Whether all control points stay close to the chord.
fn bulge(p0: Point, p3: Point, ctrl: &[Point]) -> bool {
    let d = p3 - p0;
    let len = d.hypot();
    if len < 1e-12 {
        return ctrl.iter().all(|c| (*c - p0).hypot() < 1e-9);
    }
    ctrl.iter().all(|c| (d.cross(*c - p0) / len).abs() <= STRAIGHT_BULGE * len)
}

// ---------------------------------------------------------------------------------------------
// Capsule outlines and distances

/// Outward unit normals of the two outer tangent lines of two circles; `None` when one circle
/// contains the other.
fn tangent_normals(a: Circle, b: Circle) -> Option<(Vec2, Vec2)> {
    let d = b.c - a.c;
    let len = d.hypot();
    if len <= (a.r - b.r).abs() + 1e-12 {
        return None;
    }
    let u = d / len;
    let s = (a.r - b.r) / len; // n·u
    let c = (1.0 - s * s).sqrt();
    let perp = Vec2::new(-u.y, u.x);
    Some((u * s + perp * c, u * s - perp * c))
}

/// The hull of two circles, laid out like Goodnotes draws it: tangent point on the start circle →
/// side → arc around the end circle → side → arc around the start circle → back.
pub fn capsule_outline(a: Circle, b: Circle) -> BezPath {
    let mut path = BezPath::new();
    let Some((n1, n2)) = tangent_normals(a, b) else {
        let big = if a.r >= b.r { a } else { b };
        path.move_to(big.c + Vec2::new(big.r, 0.0));
        arc_to(&mut path, big, 0.0, TAU);
        path.close_path();
        return path;
    };
    let ang = |v: Vec2| v.y.atan2(v.x);
    let (t1a, t1b) = (a.c + n1 * a.r, b.c + n1 * b.r);
    let t2a = a.c + n2 * a.r;
    // With n1, n2 at ±φ from the a→b direction, the end circle's outer arc turns 2φ clockwise
    // from n1 to n2 (through +u); the start circle's arc turns 2π−2φ clockwise from n2 back to n1.
    let sweep_b = -ccw_angle(n2, n1);
    let sweep_a = -ccw_angle(n1, n2);
    path.move_to(t1a);
    path.line_to(t1b);
    arc_to(&mut path, b, ang(n1), sweep_b);
    path.line_to(t2a);
    arc_to(&mut path, a, ang(n2), sweep_a);
    path.close_path();
    path
}

/// Counter-clockwise angle from `from` to `to`, in [0, 2π).
fn ccw_angle(from: Vec2, to: Vec2) -> f64 {
    let a = from.cross(to).atan2(from.dot(to));
    if a < 0.0 { a + TAU } else { a }
}

/// Appends cubics along a circular arc, at most 90° each.
fn arc_to(path: &mut BezPath, c: Circle, start: f64, sweep: f64) {
    let pieces = (sweep.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / pieces as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..pieces {
        let a0 = start + step * i as f64;
        let a1 = a0 + step;
        let (p0, p3) = (polar(c, a0), polar(c, a1));
        let d0 = Vec2::new(-a0.sin(), a0.cos()) * (c.r * k);
        let d1 = Vec2::new(-a1.sin(), a1.cos()) * (c.r * k);
        path.push(PathEl::CurveTo(p0 + d0, p3 - d1, p3));
    }
}

fn polar(c: Circle, a: f64) -> Point {
    c.c + Vec2::new(a.cos(), a.sin()) * c.r
}

/// Signed distance from a point to the hull of two circles (negative inside).
/// Formula: Inigo Quilez's "uneven capsule" SDF.
pub fn capsule_sdf(p: Point, a: Circle, b: Circle) -> f64 {
    let d = b.c - a.c;
    let h = d.hypot();
    if h <= (a.r - b.r).abs() + 1e-12 {
        // One circle contains the other.
        return ((p - a.c).hypot() - a.r).min((p - b.c).hypot() - b.r);
    }
    let v = d / h;
    let w = p - a.c;
    // Local frame: y along a→b, x mirrored to the positive side.
    let q = Vec2::new(w.cross(v).abs(), w.dot(v));
    let bb = (a.r - b.r) / h;
    let aa = (1.0 - bb * bb).sqrt();
    let k = q.dot(Vec2::new(-bb, aa));
    if k < 0.0 {
        q.hypot() - a.r
    } else if k > aa * h {
        (q - Vec2::new(0.0, h)).hypot() - b.r
    } else {
        q.dot(Vec2::new(aa, bb)) - a.r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split_segments;

    fn circle(x: f64, y: f64, r: f64) -> Circle {
        Circle { c: Point::new(x, y), r }
    }

    #[test]
    fn outline_points_lie_on_capsule_boundary() {
        let (a, b) = (circle(0.0, 0.0, 0.12), circle(1.0, 0.2, 0.18));
        let path = capsule_outline(a, b);
        for seg in path.segments() {
            for t in [0.0, 0.3, 0.7] {
                let d = capsule_sdf(seg.eval(t), a, b);
                assert!(d.abs() < 1e-3 * a.r, "sdf {d} at {:?}", seg.eval(t));
            }
        }
    }

    #[test]
    fn analyze_separates_circles() {
        let (a, b) = (circle(0.0, 0.0, 0.12), circle(1.0, 0.2, 0.18));
        let segs = &split_segments(&capsule_outline(a, b))[0];
        let SubpathShape::Capsule { start, end, .. } = analyze(segs) else { panic!("expected capsule") };
        for p in &start {
            assert!(a.signed_distance(*p).abs() < 1e-3 * a.r, "start point {p:?} not on a");
        }
        for p in &end {
            assert!(b.signed_distance(*p).abs() < 1e-3 * b.r, "end point {p:?} not on b");
        }
    }

    /// A tapered stroke end: sides converge to a point, only the start has a cap arc.
    #[test]
    fn pointed_end_becomes_tiny_circle() {
        let a = circle(0.0, 0.0, 0.15);
        let tip = Point::new(1.0, 0.0);
        let mut path = BezPath::new();
        // Tangent points of `a` seen from the tip.
        let d = (tip - a.c).hypot();
        let phi = (a.r / d).acos();
        let up = a.c + Vec2::new(phi.cos(), phi.sin()) * a.r;
        let down = a.c + Vec2::new(phi.cos(), -phi.sin()) * a.r;
        path.move_to(up);
        path.line_to(tip);
        path.line_to(down);
        arc_to(&mut path, a, -phi, -(TAU - 2.0 * phi));
        path.close_path();
        let segs = &split_segments(&path)[0];
        let SubpathShape::Capsule { start, end, .. } = analyze(segs) else { panic!("expected capsule") };
        let (cs, _) = end_circle(&start);
        let (ce, _) = end_circle(&end);
        // Either orientation is fine here; check that one end is `a` and the other the tip.
        let (round, pointed) = if cs.r > ce.r { (cs, ce) } else { (ce, cs) };
        assert!((round.c - a.c).hypot() < 1e-3 && (round.r - a.r).abs() < 1e-3, "{round:?}");
        assert!((pointed.c - tip).hypot() < 1e-2 && pointed.r < 0.01, "{pointed:?}");
    }

    #[test]
    fn sdf_signs() {
        let (a, b) = (circle(0.0, 0.0, 1.0), circle(4.0, 0.0, 0.5));
        assert!((capsule_sdf(Point::new(2.0, 0.0), a, b) + 0.75).abs() < 0.05);
        assert!((capsule_sdf(Point::new(-2.0, 0.0), a, b) - 1.0).abs() < 1e-9);
        assert!((capsule_sdf(Point::new(5.0, 0.0), a, b) - 0.5).abs() < 1e-9);
    }
}
