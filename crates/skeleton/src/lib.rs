//! Recovers a stroke's skeleton from its outline: circles (center + radius) along the stroke,
//! joined by a centerline.
//!
//! Goodnotes draws a stroke as a chain of subpaths. Each subpath is the region swept by a circle
//! moving along a short, possibly curved stretch of centerline to the next circle: round cap
//! arcs at both ends, two side curves following the centerline. Consecutive subpaths share a
//! circle. Fitting those circles yields the stroke's position and width (≈ pen pressure) along its
//! length; the average of the two sides is the centerline. A long subpath that its two circles
//! cannot explain (a fast stroke drawn as one crescent, wider in the middle) gets interior circles
//! from the medial axis between its sides.
//!
//! That local estimate is then refined globally (`refine`): centers, radii and centerline
//! handles are adjusted together until every subpath's outline lies on the boundary of the region
//! swept along its own stretch of the skeleton.
//!
//! A stroke may consist of several disconnected **pieces** (Goodnotes sometimes leaves gaps
//! inside one stroke). Constant-width strokes (PDF `S`) are their own centerline.

mod capsule;
mod circle;
mod refine;

pub use capsule::{SubpathShape, capsule_outline, capsule_sdf, end_circle};
pub use circle::Circle;

/// Low-level steps, exposed for debugging tools and tests.
pub use capsule::analyze as analyze_subpath;
pub use circle::fit as fit_circle;

use kurbo::{self, BezPath, CubicBez, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, PathEl, PathSeg, Point, Vec2};

/// When the two sides of a junction are merged into one circle, its relative RMS on both sides'
/// points must stay below this.
const POOLED_RMS_MAX: f64 = 0.1;
/// Circles of adjacent subpaths count as the same circle when centers and radii differ by less
/// than this fraction of the radius.
const SHARED_CIRCLE_TOL: f64 = 0.1;
/// End circles scoring at least this come from degenerate ends (pointed or flat), not arc fits.
const DEGENERATE_SCORE: f64 = 0.05;
/// Refine every piece globally against the outline after the local estimate (see `refine`).
const REFINE: bool = true;

/// A circle on the skeleton (page points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub p: Point,
    pub r: f64,
}

impl From<Circle> for Sample {
    fn from(c: Circle) -> Self {
        Sample { p: c.c, r: c.r }
    }
}

impl From<Sample> for Circle {
    fn from(s: Sample) -> Self {
        Circle { c: s.p, r: s.r }
    }
}

/// How well the skeleton explains the original outline.
#[derive(Clone, Debug, Default)]
pub struct FitQuality {
    /// Every circle fitted to cap arcs: RMS distance of its boundary points ÷ radius.
    pub residuals: Vec<f64>,
    /// Every junction merged into one shared circle: center offset between the two sides ÷ radius.
    pub chain_gaps: Vec<f64>,
    /// Every subpath: largest distance (pt) from its outline to the region swept along the
    /// skeleton, and the larger radius there.
    pub deviations: Vec<(f64, f64)>,
    /// Circles that come from pointed or flat ends instead of arc fits.
    pub fallbacks: usize,
    /// Junctions whose two circles overlap but differ by more than 10%; both are kept.
    pub breaks: usize,
}

/// A connected part of a stroke.
#[derive(Clone, Debug, Default)]
pub struct Piece {
    pub samples: Vec<Sample>,
    /// `spine[i]` runs from `samples[i].p` to `samples[i + 1].p`.
    pub spine: Vec<CubicBez>,
}

impl Piece {
    /// Points along the centerline, `per_segment` per spine segment, with radii interpolated
    /// linearly between circles.
    pub fn densify(&self, per_segment: usize) -> Vec<Sample> {
        let mut out = Vec::with_capacity(self.spine.len() * per_segment + 1);
        for (i, seg) in self.spine.iter().enumerate() {
            let (r0, r1) = (self.samples[i].r, self.samples[i + 1].r);
            for j in 0..per_segment {
                let t = j as f64 / per_segment as f64;
                out.push(Sample { p: seg.eval(t), r: r0 + (r1 - r0) * t });
            }
        }
        out.extend(self.samples.last().copied());
        out
    }
}

/// A stroke's skeleton: one or more pieces in writing order.
#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub pieces: Vec<Piece>,
    pub quality: FitQuality,
}

impl Skeleton {
    /// All circles, piece after piece.
    pub fn samples(&self) -> impl Iterator<Item = &Sample> {
        self.pieces.iter().flat_map(|p| p.samples.iter())
    }

    pub fn circle_count(&self) -> usize {
        self.pieces.iter().map(|p| p.samples.len()).sum()
    }

    pub fn max_radius(&self) -> f64 {
        self.samples().map(|s| s.r).fold(0.0, f64::max)
    }
}

/// Splits a path at every `MoveTo` and expands each subpath into segments (closing segment
/// included).
pub fn split_segments(path: &BezPath) -> Vec<Vec<PathSeg>> {
    let els = path.elements();
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=els.len() {
        if i == els.len() || matches!(els[i], PathEl::MoveTo(_)) {
            let segs: Vec<PathSeg> = kurbo::segments(els[start..i].iter().copied()).collect();
            if !segs.is_empty() {
                out.push(segs);
            }
            start = i;
        }
    }
    out
}

/// Recovers the skeleton of a filled stroke outline drawn as a chain of capsule subpaths (each
/// subpath: two circles and the region between them, consecutive subpaths sharing a circle).
pub fn recover_outline(path: &BezPath) -> Skeleton {
    let subs = split_segments(path);
    let mut q = FitQuality::default();
    if subs.is_empty() {
        return Skeleton::default();
    }

    // Pass 1: split every subpath into start-circle points, end-circle points and sides;
    // fix the orientation against the previous subpath.
    let mut ends: Vec<(Vec<Point>, Vec<Point>)> = Vec::with_capacity(subs.len());
    let mut sides: Vec<Option<(PathSeg, PathSeg)>> = Vec::with_capacity(subs.len());
    let mut prev_end: Option<Circle> = None;
    for segs in &subs {
        let (mut s, mut e, mut sd) = match analyze_subpath(segs) {
            SubpathShape::Capsule { start, end, sides } => (start, end, sides),
            SubpathShape::Blob(pts) => (pts.clone(), pts, None),
        };
        let (cs, ce) = (end_circle(&s).0, end_circle(&e).0);
        // If the "end" circle is closer to the previous subpath's end circle, the split is reversed.
        if let Some(pc) = prev_end
            && (ce.c - pc.c).hypot() < (cs.c - pc.c).hypot()
        {
            std::mem::swap(&mut s, &mut e);
            sd = sd.map(|(f, b)| (b, f));
            prev_end = Some(cs);
        } else {
            prev_end = Some(ce);
        }
        ends.push((s, e));
        sides.push(sd);
    }

    // Pass 2: decide the circle at every junction (including both stroke ends). Subpath k runs
    // from `pieces[at[k].0].samples[at[k].1]` to the next sample of the same piece.
    let n = ends.len();
    let mut pieces: Vec<Piece> = vec![Piece::default()];
    let mut fit_points: Vec<Vec<Option<Vec<Point>>>> = vec![Vec::new()];
    let mut at = vec![(0usize, 0usize); n];
    let push = |pieces: &mut Vec<Piece>, fit_points: &mut Vec<Vec<Option<Vec<Point>>>>, c: Circle, pts: Option<Vec<Point>>| {
        pieces.last_mut().unwrap().samples.push(c.into());
        fit_points.last_mut().unwrap().push(pts);
        (pieces.len() - 1, pieces.last().unwrap().samples.len() - 1)
    };
    for k in 0..=n {
        let left = (k > 0).then(|| &ends[k - 1].1);
        let right = (k < n).then(|| &ends[k].0);
        let fl = left.map(|p| end_circle(p));
        let fr = right.map(|p| end_circle(p));
        let real = |(_, score): (Circle, f64)| score < DEGENERATE_SCORE;
        let points_if_real = |pts: &Vec<Point>, f: (Circle, f64)| real(f).then(|| pts.clone());
        match (left, right, fl, fr) {
            (Some(l), Some(r), Some(a), Some(b)) => {
                let (ca, cb) = (a.0, b.0);
                let rr = ca.r.max(cb.r);
                let gap = (ca.c - cb.c).hypot();
                if gap > ca.r + cb.r {
                    // The circles do not even touch: the stroke is disconnected here.
                    q.fallbacks += !real(a) as usize + !real(b) as usize;
                    push(&mut pieces, &mut fit_points, ca, points_if_real(l, a));
                    pieces.push(Piece::default());
                    fit_points.push(Vec::new());
                    at[k] = push(&mut pieces, &mut fit_points, cb, points_if_real(r, b));
                } else if gap < SHARED_CIRCLE_TOL * rr && (ca.r - cb.r).abs() < SHARED_CIRCLE_TOL * rr {
                    // The usual case: one circle shared by both subpaths.
                    q.chain_gaps.push(gap / rr);
                    let pooled: Vec<Point> = l.iter().chain(r.iter()).copied().collect();
                    let c = fit_circle(&pooled)
                        .filter(|c| c.relative_rms(&pooled) <= POOLED_RMS_MAX)
                        .unwrap_or(if a.1 <= b.1 { ca } else { cb });
                    let pts = (real(a) || real(b)).then_some(pooled);
                    q.fallbacks += pts.is_none() as usize;
                    at[k] = push(&mut pieces, &mut fit_points, c, pts);
                } else {
                    // Overlapping but different circles: keep both.
                    q.breaks += 1;
                    q.fallbacks += !real(a) as usize + !real(b) as usize;
                    push(&mut pieces, &mut fit_points, ca, points_if_real(l, a));
                    at[k] = push(&mut pieces, &mut fit_points, cb, points_if_real(r, b));
                }
            }
            (Some(pts), None, Some(f), None) => {
                q.fallbacks += !real(f) as usize;
                push(&mut pieces, &mut fit_points, f.0, points_if_real(pts, f));
            }
            (None, Some(pts), None, Some(f)) => {
                q.fallbacks += !real(f) as usize;
                at[k] = push(&mut pieces, &mut fit_points, f.0, points_if_real(pts, f));
            }
            _ => unreachable!(),
        }
    }

    // Centerlines: straight between circles by default, bent along the sides where known.
    for piece in &mut pieces {
        piece.spine = piece.samples.windows(2).map(|w| straight(w[0].p, w[1].p)).collect();
    }
    // Interior circles where the two end circles explain a subpath poorly.
    let mut owner: Vec<Vec<Option<usize>>> = pieces.iter().map(|p| vec![None; p.spine.len()]).collect();
    let mut refined: Vec<Option<(Vec<Sample>, Vec<CubicBez>)>> = vec![None; n];
    for (k, &(pi, a)) in at.iter().enumerate() {
        let piece = &mut pieces[pi];
        if let Some((fwd, bwd)) = sides[k]
            && a + 1 < piece.samples.len()
        {
            piece.spine[a] = spine_from_sides(piece.samples[a].p, piece.samples[a + 1].p, fwd, bwd);
            owner[pi][a] = Some(k);
            refined[k] = medial_circles(&subs[k], piece.samples[a], piece.samples[a + 1], piece.spine[a], fwd, bwd);
        }
    }

    // Rebuild the pieces with the interior circles; `span[k]` is subpath k's first and last sample.
    let mut span = vec![(0usize, 0usize, 0usize); n];
    for (k, &(pi, a)) in at.iter().enumerate() {
        span[k] = (pi, a, (a + 1).min(pieces[pi].samples.len() - 1));
    }
    for (pi, piece) in pieces.iter_mut().enumerate() {
        if owner[pi].iter().flatten().all(|&k| refined[k].is_none()) {
            continue;
        }
        let old = std::mem::take(piece);
        let old_points = std::mem::take(&mut fit_points[pi]);
        let mut points = vec![old_points[0].clone()];
        piece.samples.push(old.samples[0]);
        for (a, seg) in old.spine.iter().enumerate() {
            let start = piece.samples.len() - 1;
            match owner[pi][a].and_then(|k| refined[k].take()) {
                Some((inner, spine)) => {
                    points.extend(inner.iter().map(|_| None));
                    piece.samples.extend(inner);
                    piece.spine.extend(spine);
                }
                None => piece.spine.push(*seg),
            }
            piece.samples.push(old.samples[a + 1]);
            points.push(old_points[a + 1].clone());
            if let Some(k) = owner[pi][a] {
                span[k] = (pi, start, piece.samples.len() - 1);
            }
        }
        fit_points[pi] = points;
    }

    // Global refinement: every subpath's outline on the boundary swept along its own stretch.
    if REFINE {
        for (pi, piece) in pieces.iter_mut().enumerate() {
            let targets: Vec<refine::Target> = subs
                .iter()
                .zip(&span)
                .filter(|(_, s)| s.0 == pi)
                .map(|(segs, &(_, first, last))| refine::Target { segs, first, last })
                .collect();
            refine::refine(piece, &targets);
            retry_ends(piece, &targets);
        }
    }

    // Quality: residual of every fitted circle, deviation of every subpath.
    for (piece, pts) in pieces.iter().zip(&fit_points) {
        for (s, p) in piece.samples.iter().zip(pts) {
            if let Some(p) = p {
                q.residuals.push(Circle::from(*s).relative_rms(p));
            }
        }
    }

    for (segs, &(pi, first, last)) in subs.iter().zip(&span) {
        let piece = &pieces[pi];
        let points: Vec<Point> = segs.iter().flat_map(|s| [0.0, 0.25, 0.5, 0.75].map(|t| s.eval(t))).collect();
        let dev = points.iter().map(|&p| span_sdf(piece, first, last, p).abs()).fold(0.0, f64::max);
        let r = piece.samples[first..=last].iter().map(|s| s.r).fold(0.0, f64::max);
        q.deviations.push((dev, r));
    }

    Skeleton { pieces, quality: q }
}

/// Signed distance from `p` to the region swept along samples `first..=last` of a piece.
fn span_sdf(piece: &Piece, first: usize, last: usize, p: Point) -> f64 {
    if first == last {
        let s = piece.samples[first];
        return (p - s.p).hypot() - s.r;
    }
    (first..last)
        .map(|j| swept_sdf(p, piece.spine[j], piece.samples[j].r, piece.samples[j + 1].r))
        .fold(f64::INFINITY, f64::min)
}

/// A subpath is refined when its sides lie farther than this (× the larger end radius) from the
/// region its two end circles sweep.
const MEDIAL_TRIGGER: f64 = 0.15;
/// Only subpaths whose sides are at least this long (× the larger end radius) are refined: long
/// fast strokes drawn as one subpath. Short odd subpaths sit at hairpins and stroke ends; interior
/// circles there gain nothing measurable after the global refinement (full test page, 2026-11-07).
const MEDIAL_MIN_LENGTH: f64 = 4.0;

/// Interior circles for a subpath that its two end circles explain poorly, such as a long
/// crescent drawn as one subpath (wider in the middle than at either end). Each circle touches the
/// `fwd` side at one point and is the largest one that fits inside the subpath there (a point of
/// the medial axis). Returns the interior circles and the centerline through all of them, or
/// `None` when the subpath needs no refinement or the refinement does not explain it better.
fn medial_circles(segs: &[PathSeg], a: Sample, b: Sample, spine: CubicBez, fwd: PathSeg, bwd: PathSeg) -> Option<(Vec<Sample>, Vec<CubicBez>)> {
    let (f, g) = (SideChain::find(segs, fwd)?, SideChain::find(segs, bwd)?);
    let rmax = a.r.max(b.r);
    if f.total.max(g.total) < MEDIAL_MIN_LENGTH * rmax {
        return None;
    }
    // Where the end circles fall short: along both sides, away from the caps.
    let side_points: Vec<Point> = (1..10).flat_map(|i| [f.at(i as f64 / 10.0).0, g.at(i as f64 / 10.0).0]).collect();
    let deviation = |samples: &[Sample], spines: &[CubicBez]| {
        side_points
            .iter()
            .map(|&p| {
                spines
                    .iter()
                    .enumerate()
                    .map(|(j, s)| swept_sdf(p, *s, samples[j].r, samples[j + 1].r))
                    .fold(f64::INFINITY, f64::min)
                    .abs()
            })
            .fold(0.0, f64::max)
    };
    let before = deviation(&[a, b], &[spine]);
    if before <= MEDIAL_TRIGGER * rmax {
        return None;
    }

    // Distance to the subpath's outline, except the side the circles touch.
    let others: Vec<PathSeg> = (0..segs.len()).filter(|k| !f.indices.contains(k)).map(|k| segs[k]).collect();
    let dist = |p: Point| others.iter().map(|s| s.nearest(p, 1e-9).distance_sq).fold(f64::INFINITY, f64::min).sqrt();
    let nearest_on_other_side = |p: Point| {
        g.pieces.iter().map(|c| c.eval(c.nearest(p, 1e-9).t)).min_by(|x, y| (*x - p).hypot2().total_cmp(&(*y - p).hypot2()))
    };
    // Walk the side from circle a to circle b.
    let reversed = (f.at(0.0).0 - a.p).hypot() > (f.at(1.0).0 - a.p).hypot();
    let count = ((f.total / rmax.max(1e-12)).ceil() as usize).clamp(2, 17) - 1;
    let chord = b.p - a.p;
    let mut inner: Vec<Sample> = Vec::with_capacity(count);
    let mut progress = 0.0;
    for i in 1..=count {
        let u = i as f64 / (count + 1) as f64;
        let (p, d) = f.at(if reversed { 1.0 - u } else { u });
        let Some(q) = nearest_on_other_side(p) else { continue };
        if d.hypot() < 1e-12 {
            continue;
        }
        // The normal pointing into the subpath (toward the other side).
        let mut n = Vec2::new(-d.y, d.x) / d.hypot();
        if n.dot(q - p) < 0.0 {
            n = -n;
        }
        // dist(p + n·ρ) − ρ never increases with ρ: bisect for its root.
        let gap = |rho: f64| dist(p + n * rho) - rho;
        let mut hi = dist(p).max(1e-6 * rmax);
        while gap(hi) > 0.0 && hi < 8.0 * rmax {
            hi *= 2.0;
        }
        if gap(hi) > 0.0 {
            continue;
        }
        let mut lo = 0.0;
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if gap(mid) > 0.0 { lo = mid } else { hi = mid }
        }
        let rho = 0.5 * (lo + hi);
        let c = p + n * rho;
        // Keep the centers in order along the subpath (no zigzag) and strictly inside it.
        let along = (c - a.p).dot(chord) / chord.hypot2().max(1e-24);
        if rho > 1e-6 * rmax && along > progress && along < 1.0 {
            progress = along;
            inner.push(Sample { p: c, r: rho });
        }
    }
    if inner.is_empty() {
        return None;
    }

    // A smooth centerline through all circles; the end tangents follow the original centerline.
    let pts: Vec<Point> = std::iter::once(a.p).chain(inner.iter().map(|s| s.p)).chain(std::iter::once(b.p)).collect();
    let unit = |v: Vec2| if v.hypot() > 1e-12 { v / v.hypot() } else { Vec2::ZERO };
    let m = pts.len();
    let tangents: Vec<Vec2> = (0..m)
        .map(|i| match i {
            0 => unit(if (spine.p1 - spine.p0).hypot() > 1e-12 { spine.p1 - spine.p0 } else { pts[1] - pts[0] }),
            _ if i == m - 1 => unit(if (spine.p3 - spine.p2).hypot() > 1e-12 { spine.p3 - spine.p2 } else { pts[m - 1] - pts[m - 2] }),
            _ => unit(pts[i + 1] - pts[i - 1]),
        })
        .collect();
    let spines: Vec<CubicBez> = (0..m - 1)
        .map(|i| {
            let h = (pts[i + 1] - pts[i]).hypot() / 3.0;
            CubicBez::new(pts[i], pts[i] + tangents[i] * h, pts[i + 1] - tangents[i + 1] * h, pts[i + 1])
        })
        .collect();
    let samples: Vec<Sample> = std::iter::once(a).chain(inner.iter().copied()).chain(std::iter::once(b)).collect();
    (deviation(&samples, &spines) < 0.7 * before).then_some((inner, spines))
}

/// One side of a subpath as its actual pieces: the capsule analysis may have merged several into
/// one approximate cubic, keeping the first piece's start and the last piece's end exactly.
struct SideChain {
    pieces: Vec<CubicBez>,
    /// Indices of the pieces among the subpath's segments.
    indices: Vec<usize>,
    lengths: Vec<f64>,
    total: f64,
}

impl SideChain {
    fn find(segs: &[PathSeg], side: PathSeg) -> Option<Self> {
        let n = segs.len();
        let first = segs.iter().position(|s| s.start() == side.start())?;
        let mut indices = Vec::new();
        for j in 0..n {
            let k = (first + j) % n;
            indices.push(k);
            if segs[k].end() == side.end() {
                let pieces: Vec<CubicBez> = indices.iter().map(|&k| as_cubic(segs[k])).collect();
                let lengths: Vec<f64> = pieces.iter().map(|c| c.arclen(1e-6)).collect();
                let total = lengths.iter().sum();
                return Some(Self { pieces, indices, lengths, total });
            }
        }
        None
    }

    /// Point and (unnormalized) tangent at arclength fraction `u`, in path direction.
    fn at(&self, u: f64) -> (Point, Vec2) {
        let mut s = u.clamp(0.0, 1.0) * self.total;
        for (i, (c, &len)) in self.pieces.iter().zip(&self.lengths).enumerate() {
            if s <= len || i + 1 == self.pieces.len() {
                let t = if len > 1e-12 { c.inv_arclen(s.min(len), 1e-6) } else { 0.0 };
                return (c.eval(t), c.deriv().eval(t).to_vec2());
            }
            s -= len;
        }
        unreachable!("a side has at least one piece")
    }
}

/// `c` oriented to run from near `a` to near `b`.
fn orient(c: CubicBez, a: Point, b: Point) -> CubicBez {
    let same = (c.p0 - a).hypot() + (c.p3 - b).hypot();
    let flip = (c.p0 - b).hypot() + (c.p3 - a).hypot();
    if flip < same { CubicBez::new(c.p3, c.p2, c.p1, c.p0) } else { c }
}

/// Recovers the skeleton of a constant-width line: the path is the centerline, every circle has
/// radius `r` (half the line width).
pub fn recover_line(path: &BezPath, r: f64) -> Skeleton {
    let mut pieces = Vec::new();
    for sub in split_segments(path) {
        let mut piece = Piece::default();
        let mut sub_path = BezPath::new();
        if let Some(first) = sub.first() {
            sub_path.move_to(first.start());
        }
        for seg in &sub {
            sub_path.push(seg.as_path_el());
        }
        kurbo::flatten(sub_path.elements().iter().copied(), 0.05 * r, |el| {
            if let PathEl::MoveTo(p) | PathEl::LineTo(p) = el
                && piece.samples.last().is_none_or(|s| (s.p - p).hypot() > 1e-6 * r)
            {
                piece.samples.push(Sample { p, r });
            }
        });
        piece.spine = piece.samples.windows(2).map(|w| straight(w[0].p, w[1].p)).collect();
        pieces.push(piece);
    }
    Skeleton { pieces, quality: FitQuality::default() }
}

/// A straight centerline between two points.
/// Reads a piece's ends a second way. Local analysis can misread an odd end subpath (a short thick
/// stroke taken for two nearly concentric circles), and refinement then settles in that wrong
/// valley. The second reading follows the subpath's principal axis instead ([`axis_reading`]);
/// whichever explains the outlines better after refinement wins. Tried always for a piece of one
/// subpath, and for an end subpath that fits clearly worse than the rest of its piece.
fn retry_ends(piece: &mut Piece, targets: &[refine::Target]) {
    let n = piece.samples.len();
    if n < 2 || targets.is_empty() {
        return;
    }
    let (mut error, per_target) = refine::fit_error(piece, targets);
    let mut sorted = per_target.clone();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    for at_start in [true, false] {
        let Some(k) = targets.iter().position(|t| if at_start { t.first == 0 } else { t.last == n - 1 }) else { continue };
        let lone = targets.len() == 1;
        if !lone && per_target[k] <= 3.0 * median {
            continue;
        }
        let Some([(a, ra), (b, rb)]) = axis_reading(targets[k].segs) else { continue };
        let mut candidate = piece.clone();
        if lone && n == 2 {
            // Both circles, the one nearer the current pen-down circle first.
            let swap = (a - piece.samples[0].p).hypot() > (b - piece.samples[0].p).hypot();
            let (first, second) = if swap { ((b, rb), (a, ra)) } else { ((a, ra), (b, rb)) };
            candidate.samples = vec![Sample { p: first.0, r: first.1 }, Sample { p: second.0, r: second.1 }];
            candidate.spine = vec![straight(first.0, second.0)];
        } else {
            // Only the end circle: the axis end farther from the neighboring circle.
            let (end, inner, seg) = if at_start { (0, 1, 0) } else { (n - 1, n - 2, n - 2) };
            let joint = piece.samples[inner].p;
            let (p, r) = if (a - joint).hypot() >= (b - joint).hypot() { (a, ra) } else { (b, rb) };
            candidate.samples[end] = Sample { p, r };
            candidate.spine[seg] = straight(candidate.samples[seg].p, candidate.samples[seg + 1].p);
        }
        refine::refine(&mut candidate, targets);
        let (candidate_error, _) = refine::fit_error(&candidate, targets);
        if candidate_error < error {
            *piece = candidate;
            error = candidate_error;
        }
    }
}

/// The two ends of a subpath read along its principal axis: at each end a circle as wide as the
/// outline there (half its extent across the axis over the last third of its length), touching
/// the outline's extreme along the axis.
fn axis_reading(segs: &[PathSeg]) -> Option<[(Point, f64); 2]> {
    let pts: Vec<Point> = segs.iter().flat_map(|s| (0..8).map(move |i| s.eval(i as f64 / 8.0))).collect();
    if pts.len() < 3 {
        return None;
    }
    let m = pts.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / pts.len() as f64;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let d = p.to_vec2() - m;
        (sxx, sxy, syy) = (sxx + d.x * d.x, sxy + d.x * d.y, syy + d.y * d.y);
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (u, v) = (Vec2::new(angle.cos(), angle.sin()), Vec2::new(-angle.sin(), angle.cos()));
    let along: Vec<f64> = pts.iter().map(|p| (p.to_vec2() - m).dot(u)).collect();
    let (lo, hi) = along.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &a| (lo.min(a), hi.max(a)));
    let third = (hi - lo) / 3.0;
    let middle = 0.5 * (lo + hi);
    let end = |near_high: bool| -> Option<(Point, f64)> {
        let across: Vec<f64> = pts
            .iter()
            .zip(&along)
            .filter(|&(_, &a)| if near_high { a >= hi - third } else { a <= lo + third })
            .map(|(p, _)| (p.to_vec2() - m).dot(v))
            .collect();
        let (b0, b1) = across.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &b| (lo.min(b), hi.max(b)));
        let r = 0.5 * (b1 - b0);
        if !(r > 0.0 && r.is_finite()) {
            return None;
        }
        let a = if near_high { (hi - r).max(middle) } else { (lo + r).min(middle) };
        Some(((m + u * a + v * (0.5 * (b0 + b1))).to_point(), r))
    };
    Some([end(false)?, end(true)?])
}

fn straight(a: Point, b: Point) -> CubicBez {
    CubicBez::new(a, a.lerp(b, 1.0 / 3.0), a.lerp(b, 2.0 / 3.0), b)
}

fn as_cubic(seg: PathSeg) -> CubicBez {
    match seg {
        PathSeg::Line(l) => straight(l.p0, l.p1),
        PathSeg::Quad(q) => q.raise(),
        PathSeg::Cubic(c) => c,
    }
}

/// The centerline between circles `a` and `b` from the two side curves: sides ≈ centerline ±
/// radius·normal, so the average of the sides' handles is the centerline's handle.
fn spine_from_sides(a: Point, b: Point, fwd: PathSeg, bwd: PathSeg) -> CubicBez {
    // Orient both sides from a to b (by which circle their endpoints are near).
    let f = orient(as_cubic(fwd), a, b);
    let r = orient(as_cubic(bwd), a, b);
    let chord = (b - a).hypot();
    if chord < 1e-9 {
        return straight(a, b);
    }
    // The sides are shorter than the center distance when the radii differ; scale the handles so
    // the curve's parametrization matches radii interpolated linearly in the parameter.
    let side_chord = ((f.p3 - f.p0).hypot() + (r.p3 - r.p0).hypot()) / 2.0;
    let scale = if side_chord > 1e-12 { chord / side_chord } else { 1.0 };
    // Never let a handle exceed the chord: odd sides must not fling the centerline away.
    let clamp = |h: Vec2| if h.hypot() > chord { h * (chord / h.hypot()) } else { h };
    let h0 = clamp(((f.p1 - f.p0) + (r.p1 - r.p0)) / 2.0 * scale);
    let h1 = clamp(((f.p2 - f.p3) + (r.p2 - r.p3)) / 2.0 * scale);
    CubicBez::new(a, a + h0, b + h1, b)
}

/// Signed distance from `p` to the region swept by a circle along `spine` (radius linear from
/// `r0` to `r1`), approximated by discrete circles at most 1/5 of the smaller radius apart, so the
/// notches between them stay below 0.5% of the radius.
fn swept_sdf(p: Point, spine: CubicBez, r0: f64, r1: f64) -> f64 {
    let len = kurbo::ParamCurveArclen::arclen(&spine, 1e-4);
    let steps = (len / (0.2 * r0.min(r1).max(1e-12))).ceil().clamp(8.0, 1000.0) as usize;
    (0..=steps)
        .map(|i| {
            let t = i as f64 / steps as f64;
            (p - spine.eval(t)).hypot() - (r0 + (r1 - r0) * t)
        })
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goodnotes-style outline of known circles: one capsule subpath per pair of neighbors.
    fn synth(circles: &[Circle]) -> BezPath {
        let mut path = BezPath::new();
        for w in circles.windows(2) {
            path.extend(capsule_outline(w[0], w[1]));
        }
        path
    }

    fn wavy_chain(n: usize, x0: f64) -> Vec<Circle> {
        (0..n)
            .map(|i| {
                let t = i as f64;
                Circle {
                    c: Point::new(x0 + t * 0.6, 200.0 + (t * 0.4).sin() * 2.0),
                    r: 0.12 + 0.05 * (t * 0.7).sin().abs(),
                }
            })
            .collect()
    }

    #[test]
    fn recovers_synthetic_chain() {
        let truth = wavy_chain(25, 100.0);
        let sk = recover_outline(&synth(&truth));
        assert_eq!(sk.pieces.len(), 1);
        let got = &sk.pieces[0].samples;
        assert_eq!(got.len(), truth.len(), "{:?}", sk.quality);
        // The error floor is the cubic approximation of circular arcs (≈2.7e-4·r per 90° piece).
        for (g, w) in got.iter().zip(&truth) {
            assert!((g.p - w.c).hypot() < 1e-3 * w.r, "center {g:?} vs {w:?}");
            assert!((g.r - w.r).abs() < 1e-3 * w.r, "radius {g:?} vs {w:?}");
        }
        assert_eq!(sk.quality.breaks, 0);
        assert_eq!(sk.quality.fallbacks, 0);
        // Deviations are measured against discrete circles 0.2 r apart (notches ≈ 0.5% r).
        let worst = sk.quality.deviations.iter().map(|(d, r)| d / r).fold(0.0, f64::max);
        assert!(worst < 1e-2, "worst deviation {worst}");
    }

    #[test]
    fn disconnected_chains_become_pieces() {
        let (a, b) = (wavy_chain(6, 100.0), wavy_chain(5, 110.0));
        let mut path = synth(&a);
        path.extend(synth(&b));
        let sk = recover_outline(&path);
        assert_eq!(sk.pieces.len(), 2);
        assert_eq!(sk.pieces[0].samples.len(), a.len());
        assert_eq!(sk.pieces[1].samples.len(), b.len());
    }

    /// One long Goodnotes-style subpath shaped like a crescent: end circles of radius 0.1, but the
    /// top side bulges to 0.45 in the middle. Two circles cannot explain it; interior circles from
    /// the medial axis can (the widest one is tangent to both sides: r = 0.275 at x = 1.5).
    #[test]
    fn long_crescent_gets_interior_circles() {
        let k = 0.5523 * 0.1;
        let mut path = BezPath::new();
        path.move_to((0.0, 0.1));
        path.curve_to((1.0, 0.567), (2.0, 0.567), (3.0, 0.1));
        path.line_to((3.00005, 0.1));
        path.curve_to((3.0 + k, 0.1), (3.1, k), (3.1, 0.0));
        path.curve_to((3.1, -k), (3.0 + k, -0.1), (3.0, -0.1));
        path.curve_to((2.0, -0.1), (1.0, -0.1), (0.0, -0.1));
        path.line_to((-0.00005, -0.1));
        path.curve_to((-k, -0.1), (-0.1, -k), (-0.1, 0.0));
        path.curve_to((-0.1, k), (-k, 0.1), (0.0, 0.1));
        path.close_path();
        let sk = recover_outline(&path);
        assert_eq!(sk.pieces.len(), 1);
        assert!(sk.circle_count() > 2, "{:?}", sk.pieces[0].samples);
        let widest = sk.samples().max_by(|a, b| a.r.total_cmp(&b.r)).unwrap();
        assert!((widest.r - 0.275).abs() < 0.02 && (widest.p.x - 1.5).abs() < 0.25, "widest {widest:?}");
        // The centers advance along the stroke (no zigzag).
        assert!(sk.pieces[0].samples.windows(2).all(|w| w[1].p.x > w[0].p.x));
        let (dev, r) = sk.quality.deviations[0];
        assert!(dev < 0.1 * r, "deviation {dev} (r {r})");
    }

    #[test]
    fn centerline_for_line_strokes() {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((1.0, 0.0));
        path.line_to((1.0, 1.0));
        let sk = recover_line(&path, 0.15);
        assert_eq!(sk.pieces.len(), 1);
        assert_eq!(sk.pieces[0].samples.len(), 3);
        assert!(sk.samples().all(|s| s.r == 0.15));
    }
}
