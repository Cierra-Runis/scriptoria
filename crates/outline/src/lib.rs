//! Regenerates stroke outlines from a skeleton, at any weight.
//!
//! Every piece of a stroke becomes one closed contour whose structure depends only on the
//! skeleton, never on the weight, so the outlines of all weights are compatible and can serve
//! directly as the masters of a variable font:
//!
//! ```text
//! left point at pen-down L₀ → left side L₁ … Lₘ → end cap (line, 2 cubics, line) → Rₘ
//!                          → right side Rₘ₋₁ … R₀ → start cap (line, 2 cubics, line) → back to L₀
//! ```
//!
//! (The cap lines usually have zero length; see `cap`.)
//!
//! "Left" is the writer's left in a y-up coordinate system, so the contour is **clockwise** with
//! y up (the TrueType outer-contour direction) and counter-clockwise in y-down page coordinates.
//! Left and right on-curve points come in pairs (Lᵢ ↔ Rᵢ) around the same centerline point, so
//! downstream tools can recover the skeleton.
//!
//! How the sides are built, so that the outline follows the union of the skeleton's circles:
//! 1. the centerline is sampled densely (in length and in turning), with explicit tangents and a
//!    fan of extra directions at kinks and tight turns (so the outer side wraps around them);
//! 2. each sample contributes one point per side on the envelope of the circle family;
//! 3. side points swallowed by nearby circles are pushed out onto the union boundary, and local
//!    self-intersections (swallowtails at sharp inner turns) are cut out;
//! 4. every half-cell between consecutive samples is made to run the way the contour turns
//!    (`make_positive`), which rules out holes under the nonzero fill rule whatever the skeleton;
//! 5. one least-squares cubic is fitted per knot interval, with shared tangents at smooth knots and
//!    one-sided tangents at corners.

mod boundary;

use kurbo::{self, BezPath, CubicBez, ParamCurve, ParamCurveArclen, ParamCurveDeriv, ParamCurveNearest, Point, Vec2};
use scriptoria_skeleton::{Sample, Skeleton};

/// Weight parameters: new radius = clamp(radius × scale + offset, r_min, r_max).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeightParams {
    pub scale: f64,
    pub offset: f64,
    pub r_min: f64,
    pub r_max: f64,
}

impl Default for WeightParams {
    fn default() -> Self {
        Self { scale: 1.0, offset: 0.0, r_min: 0.0, r_max: f64::INFINITY }
    }
}

impl WeightParams {
    /// Scales radii only.
    pub fn scaled(scale: f64) -> Self {
        Self { scale, ..Self::default() }
    }

    pub fn radius(&self, r: f64) -> f64 {
        (r * self.scale + self.offset).clamp(self.r_min, self.r_max).max(0.0)
    }
}

/// A point on the densely sampled centerline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DenseSample {
    pub p: Point,
    pub r: f64,
    /// Unit tangent (direction of writing).
    pub t: Vec2,
    /// Radius change per unit of centerline length.
    pub dr: f64,
    /// Centerline length from the piece's start.
    pub s: f64,
}

/// The weight-independent part of an outline: dense samples along each piece's centerline and
/// the knots (indices into the dense samples) where the side curves are split.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub pieces: Vec<PiecePlan>,
}

#[derive(Clone, Debug, Default)]
pub struct PiecePlan {
    pub dense: Vec<DenseSample>,
    /// Indices into `dense`, increasing; always contains the first and the last index.
    pub knots: Vec<usize>,
}

impl PiecePlan {
    /// The circles at the knots (the on-curve points of the side curves sit on them).
    pub fn knot_samples(&self) -> impl Iterator<Item = Sample> + '_ {
        self.knots.iter().map(|&i| Sample { p: self.dense[i].p, r: self.dense[i].r })
    }
}

impl Plan {
    /// Number of on-curve points per contour side, piece by piece (identical for every weight).
    pub fn knot_counts(&self) -> Vec<usize> {
        self.pieces.iter().map(|p| p.knots.len()).collect()
    }
}

/// Kinks sharper than this get a fan of extra directions.
const KINK: f64 = 3.0 * std::f64::consts::PI / 180.0;
/// Angular step of a fan.
const FAN_STEP: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// What a plan must achieve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanParams {
    /// Largest distance (pt) of an outline's cubics from the exact outline, at every weight in
    /// `weights`: the one accuracy setting (as a font editor's fitting precision).
    pub precision: f64,
    /// The weights (radius scales) the outlines must serve, thinnest and boldest: the masters of
    /// a variable font.
    pub weights: (f64, f64),
}

impl Default for PlanParams {
    fn default() -> Self {
        // 0.004 pt is one font unit for a 4 pt high character at 1000 units per em.
        Self { precision: 0.004, weights: (0.3, 3.0) }
    }
}

/// Samples each piece's centerline densely (spacing ≤ ¼ of the local radius, finer where it turns)
/// and picks as few knots as the precision allows: each stretch between knots is as long as one
/// cubic per side can follow within `params.precision` at both weights of `params.weights`. The
/// knots are shared by all weights, which is what keeps the outlines compatible.
pub fn plan(sk: &Skeleton, params: &PlanParams) -> Plan {
    let pieces = sk.pieces.iter().map(|piece| plan_piece(&piece.samples, &piece.spine, params)).collect();
    Plan { pieces }
}

fn plan_piece(samples: &[Sample], spine: &[CubicBez], params: &PlanParams) -> PiecePlan {
    let mut dense: Vec<DenseSample> = Vec::new();
    if spine.is_empty() {
        if let Some(s) = samples.first() {
            dense.push(DenseSample { p: s.p, r: s.r, t: Vec2::new(1.0, 0.0), dr: 0.0, s: 0.0 });
        }
        return PiecePlan { dense, knots: (0..samples.len().min(1)).collect() };
    }
    let mut s_acc = 0.0;
    let mut prev_t: Option<Vec2> = None;
    let mut prev_dr = 0.0;
    // Forced knots: both ends of every fan, so the sides' corners stay corners.
    let mut forced: Vec<usize> = Vec::new();
    for (i, seg) in spine.iter().enumerate() {
        let (r0, r1) = (samples[i].r, samples[i + 1].r);
        let len = seg.arclen(1e-4);
        let dr = if len > 1e-9 { (r1 - r0) / len } else { 0.0 };
        let n = (len / (0.25 * r0.min(r1).max(0.02))).ceil().clamp(2.0, 400.0) as usize;
        let d = seg.deriv();
        let chord = seg.p3 - seg.p0;
        let before = prev_t;
        let tangent = |t: f64| {
            let v = d.eval(t).to_vec2();
            if v.hypot() > 1e-9 {
                v / v.hypot()
            } else if chord.hypot() > 1e-12 {
                chord / chord.hypot()
            } else {
                before.unwrap_or(Vec2::new(1.0, 0.0))
            }
        };
        // A fan at the joint with the previous segment, when the centerline kinks there.
        let t_in = tangent(0.0);
        if let Some(t_prev) = prev_t {
            let a = t_prev.cross(t_in).atan2(t_prev.dot(t_in));
            if a.abs() > KINK {
                let steps = (a.abs() / FAN_STEP).ceil() as usize;
                // The joint with the incoming direction, then the fan, then (below) the outgoing one.
                forced.push(dense.len());
                dense.push(DenseSample { p: seg.p0, r: r0, t: t_prev, dr: prev_dr, s: s_acc });
                for k in 1..steps {
                    let ang = t_prev.y.atan2(t_prev.x) + a * k as f64 / steps as f64;
                    dense.push(DenseSample { p: seg.p0, r: r0, t: Vec2::new(ang.cos(), ang.sin()), dr: 0.0, s: s_acc });
                }
                forced.push(dense.len());
            }
        }
        // Parameters: uniform (spacing ≤ ¼ r), bisected wherever the tangent turns more than a
        // fan step between neighbors, so a tight turn inside a segment is sampled like a fan.
        let mut ts: Vec<f64> = (0..=n).map(|j| j as f64 / n as f64).collect();
        let mut k = 0;
        while k + 1 < ts.len() {
            let (a, b) = (ts[k], ts[k + 1]);
            let (ta, tb) = (tangent(a), tangent(b));
            if ta.cross(tb).atan2(ta.dot(tb)).abs() > FAN_STEP && b - a > 1e-7 && ts.len() < 4000 {
                ts.insert(k + 1, 0.5 * (a + b));
            } else {
                k += 1;
            }
        }
        // A turn the bisection cannot resolve (a cusp) becomes an explicit fan around its point.
        let mut prev: Option<Vec2> = None;
        for &t in &ts[..ts.len() - 1] {
            let (p, r, tan, s) = (seg.eval(t), r0 + (r1 - r0) * t, tangent(t), s_acc + len * t);
            if let Some(tp) = prev {
                let a = tp.cross(tan).atan2(tp.dot(tan));
                let steps = (a.abs() / FAN_STEP).ceil().max(1.0) as usize;
                for k in 1..steps {
                    let ang = tp.y.atan2(tp.x) + a * k as f64 / steps as f64;
                    dense.push(DenseSample { p, r, t: Vec2::new(ang.cos(), ang.sin()), dr: 0.0, s });
                }
            }
            dense.push(DenseSample { p, r, t: tan, dr, s });
            prev = Some(tan);
        }
        s_acc += len;
        prev_t = Some(tangent(1.0));
        prev_dr = dr;
        if i + 1 == spine.len() {
            dense.push(DenseSample { p: seg.p3, r: r1, t: tangent(1.0), dr, s: s_acc });
        }
    }

    let knots = select_knots(&dense, &forced, params);
    PiecePlan { dense, knots }
}

/// The fewest knots (dense indices, increasing, first and last included, `forced` kept) such that
/// between consecutive knots one cubic per side follows each side within `params.precision`, at
/// both weights of `params.weights`. Greedy: from each knot, the farthest next one that still fits
/// (found by bisection; a stretch that fits keeps fitting when shortened).
fn select_knots(dense: &[DenseSample], forced: &[usize], params: &PlanParams) -> Vec<usize> {
    let m = dense.len();
    if m <= 2 {
        return (0..m).collect();
    }
    // Both sides at both weights, as polylines indexed like `dense`, with travel directions.
    let mut sides: Vec<(Vec<Point>, Vec<Option<Vec2>>)> = Vec::with_capacity(4);
    for w in [params.weights.0, params.weights.1] {
        let (pts, dirs) = contour(dense, &WeightParams::scaled(w));
        sides.push((pts[..m].to_vec(), dirs[..m].to_vec()));
        let right = |k: usize| m + CAP_POINTS + (m - 1 - k);
        sides.push(((0..m).map(|k| pts[right(k)]).collect(), (0..m).map(|k| dirs[right(k)].map(|d| -d)).collect()));
    }
    let fits = |k0: usize, k1: usize| {
        sides.iter().all(|(pts, dirs)| {
            let span = &pts[k0..=k1];
            let t0 = dirs[k0].unwrap_or_else(|| first_direction(span.iter().copied()));
            let t1 = dirs[k1].unwrap_or_else(|| -first_direction(span.iter().rev().copied()));
            max_distance(&fit_cubic(span, t0, t1), span) <= params.precision
        })
    };
    let mut bounds: Vec<usize> = forced.iter().copied().chain([0, m - 1]).filter(|&k| k < m).collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut knots = vec![0];
    for w in bounds.windows(2) {
        let (mut k0, b) = (w[0], w[1]);
        while k0 < b {
            let k1 = if fits(k0, b) {
                b
            } else {
                let (mut lo, mut hi) = (k0 + 1, b);
                while hi - lo > 1 {
                    let mid = (lo + hi) / 2;
                    if fits(k0, mid) { lo = mid } else { hi = mid }
                }
                lo
            };
            knots.push(k1);
            k0 = k1;
        }
    }
    knots
}

/// The unit direction from the first point towards the first point distinct from it.
fn first_direction(mut pts: impl Iterator<Item = Point>) -> Vec2 {
    let Some(a) = pts.next() else { return Vec2::new(1.0, 0.0) };
    pts.map(|p| p - a).find(|v| v.hypot() > 1e-12).map_or(Vec2::new(1.0, 0.0), unit_or_zero)
}

/// The outline at weight `w`: one closed contour per piece, in writing order.
pub fn expand(plan: &Plan, w: &WeightParams) -> BezPath {
    let mut path = BezPath::new();
    for piece in &plan.pieces {
        expand_piece(&mut path, piece, w);
    }
    path
}

/// Points of each cap between the two sides: a cap turns half a circle, one point every 10°.
const CAP_POINTS: usize = 17;

/// One piece's outline at weight `w`: its contour ([`contour`]) fitted with one cubic per knot
/// interval.
///
/// Every dense sample is a disk; the contour follows the boundary of their union (counting only
/// local coverage, see [`boundary::Union::new`]). Its points are fixed slots, the same at every
/// weight: the left side's envelope point of every sample, the end cap, the right side's
/// backwards, the start cap. A slot whose point is visible lies on the boundary as it is; a run of
/// hidden slots (inside other disks: inner turns, small circles inside a blob, ends folded into
/// the stroke) is moved onto the stretch of boundary between the visible slots around it, walked
/// exactly along the circle arcs ([`boundary`]). The contour then is the union's boundary itself,
/// free of holes and folds, except where a run had to be left as it was ([`relocate`]).
fn expand_piece(path: &mut BezPath, piece: &PiecePlan, w: &WeightParams) {
    let m = piece.dense.len();
    if m == 0 {
        return;
    }
    if m == 1 {
        circle(path, piece.dense[0].p, w.radius(piece.dense[0].r));
        return;
    }
    let (mut pts, mut dirs) = contour(&piece.dense, w);
    // Knots: the plan's on both sides and the middle of each cap; then once more the first point.
    let n = pts.len();
    let middle = CAP_POINTS / 2 + 1;
    let mut knots: Vec<usize> = piece.knots.clone();
    knots.push(m - 1 + middle);
    knots.extend(piece.knots.iter().rev().map(|&k| m + CAP_POINTS + (m - 1 - k)));
    knots.push(2 * m + CAP_POINTS + middle - 1);
    knots.push(n);
    pts.push(pts[0]);
    dirs.push(dirs[0]);
    path.move_to(pts[0]);
    fit_polyline(path, &pts, &knots, &dirs);
    path.close_path();
}

/// The contour's points at weight `w` (at least two dense samples), in slot order, with the
/// travel direction where it is known exactly (see [`expand_piece`]).
fn contour(dense: &[DenseSample], w: &WeightParams) -> (Vec<Point>, Vec<Option<Vec2>>) {
    use std::f64::consts::{PI, TAU};
    let m = dense.len();
    let r: Vec<f64> = dense.iter().map(|d| w.radius(d.r)).collect();

    // The disks along the centerline (repeats of a sample, as in a fan, are one disk).
    let mut disk_of = Vec::with_capacity(m);
    let mut disks: Vec<(Point, f64)> = Vec::new();
    let mut along: Vec<f64> = Vec::new();
    for (d, &ri) in dense.iter().zip(&r) {
        if disks.last().is_none_or(|&(c, rr)| (c - d.p).hypot() > 1e-12 || (rr - ri).abs() > 1e-12) {
            disks.push((d.p, ri));
            along.push(d.s);
        }
        disk_of.push(disks.len() - 1);
    }
    let union = boundary::Union::new(disks, Some(&along));

    // Envelope of the circle family: p + r·(−r'·t ± √(1−r'²)·n), n = writer's left (y up). The
    // left side runs along t·c + n·r', the right side along t·c − n·r'.
    let side = |k: usize, sign: f64| -> (Point, Vec2) {
        let d = &dense[k];
        let n = Vec2::new(-d.t.y, d.t.x) * sign;
        let dr = (d.dr * w.scale).clamp(-0.95, 0.95);
        let c = (1.0 - dr * dr).sqrt();
        (d.p + (n * c - d.t * dr) * r[k], unit_or_zero(d.t * c + n * dr))
    };
    // Slots: (disk, angle on its circle, direction of travel there).
    let mut slots: Vec<(usize, f64, Vec2)> = Vec::with_capacity(2 * m + 2 * CAP_POINTS);
    for k in 0..m {
        let (p, dir) = side(k, 1.0);
        slots.push((disk_of[k], union.angle(disk_of[k], p), dir));
    }
    // A cap turns clockwise (decreasing angles, y up) from one side's end to the other's.
    let cap = |slots: &mut Vec<(usize, f64, Vec2)>, disk: usize, from: f64, to: f64| {
        let sweep = match -(from - to).rem_euclid(TAU) {
            s if s.abs() < 1e-9 => -PI,
            s => s,
        };
        for s in 1..=CAP_POINTS {
            let a = from + sweep * s as f64 / (CAP_POINTS + 1) as f64;
            slots.push((disk, a, Vec2::new(a.sin(), -a.cos())));
        }
    };
    let (end_left, end_right) = (side(m - 1, 1.0).0, side(m - 1, -1.0).0);
    let end = disk_of[m - 1];
    cap(&mut slots, end, union.angle(end, end_left), union.angle(end, end_right));
    for k in (0..m).rev() {
        let (p, dir) = side(k, -1.0);
        slots.push((disk_of[k], union.angle(disk_of[k], p), -dir));
    }
    let (start_left, start_right) = (side(0, 1.0).0, side(0, -1.0).0);
    let start = disk_of[0];
    cap(&mut slots, start, union.angle(start, start_right), union.angle(start, start_left));

    let mut pts: Vec<Point> = slots.iter().map(|&(i, a, _)| union.point(i, a)).collect();
    let hidden: Vec<bool> = slots.iter().map(|&(i, a, _)| union.hidden(i, a)).collect();
    relocate(&mut pts, &hidden, &slots, &union);
    // Analytic directions only hold where a point stayed where it was.
    let dirs: Vec<Option<Vec2>> = slots.iter().zip(&hidden).map(|(s, &h)| (!h && s.2.hypot() > 0.5).then_some(s.2)).collect();
    (pts, dirs)
}

/// Moves every run of hidden slots onto the boundary between the visible slots around it, spread
/// evenly by length. A run is left as it is when the walk does not get from one end to the other
/// (the ends lie on different boundary loops) or gets there only by a detour more than twice the
/// run's own length (around some other part of the stroke): a run left inside the stroke only
/// adds overlap, while a wrong path would add or cut away ink.
fn relocate(pts: &mut [Point], hidden: &[bool], slots: &[(usize, f64, Vec2)], union: &boundary::Union) {
    const ARC_STEP: f64 = 2.0 * std::f64::consts::PI / 180.0;
    let n = pts.len();
    let Some(first_visible) = (0..n).find(|&i| !hidden[i]) else { return };
    let mut k = 0;
    while k < n {
        let a = (first_visible + k) % n;
        if !hidden[(a + 1) % n] {
            k += 1;
            continue;
        }
        let len = (1..n).take_while(|&s| hidden[(a + s) % n]).count();
        let b = (a + len + 1) % n;
        // The raw run's own length, to tell a walk along the run from a detour around something.
        let raw_len: f64 = (0..=len).map(|s| (pts[(a + s + 1) % n] - pts[(a + s) % n]).hypot()).sum();
        let Some(path) = union.walk((slots[a].0, slots[a].1), (slots[b].0, slots[b].1), ARC_STEP) else {
            k += len + 1;
            continue;
        };
        let cumulative: Vec<f64> = std::iter::once(0.0)
            .chain(path.windows(2).scan(0.0, |acc, w| {
                *acc += (w[1] - w[0]).hypot();
                Some(*acc)
            }))
            .collect();
        let total = *cumulative.last().unwrap();
        if total > 2.0 * raw_len + 1e-9 {
            k += len + 1;
            continue;
        }
        for t in 0..len {
            let s = total * (t + 1) as f64 / (len + 1) as f64;
            let j = cumulative.partition_point(|&c| c < s).clamp(1, path.len() - 1);
            let span = (cumulative[j] - cumulative[j - 1]).max(1e-300);
            pts[(a + 1 + t) % n] = path[j - 1].lerp(path[j], ((s - cumulative[j - 1]) / span).clamp(0.0, 1.0));
        }
        k += len + 1;
    }
}

fn unit_or_zero(v: Vec2) -> Vec2 {
    if v.hypot() > 1e-12 { v / v.hypot() } else { Vec2::ZERO }
}

/// Without an analytic direction, a knot whose two sides turn less than this is treated as smooth.
const SMOOTH: f64 = 2.0 * std::f64::consts::PI / 180.0;

/// Fits one cubic per knot interval through a polyline (in travel order; `knots` increasing),
/// appending them to `path`. `dirs[i]` is the analytic travel direction at point i where known
/// (points left untouched by the cleanup); elsewhere tangents come from the polyline: shared at
/// smooth knots, one-sided at corners.
fn fit_polyline(path: &mut BezPath, pts: &[Point], knots: &[usize], dirs: &[Option<Vec2>]) {
    // Unit direction from point i towards the first point in direction `step` that is at least
    // `reach` away (the nearest distinct point when `reach` is tiny).
    let toward = |i: usize, step: isize, reach: f64| -> Option<Vec2> {
        let mut j = i as isize + step;
        let mut last = None;
        while j >= 0 && (j as usize) < pts.len() {
            let v = pts[j as usize] - pts[i];
            if v.hypot() > 1e-9 {
                last = Some(v / v.hypot());
                if v.hypot() >= reach {
                    return last;
                }
            }
            j += step;
        }
        last
    };
    // Polyline length between two indices.
    let length = |a: usize, b: usize| pts[a..=b].windows(2).map(|w| (w[1] - w[0]).hypot()).sum::<f64>();
    let tangents = |pos: usize, analytic: bool| -> (Vec2, Vec2) {
        // (arriving, leaving) directions at knot number `pos`.
        let k = knots[pos];
        if analytic && let Some(d) = dirs[k] {
            return (d, d);
        }
        // A point moved by the cleanup sits at a cusp (or on another circle's boundary): use
        // one-sided directions, looking a little further along each interval so other displaced
        // points near the cusp do not skew them.
        let inc = toward(k, -1, 0.0).map(|v| -v);
        let out = toward(k, 1, 0.0);
        let before = (pos > 0).then(|| 0.15 * length(knots[pos - 1], k));
        let after = (pos + 1 < knots.len()).then(|| 0.15 * length(k, knots[pos + 1]));
        let a = before.and_then(|r| toward(k, -1, r)).map(|v| -v).or(inc);
        let b = after.and_then(|r| toward(k, 1, r)).or(out);
        match (a, b) {
            // Nearly straight through: share one tangent to keep the joint smooth.
            (Some(a), Some(b)) if a.cross(b).atan2(a.dot(b)).abs() < SMOOTH => {
                let s = unit_or_zero(a + b);
                (s, s)
            }
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => (a, a),
            (None, Some(b)) => (b, b),
            (None, None) => (Vec2::new(1.0, 0.0), Vec2::new(1.0, 0.0)),
        }
    };
    for pos in 0..knots.len().saturating_sub(1) {
        let (k0, k1) = (knots[pos], knots[pos + 1]);
        let span = &pts[k0..=k1];
        let mut cubic = fit_cubic(span, tangents(pos, true).1, tangents(pos + 1, true).0);
        // An analytic direction holds at its point only; after a quick change of radius the side
        // soon bends away from it, and a long cubic held to it bows out. If the cubic misses its
        // points (by more than 0.001 pt), try directions taken from the polyline and keep the
        // closer fit.
        let error = max_distance(&cubic, span);
        if (dirs[k0].is_some() || dirs[k1].is_some()) && error > 1e-3 {
            let alternative = fit_cubic(span, tangents(pos, false).1, tangents(pos + 1, false).0);
            if max_distance(&alternative, span) < error {
                cubic = alternative;
            }
        }
        path.curve_to(cubic.p1, cubic.p2, cubic.p3);
    }
}

/// Largest distance from the points to the cubic.
fn max_distance(c: &CubicBez, pts: &[Point]) -> f64 {
    pts.iter().map(|&p| c.nearest(p, 1e-9).distance_sq).fold(0.0, f64::max).sqrt()
}

/// Least-squares cubic through `pts` with fixed endpoints and unit end tangents `t0` (leaving
/// the start) and `t1` (arriving at the end), with Newton reparametrization rounds.
fn fit_cubic(pts: &[Point], t0: Vec2, t1: Vec2) -> CubicBez {
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let chord = (b - a).hypot();
    let fallback = || CubicBez::new(a, a + t0 * (chord / 3.0), b - t1 * (chord / 3.0), b);
    if pts.len() <= 2 || chord < 1e-9 {
        return fallback();
    }
    // Chord-length parameters.
    let mut u = vec![0.0; pts.len()];
    for i in 1..pts.len() {
        u[i] = u[i - 1] + (pts[i] - pts[i - 1]).hypot();
    }
    let total = u[pts.len() - 1];
    if total < 1e-12 {
        return fallback();
    }
    u.iter_mut().for_each(|v| *v /= total);

    let mut best = fallback();
    for _ in 0..3 {
        let (mut c11, mut c12, mut c22, mut x1, mut x2) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (p, &t) in pts.iter().zip(&u) {
            let mt = 1.0 - t;
            let (b0, b1, b2, b3) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
            let (a1, a2) = (t0 * b1, -t1 * b2);
            let rest = *p - (a.to_vec2() * (b0 + b1) + b.to_vec2() * (b2 + b3)).to_point();
            c11 += a1.dot(a1);
            c12 += a1.dot(a2);
            c22 += a2.dot(a2);
            x1 += a1.dot(rest);
            x2 += a2.dot(rest);
        }
        let det = c11 * c22 - c12 * c12;
        let (alpha, beta) = if det.abs() > 1e-18 {
            ((x1 * c22 - x2 * c12) / det, (c11 * x2 - c12 * x1) / det)
        } else {
            (chord / 3.0, chord / 3.0)
        };
        // Handles pointing backwards or absurdly long would create loops and spikes.
        let ok = |h: f64| h > 1e-6 * chord && h < 2.0 * chord;
        let (alpha, beta) = if ok(alpha) && ok(beta) { (alpha, beta) } else { (chord / 3.0, chord / 3.0) };
        best = CubicBez::new(a, a + t0 * alpha, b - t1 * beta, b);
        // Newton step per parameter: minimize |B(u) − p|.
        let d1 = best.deriv();
        let d2 = d1.deriv();
        for (p, t) in pts.iter().zip(u.iter_mut()) {
            let diff = best.eval(*t) - *p;
            let (f1, f2) = (d1.eval(*t).to_vec2(), d2.eval(*t).to_vec2());
            let den = f1.dot(f1) + diff.dot(f2);
            if den.abs() > 1e-18 {
                *t = (*t - diff.dot(f1) / den).clamp(0.0, 1.0);
            }
        }
    }
    best
}

/// A full circle (4 cubics), clockwise with y up.
fn circle(path: &mut BezPath, c: Point, r: f64) {
    use std::f64::consts::{FRAC_PI_2, FRAC_PI_8};
    let k = 4.0 / 3.0 * FRAC_PI_8.tan() * r;
    path.move_to(c + unit(0.0) * r);
    for i in 0..4 {
        // Decreasing angles: clockwise with y up.
        let (s, e) = (-FRAC_PI_2 * i as f64, -FRAC_PI_2 * (i + 1) as f64);
        path.curve_to(c + unit(s) * r - perp(s) * k, c + unit(e) * r + perp(e) * k, c + unit(e) * r);
    }
    path.close_path();
}

fn unit(a: f64) -> Vec2 {
    Vec2::new(a.cos(), a.sin())
}

/// Counter-clockwise tangent of the circle at angle `a`.
fn perp(a: f64) -> Vec2 {
    Vec2::new(-a.sin(), a.cos())
}

/// Signed area of a path (kurbo convention: positive for clockwise with y down, i.e.
/// counter-clockwise with y up).
pub fn signed_area(path: &BezPath) -> f64 {
    kurbo::Shape::area(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::{PathEl, Shape};
    use scriptoria_skeleton::Piece;
    use scriptoria_testkit::metrics::compare;

    /// A skeleton with one piece through the given (x, y, r), straight centerline between them.
    fn skeleton(pts: &[(f64, f64, f64)]) -> Skeleton {
        let samples: Vec<Sample> = pts.iter().map(|&(x, y, r)| Sample { p: Point::new(x, y), r }).collect();
        let spine = samples
            .windows(2)
            .map(|w| CubicBez::new(w[0].p, w[0].p.lerp(w[1].p, 1.0 / 3.0), w[0].p.lerp(w[1].p, 2.0 / 3.0), w[1].p))
            .collect();
        Skeleton { pieces: vec![Piece { samples, spine }], quality: Default::default() }
    }

    /// The exact shape a skeleton describes: the union of its circles swept along the centerline.
    fn swept_union(plan: &Plan, w: &WeightParams) -> BezPath {
        let mut out = BezPath::new();
        for piece in &plan.pieces {
            for s in &piece.dense {
                out.extend(kurbo::Circle::new(s.p, w.radius(s.r)).path_elements(1e-4));
            }
        }
        out
    }

    fn kinds(path: &BezPath) -> Vec<u8> {
        path.elements()
            .iter()
            .map(|e| match e {
                PathEl::MoveTo(_) => 0,
                PathEl::LineTo(_) => 1,
                PathEl::QuadTo(..) => 2,
                PathEl::CurveTo(..) => 3,
                PathEl::ClosePath => 4,
            })
            .collect()
    }

    fn wave() -> Skeleton {
        skeleton(&(0..12).map(|i| (i as f64 * 0.5, 0.3 * (i as f64 * 0.8).sin(), 0.15 + 0.04 * i as f64 / 11.0)).collect::<Vec<_>>())
    }

    /// V turn sharper than the radius, a hairpin, and a stroke starting with a big blob.
    fn hard_cases() -> Vec<(&'static str, Skeleton)> {
        vec![
            ("v-turn", skeleton(&[(0.0, 0.0, 0.3), (1.0, 0.0, 0.3), (2.0, 0.0, 0.3), (1.2, 0.5, 0.3), (0.4, 1.0, 0.3)])),
            ("hairpin", skeleton(&[(0.0, 0.0, 0.15), (2.0, 0.0, 0.15), (2.2, 0.2, 0.15), (2.0, 0.4, 0.15), (0.0, 0.4, 0.15)])),
            ("blob", skeleton(&[(0.0, 0.0, 0.45), (0.2, 0.0, 0.15), (1.5, 0.1, 0.15), (3.0, 0.0, 0.12)])),
            ("wave", wave()),
            // A pen-down stutter (forwards 0.07, back, forwards again with r ≈ 0.12), from a real stroke.
            (
                "stutter",
                skeleton(&[
                    (0.0718, 0.8928, 0.1200),
                    (0.1454, 0.9057, 0.1063),
                    (0.0756, 0.8925, 0.1224),
                    (0.0747, 0.8925, 0.1221),
                    (0.1686, 0.8953, 0.1454),
                    (0.4370, 0.9186, 0.1698),
                    (1.0916, 0.9303, 0.1425),
                ]),
            ),
        ]
    }

    #[test]
    fn topology_is_weight_independent() {
        let plan = plan(&wave(), &PlanParams::default());
        let thin = expand(&plan, &WeightParams::scaled(0.3));
        let bold = expand(&plan, &WeightParams::scaled(2.5));
        assert_eq!(kinds(&thin), kinds(&bold));
        let m = plan.pieces[0].knots.len();
        // Two caps of 2 cubics each + both sides with m − 1 cubics.
        assert_eq!(kinds(&thin).iter().filter(|&&k| k == 3).count(), 2 * (m - 1) + 4);
    }

    #[test]
    fn clockwise_with_y_up_starting_at_pen_down() {
        let sk = wave();
        let path = expand(&plan(&sk, &PlanParams::default()), &WeightParams::default());
        // Clockwise with y up ⇒ negative kurbo area.
        assert!(signed_area(&path) < 0.0, "area {}", signed_area(&path));
        let PathEl::MoveTo(start) = path.elements()[0] else { panic!() };
        let s0 = sk.pieces[0].samples[0];
        assert!(((start - s0.p).hypot() - s0.r).abs() < 1e-9);
        // The start point is on the writer's left (y up).
        let t = sk.pieces[0].samples[1].p - s0.p;
        assert!(t.cross(start - s0.p) > 0.0);
    }

    #[test]
    fn area_grows_with_weight() {
        let plan = plan(&wave(), &PlanParams::default());
        let areas: Vec<f64> =
            [0.3, 0.6, 1.0, 1.5, 2.5].iter().map(|&k| signed_area(&expand(&plan, &WeightParams::scaled(k))).abs()).collect();
        assert!(areas.windows(2).all(|w| w[1] > w[0]), "{areas:?}");
    }

    /// The regenerated outline must match the swept union of circles: no spikes, no loops,
    /// at several weights, including sharp turns and blobs.
    #[test]
    fn matches_swept_circles_without_burrs() {
        use scriptoria_testkit::report::{Panel, Report, Svg, color};
        let mut report = Report::new("outline-unit-swept", "outline · unit · regenerated vs swept circles");
        let mut failures = Vec::new();
        for (name, sk) in hard_cases() {
            let plan = plan(&sk, &PlanParams::default());
            let section = report.section(name, "gray: union of the skeleton's circles · red: regenerated outline");
            for weight in [0.5, 1.0, 2.0] {
                let w = WeightParams::scaled(weight);
                let got = expand(&plan, &w);
                let reference = swept_union(&plan, &w);
                let cmp = compare(&reference, &got);
                let r = sk.max_radius() * weight;
                let bad = cmp.iou <= 0.97 || cmp.hausdorff >= 0.15 * r || cmp.edge_ratio >= 1.08;
                if bad {
                    failures.push(format!("{name} ×{weight}: hausdorff {:.3} r · {cmp:?}", cmp.hausdorff / r));
                }
                let view = reference.bounding_box().union(got.bounding_box()).inflate(0.1, 0.1);
                let mut svg = Svg::new(view, 360.0);
                svg.path(&reference, color::ORIGINAL, "none", 0.0);
                svg.path(&got, color::REGENERATED_FILL, color::REGENERATED, 1.0);
                for piece in &plan.pieces {
                    svg.polyline(&piece.dense.iter().map(|s| s.p).collect::<Vec<_>>(), color::SPINE, 1.0);
                }
                section.panels.push(Panel {
                    caption: format!("×{weight} · IoU {:.3} · Hausdorff {:.2} r · edges ×{:.2}", cmp.iou, cmp.hausdorff / r, cmp.edge_ratio),
                    svg: svg.render(),
                    flagged: bad,
                });
            }
        }
        report.write();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn single_sample_is_a_circle() {
        let sk = skeleton(&[(1.0, 1.0, 0.2)]);
        let path = expand(&plan(&sk, &PlanParams::default()), &WeightParams::default());
        assert!((path.area().abs() - std::f64::consts::PI * 0.04).abs() < 1e-3);
    }
}

