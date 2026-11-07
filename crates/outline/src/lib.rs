//! Regenerates stroke outlines from a skeleton, at any weight.
//!
//! Every piece of a stroke becomes one closed contour whose structure depends only on the
//! skeleton, never on the weight, so the outlines of all weights are compatible and can serve
//! directly as the masters of a variable font:
//!
//! ```text
//! left point at pen-down L₀ → left side L₁ … Lₘ → end cap (2 cubics) → Rₘ
//!                          → right side Rₘ₋₁ … R₀ → start cap (2 cubics) → back to L₀
//! ```
//!
//! "Left" is the writer's left in a y-up coordinate system, so the contour is **clockwise** with
//! y up (the TrueType outer-contour direction) and counter-clockwise in y-down page coordinates.
//! Left and right on-curve points come in pairs (Lᵢ ↔ Rᵢ) around the same centerline point, so
//! downstream tools can recover the skeleton.
//!
//! The contour fills the union of the skeleton's circles under the **non-zero** rule; it is not
//! that union's boundary. Where the stroke folds (the inner side of a turn, a kink, a circle that
//! swallows its neighbors) it overlaps itself, the way a variable font keeps overlapping contours:
//! computing the union's boundary instead would move points abruptly as they get covered, and the
//! weights in between, which a font makes by interpolating its masters point by point, would come
//! out wrong. How it is built:
//! 1. the centerline is sampled densely (in length and in turning), with explicit tangents and a
//!    fan of extra directions where the sides' directions jump (kinks, changes in the rate the
//!    radius changes), so the outer side wraps around the joint's circle;
//! 2. each sample contributes one point per side, on the envelope of the circle family (through
//!    the center on the inner side of a kink, see `contour`);
//! 3. knots are chosen where one cubic per interval no longer follows the stroke's edge within
//!    the precision at both ends of the weight range (`select_knots`), and one least-squares
//!    cubic is fitted per interval, depending continuously on the points (`fit_span`); the
//!    weights to keep as masters follow from how far the interpolation strays (`masters`).

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
    /// The weights (radius scales) the plan serves, thinnest and boldest (see [`PlanParams`]).
    pub weights: (f64, f64),
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

/// Largest angular step of a fan, and of the turn between samples along the centerline.
const FAN_STEP: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// The angle an arc of radius `r` may turn and still lie within `precision` of its chord
/// (r·(1 − cos θ/2) ≤ precision), at most a fan step: a turn of the sides up to this needs no fan,
/// and a fan turns this much per step.
fn arc_step(r: f64, precision: f64) -> f64 {
    if r <= precision { FAN_STEP } else { (2.0 * (1.0 - precision / r).acos()).min(FAN_STEP) }
}

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
    Plan { pieces, weights: params.weights }
}

fn plan_piece(samples: &[Sample], spine: &[CubicBez], params: &PlanParams) -> PiecePlan {
    let mut dense: Vec<DenseSample> = Vec::new();
    if spine.is_empty() {
        if let Some(s) = samples.first() {
            dense.push(DenseSample { p: s.p, r: s.r, t: Vec2::new(1.0, 0.0), dr: 0.0, s: 0.0 });
        }
        return PiecePlan { dense, knots: (0..samples.len().min(1)).collect() };
    }
    let rmax = samples.iter().map(|s| s.r).fold(0.0, f64::max);
    let bold = params.weights.1;
    let mut s_acc = 0.0;
    let mut prev_t: Option<Vec2> = None;
    let mut prev_dr = 0.0;
    for (i, seg) in spine.iter().enumerate() {
        let (r0, r1) = (samples[i].r, samples[i + 1].r);
        let len = seg.arclen(1e-4 * rmax);
        let dr = if len > 1e-9 * rmax { (r1 - r0) / len } else { 0.0 };
        // Spacing ≤ ¼ of the radius (not finer than for a twentieth of the largest radius).
        let n = (len / (0.25 * r0.min(r1).max(0.05 * rmax))).ceil().clamp(2.0, 400.0) as usize;
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
        // A fan at the joint with the previous segment, where the side directions jump (where the
        // centerline kinks, or where the radius starts changing at another rate: the envelope
        // leaves each circle at 90° + asin(dr) from the tangent) further than a chord across the
        // joint's circle can stand for at the boldest weight. Its samples turn the tangent and
        // blend the rate, so on each side the direction turns steadily from the incoming to the
        // outgoing one: an arc of the joint's circle on the outer side.
        let t_in = tangent(0.0);
        if let Some(t_prev) = prev_t {
            let a = t_prev.cross(t_in).atan2(t_prev.dot(t_in));
            let shift = |dr: f64| (dr * bold).clamp(-1.0, 1.0).asin();
            let jump = shift(dr) - shift(prev_dr);
            let turn = (a + jump).abs().max((a - jump).abs());
            let step = arc_step(r0 * bold, params.precision);
            if turn > step {
                let steps = (turn / step).ceil() as usize;
                // The joint with the incoming direction, then the fan, then (below) the outgoing one.
                dense.push(DenseSample { p: seg.p0, r: r0, t: t_prev, dr: prev_dr, s: s_acc });
                for k in 1..steps {
                    let f = k as f64 / steps as f64;
                    let ang = t_prev.y.atan2(t_prev.x) + a * f;
                    dense.push(DenseSample { p: seg.p0, r: r0, t: Vec2::new(ang.cos(), ang.sin()), dr: prev_dr + (dr - prev_dr) * f, s: s_acc });
                }
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
                let steps = (a.abs() / arc_step(r * bold, params.precision)).ceil().max(1.0) as usize;
                for k in 1..steps {
                    let ang = tp.y.atan2(tp.x) + a * k as f64 / steps as f64;
                    dense.push(DenseSample { p, r, t: Vec2::new(ang.cos(), ang.sin()), dr, s });
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

    let knots = select_knots(&dense, params);
    PiecePlan { dense, knots }
}

/// The fewest knots (dense indices, increasing, first and last included) such that between
/// consecutive knots one cubic per side fits each side ([`Ink::miss`] within `params.precision`)
/// at both weights of `params.weights`. Greedy: from each knot, the farthest next one that still
/// fits (found by bisection; a stretch that fits keeps fitting when shortened).
fn select_knots(dense: &[DenseSample], params: &PlanParams) -> Vec<usize> {
    let m = dense.len();
    if m <= 2 {
        return (0..m).collect();
    }
    // Both sides at both weights, as polylines indexed like `dense`, with headings, the ink there
    // and which points lie on its edge.
    let mut inks: Vec<Ink> = Vec::with_capacity(2);
    let mut sides: Vec<(Vec<Point>, Vec<Heading>, Vec<f64>, usize)> = Vec::with_capacity(4);
    for (i, w) in [params.weights.0, params.weights.1].into_iter().enumerate() {
        let weight = WeightParams::scaled(w);
        let (pts, heads) = contour(dense, &weight, params.weights);
        let ink = Ink::new(dense, &weight);
        let right = |k: usize| m + CAP_POINTS + (m - 1 - k);
        let left: Vec<Point> = pts[..m].to_vec();
        let right_pts: Vec<Point> = (0..m).map(|k| pts[right(k)]).collect();
        for (side, h) in [(left, heads[..m].to_vec()), (right_pts, (0..m).map(|k| (-heads[right(k)].0, heads[right(k)].1)).collect())] {
            let slack = (0..m).map(|k| ink.slack(side[k], weight.radius(dense[k].r), params.precision)).collect();
            sides.push((side, h, slack, i));
        }
        inks.push(ink);
    }
    let fits = |k0: usize, k1: usize| {
        sides.iter().all(|(pts, heads, slack, i)| {
            let span = &pts[k0..=k1];
            inks[*i].miss(&fit_span(span, &heads[k0..=k1]), span, &slack[k0..=k1]) <= params.precision
        })
    };
    let mut knots = vec![0];
    let mut k0 = 0;
    while k0 < m - 1 {
        let b = m - 1;
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
    knots
}

/// The union of a piece's circles at one weight: the ink a contour must fill and not leave.
struct Ink {
    disks: Vec<(Point, f64)>,
    /// Distances below this count as zero (scaled to the largest radius).
    tiny: f64,
}

impl Ink {
    fn new(dense: &[DenseSample], w: &WeightParams) -> Self {
        let disks: Vec<(Point, f64)> = dense.iter().map(|d| (d.p, w.radius(d.r))).collect();
        let tiny = 1e-6 * disks.iter().map(|d| d.1).fold(0.0, f64::max);
        Ink { disks, tiny }
    }

    /// Signed distance to the union's edge (negative inside).
    fn outside(&self, p: Point) -> f64 {
        self.disks.iter().map(|&(c, r)| (p - c).hypot() - r).fold(f64::INFINITY, f64::min)
    }

    /// How far a cubic through a contour point may pass from it (scaled by the miss allowed): a
    /// point on the ink's edge must be followed within the precision; one inside (pulled in
    /// towards its center, or covered by another circle) only roughly: within 3 % of its circle's
    /// radius, so the curve stays where other parts of the contour cover the ink too, and within
    /// half its depth in the ink, so the curve cannot cross the part of the contour along the edge
    /// nearby (which would cut a sliver out of the ink between them).
    fn slack(&self, p: Point, radius: f64, precision: f64) -> f64 {
        let depth = -self.outside(p);
        ((0.03 * radius).min(0.5 * depth) / precision).max(1.0)
    }

    /// How far a cubic misses the ink: how far it leaves it, or passes from the contour points it
    /// stands for, each distance divided by that point's slack ([`Ink::slack`]).
    fn miss(&self, c: &CubicBez, pts: &[Point], slack: &[f64]) -> f64 {
        let out = (0..=16).map(|j| self.outside(c.eval(j as f64 / 16.0))).fold(0.0, f64::max);
        pts.iter().zip(slack).map(|(&p, &s)| c.nearest(p, 1e-9).distance_sq.sqrt() / s).fold(out, f64::max)
    }
}


/// The outline at weight `w`: one closed contour per piece, in writing order.
pub fn expand(plan: &Plan, w: &WeightParams) -> BezPath {
    let mut path = BezPath::new();
    for piece in &plan.pieces {
        expand_piece(&mut path, piece, w, plan.weights);
    }
    path
}

/// Points of each cap between the two sides: a cap turns about half a circle, one point every 10°.
const CAP_POINTS: usize = 17;

/// One piece's outline at weight `w`: its contour ([`contour`]) fitted with one cubic per knot
/// interval (the plan's knots on both sides, and the middle of each cap).
fn expand_piece(path: &mut BezPath, piece: &PiecePlan, w: &WeightParams, range: (f64, f64)) {
    match piece.dense.len() {
        0 => {}
        1 => circle(path, piece.dense[0].p, w.radius(piece.dense[0].r)),
        _ => {
            let cubics = piece_cubics(piece, w, range);
            path.move_to(cubics[0].p0);
            for c in &cubics {
                path.curve_to(c.p1, c.p2, c.p3);
            }
            path.close_path();
        }
    }
}

/// The closed contour of a piece of at least two dense samples (its first point repeated at the
/// end), with its headings and its knots: the plan's on both sides, the middle of each cap, and
/// the end.
fn closed_contour(piece: &PiecePlan, w: &WeightParams, range: (f64, f64)) -> (Vec<Point>, Vec<Heading>, Vec<usize>) {
    let m = piece.dense.len();
    let (mut pts, mut heads) = contour(&piece.dense, w, range);
    let n = pts.len();
    let (end_widest, start_widest) = widest_caps(&piece.dense, range);
    let mut knots: Vec<usize> = piece.knots.clone();
    knots.extend(cap_knots(end_widest).into_iter().map(|o| m - 1 + o));
    knots.extend(piece.knots.iter().rev().map(|&k| m + CAP_POINTS + (m - 1 - k)));
    knots.extend(cap_knots(start_widest).into_iter().map(|o| 2 * m + CAP_POINTS - 1 + o));
    knots.push(n);
    pts.push(pts[0]);
    heads.push(heads[0]);
    (pts, heads, knots)
}

/// The cubics of a piece's outline (at least two dense samples), one per knot interval.
fn piece_cubics(piece: &PiecePlan, w: &WeightParams, range: (f64, f64)) -> Vec<CubicBez> {
    let (pts, heads, knots) = closed_contour(piece, w, range);
    knots.windows(2).map(|k| fit_span(&pts[k[0]..=k[1]], &heads[k[0]..=k[1]])).collect()
}

/// How far the outline that a variable font makes at weight `w` by interpolating each point
/// between its masters at weights `a` and `b` strays from the contour it should follow there:
/// for every cubic, how far it leaves the union of the piece's circles at `w` (extra ink), and
/// how far it passes from the contour's points in its interval that lie on that union's edge (a
/// notch). What lies inside the stroke (points pulled in towards their centers, points covered
/// by other circles, and any curve among them) cannot change the filled shape and is not counted.
pub fn interpolation_error(plan: &Plan, a: f64, b: f64, w: f64) -> f64 {
    let t = (w - a) / (b - a);
    let lerp = |p: &CubicBez, q: &CubicBez| CubicBez::new(p.p0.lerp(q.p0, t), p.p1.lerp(q.p1, t), p.p2.lerp(q.p2, t), p.p3.lerp(q.p3, t));
    let weight = WeightParams::scaled(w);
    let mut worst: f64 = 0.0;
    for piece in plan.pieces.iter().filter(|p| p.dense.len() >= 2) {
        let ink = Ink::new(&piece.dense, &weight);
        let (ca, cb) = (piece_cubics(piece, &WeightParams::scaled(a), plan.weights), piece_cubics(piece, &WeightParams::scaled(b), plan.weights));
        let (pts, _, knots) = closed_contour(piece, &weight, plan.weights);
        for (i, k) in knots.windows(2).enumerate() {
            let c = lerp(&ca[i], &cb[i]);
            let edge: Vec<Point> = pts[k[0]..=k[1]].iter().copied().filter(|&p| ink.outside(p) > -ink.tiny).collect();
            worst = worst.max(ink.miss(&c, &edge, &vec![1.0; edge.len()]));
        }
    }
    worst
}

/// Deepest bisection of the weight range when choosing masters (at most 2⁴ intervals).
const MASTER_DEPTH: usize = 4;

/// The weights at which to keep the outline as masters of a variable font, so that interpolating
/// between consecutive ones stays within `tolerance` of the outline at every weight between them
/// (checked at the quarter points): the two ends of `range`, and between them as many as halving
/// the intervals that miss needs, up to [`MASTER_DEPTH`] halvings. A font may keep its own set
/// per glyph (intermediate regions).
pub fn masters(plan: &Plan, range: (f64, f64), tolerance: f64) -> Vec<f64> {
    fn split(plan: &Plan, a: f64, b: f64, tolerance: f64, depth: usize, out: &mut Vec<f64>) {
        let misses = [0.25, 0.5, 0.75].iter().any(|f| interpolation_error(plan, a, b, a + (b - a) * f) > tolerance);
        if misses && depth < MASTER_DEPTH {
            let mid = 0.5 * (a + b);
            split(plan, a, mid, tolerance, depth + 1, out);
            split(plan, mid, b, tolerance, depth + 1, out);
        } else {
            out.push(b);
        }
    }
    let mut out = vec![range.0];
    split(plan, range.0, range.1, tolerance, 0, &mut out);
    out
}

/// The contour's points at weight `w` (at least two dense samples), in slot order — the left
/// side's point of every sample, the end cap, the right side's points backwards, the start cap —
// with their headings. The slots are the same at every weight and every point moves continuously
/// with the weight, so outlines at a few weights interpolate well (the masters of a variable font).
///
/// The contour is not the boundary of the union of the samples' disks: it overlaps itself where
/// the stroke folds, and fills the union under the non-zero rule. Seen as a sum of cells (the
/// quadrilateral between two consecutive samples and their side points, the triangles of the
/// caps), it covers the union as long as no cell runs the wrong way round, which would cancel the
/// cover of another cell and leave a hole, unless other cells cover that place twice. Each side
/// point lies on the envelope of the circles, p + r·(n·cos a − t·sin a) with a = asin(dr) (n the
/// side's normal, t the tangent), which is where its circle touches the union's boundary. Where the
/// side runs backwards there (a swallowtail on the inner side of a tight turn, a stutter back and
/// forth along one circle), the stretch it runs back over is covered twice by the parts on either
/// side, so the inverted cells leave no hole; pulling those points in instead would cut the ink
/// wherever those parts are short. The exception is a fan turning the wrong way for a side (the
/// inner side of a kink), which would run backwards around a whole arc: its points between its
/// ends go through the center, as the inner join of a stroker does, blended in over the first fan
/// step so they move continuously with the weight.
fn contour(dense: &[DenseSample], w: &WeightParams, range: (f64, f64)) -> (Vec<Point>, Vec<Heading>) {
    use std::f64::consts::FRAC_PI_2;
    let m = dense.len();
    let r: Vec<f64> = dense.iter().map(|d| w.radius(d.r)).collect();
    // Angle a between a side's normal and its envelope point (the radius changes at rate dr·scale).
    let shift: Vec<f64> = dense.iter().map(|d| (d.dr * w.scale).clamp(-1.0, 1.0).asin()).collect();
    let same = |a: usize, b: usize| dense[a].p == dense[b].p;

    // One side (sign 1: the writer's left, y up; −1: the right), in writing order: points and
    // headings.
    let side = |sign: f64| -> (Vec<Point>, Vec<Heading>) {
        let (dir, travel): (Vec<Vec2>, Vec<Vec2>) = (0..m)
            .map(|k| {
                let (t, a) = (dense[k].t, shift[k]);
                let n = Vec2::new(-t.y, t.x) * sign;
                (n * a.cos() - t * a.sin(), t * a.cos() + n * a.sin())
            })
            .unzip();
        let mut reach = r.clone();
        let mut k = 0;
        while k + 1 < m {
            if !same(k, k + 1) {
                k += 1;
                continue;
            }
            // A fan: samples k..=j around one center. The right way round is clockwise for the left
            // side and counter-clockwise for the right one (y up).
            let mut j = k + 1;
            while j + 1 < m && same(j, j + 1) {
                j += 1;
            }
            let turn: f64 = (k..j).map(|i| dir[i].cross(dir[i + 1]).atan2(dir[i].dot(dir[i + 1]))).sum();
            let keep = (1.0 - turn * sign / FAN_STEP).clamp(0.0, 1.0);
            for i in k + 1..j {
                reach[i] *= keep;
            }
            k = j;
        }
        let pts = (0..m).map(|k| dense[k].p + dir[k] * reach[k]).collect();
        // The travel direction on the envelope is trusted fully, and not at all once the point is
        // pulled in by a tenth of the radius.
        let trust = |k: usize| if r[k] > 0.0 { ((reach[k] / r[k] - 0.9) / 0.1).clamp(0.0, 1.0) } else { 1.0 };
        let heads = (0..m).map(|k| (travel[k], trust(k))).collect();
        (pts, heads)
    };
    let (left, left_travel) = side(1.0);
    let (right, right_travel) = side(-1.0);

    let mut pts: Vec<Point> = Vec::with_capacity(2 * m + 2 * CAP_POINTS);
    let mut heads: Vec<Heading> = Vec::with_capacity(2 * m + 2 * CAP_POINTS);
    // A cap turns clockwise (y up) around its circle from one side's envelope point to the other's,
    // through `ahead` (the tangent at the end, its opposite at the start), `half` either way (see
    // `widest_caps`). Its points sit at fixed angles, spread over the widest the cap gets within
    // the plan's weights, those beyond its current reach waiting at its ends: so each point only
    // moves out with the radius, linearly in the weight, instead of turning with the cap's reach
    // (which changes fastest just where a circle starts to swallow its neighbor).
    let cap = |pts: &mut Vec<Point>, heads: &mut Vec<Heading>, k: usize, ahead: Vec2, half: f64, widest: f64| {
        let ahead = ahead.y.atan2(ahead.x);
        let widest = widest.max(half);
        for s in 1..=CAP_POINTS {
            let a = ahead + (widest - 2.0 * widest * s as f64 / (CAP_POINTS + 1) as f64).clamp(-half, half);
            pts.push(dense[k].p + unit(a) * r[k]);
            heads.push((Vec2::new(a.sin(), -a.cos()), 1.0));
        }
    };
    let (end, start) = (m - 1, 0);
    let (end_widest, start_widest) = widest_caps(dense, range);
    pts.extend(&left);
    heads.extend(&left_travel);
    cap(&mut pts, &mut heads, end, dense[end].t, FRAC_PI_2 + shift[end], end_widest);
    pts.extend(right.iter().rev());
    heads.extend(right_travel.iter().rev().map(|&(d, c)| (-d, c)));
    cap(&mut pts, &mut heads, start, -dense[start].t, FRAC_PI_2 - shift[start], start_widest);
    (pts, heads)
}

/// The widest half-angles the end and the start cap reach within the weights `range` (a cap spans
/// twice that around the tangent): a quarter turn where the radius stays, more where it grows
/// towards the cap, less where it shrinks.
fn widest_caps(dense: &[DenseSample], range: (f64, f64)) -> (f64, f64) {
    use std::f64::consts::FRAC_PI_2;
    let m = dense.len();
    let shift = |k: usize, scale: f64| (dense[k].dr * scale).clamp(-1.0, 1.0).asin();
    let end = (FRAC_PI_2 + shift(m - 1, range.0)).max(FRAC_PI_2 + shift(m - 1, range.1));
    let start = (FRAC_PI_2 - shift(0, range.0)).max(FRAC_PI_2 - shift(0, range.1));
    (end, start)
}

/// Knots inside a cap, as offsets (1 to `CAP_POINTS`) from the side point before it: one cubic
/// per 112.5° of its widest span (a cubic follows such an arc within 0.1 % of its radius), at
/// least two.
fn cap_knots(widest_half: f64) -> Vec<usize> {
    let cubics = ((2.0 * widest_half / (0.625 * std::f64::consts::PI)).ceil() as usize).clamp(2, 4);
    (1..cubics).map(|j| (j * (CAP_POINTS + 1) + cubics / 2) / cubics).collect()
}

fn unit_or_zero(v: Vec2) -> Vec2 {
    if v.hypot() > 1e-12 { v / v.hypot() } else { Vec2::ZERO }
}

/// A point's travel direction along the contour where known from the geometry, and how far it can
/// be trusted: 1 on the envelope and on the caps, falling to 0 as a side point is pulled in
/// towards its center (it then sits at a cusp of the side, where that direction means nothing).
type Heading = (Vec2, f64);

/// How far into a span, as a fraction of its length, the polyline's own direction at either end is
/// read (so points crowded around a cusp do not skew it).
const LOOK: f64 = 0.15;

/// One cubic through a span of the polyline (and its points' headings), with the span's ends,
/// leaving and arriving along directions blended from the ends' headings and the polyline's own
/// direction there. A heading holds at its point only, so it counts only as far as it is trusted,
/// its neighbor in the span is trusted too (next to a point pulled in, the side turns a corner at
/// the end), and it agrees with the polyline (fully when parallel, not at all from 60° apart: the
/// side then turns too fast near its end for one cubic to keep that direction). Everything here
/// depends continuously on the points (no thresholds, no choices between alternatives), so as a
/// contour moves continuously with the weight its cubics do too, and the masters of a variable
/// font interpolate.
fn fit_span(span: &[Point], heads: &[Heading]) -> CubicBez {
    let n = span.len();
    let len: f64 = span.windows(2).map(|w| (w[1] - w[0]).hypot()).sum();
    let leave = unit_or_zero(point_along(span.iter().copied(), LOOK * len) - span[0]);
    let arrive = unit_or_zero(span[n - 1] - point_along(span.iter().rev().copied(), LOOK * len));
    let blend = |(d, c): Heading, neighbor: f64, own: Vec2| {
        let agree = ((d.dot(own) - 0.5) / 0.5).clamp(0.0, 1.0);
        let c = c.min(neighbor) * if own == Vec2::ZERO { 1.0 } else { agree };
        let v = unit_or_zero(d * c + own * (1.0 - c));
        if v != Vec2::ZERO { v } else if own != Vec2::ZERO { own } else { d }
    };
    let t0 = blend(heads[0], heads[1.min(n - 1)].1, leave);
    let t1 = blend(heads[n - 1], heads[n.saturating_sub(2)].1, arrive);
    fit_cubic(span, t0, t1)
}

/// The point `dist` along a polyline from its first point (its last point if it is shorter).
fn point_along(mut pts: impl Iterator<Item = Point>, dist: f64) -> Point {
    let Some(mut prev) = pts.next() else { return Point::ZERO };
    let mut left = dist;
    for p in pts {
        let l = (p - prev).hypot();
        if l > 0.0 && l >= left {
            return prev.lerp(p, left / l);
        }
        left -= l;
        prev = p;
    }
    prev
}

/// Least-squares cubic through `pts` with fixed endpoints and unit end tangents `t0` (leaving
/// the start) and `t1` (arriving at the end), with Gauss-Newton reparametrization rounds (no second-derivative term, whose sign can flip and
/// throw a parameter to the other end: the fit stays continuous in the points).
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
        // A faint pull of both handles towards a third of the chord: where the points leave the
        // handles undetermined (a straight span with tangents along it, where they only change
        // the speed along the line), they take that value instead of swinging with every rounding;
        // elsewhere it changes nothing measurable.
        let ridge = 1e-3 * (c11 + c22);
        let (c11, c22) = (c11 + ridge, c22 + ridge);
        let (x1, x2) = (x1 + ridge * chord / 3.0, x2 + ridge * chord / 3.0);
        let det = c11 * c22 - c12 * c12;
        let (alpha, beta) = if det.abs() > 1e-18 {
            ((x1 * c22 - x2 * c12) / det, (c11 * x2 - c12 * x1) / det)
        } else {
            (chord / 3.0, chord / 3.0)
        };
        // Handles pointing backwards or absurdly long would create loops and spikes: kept within
        // bounds (continuously, so the cubic still follows its points as they move).
        let bound = |h: f64| h.clamp(1e-6 * chord, 2.0 * chord);
        let (alpha, beta) = (bound(alpha), bound(beta));
        best = CubicBez::new(a, a + t0 * alpha, b - t1 * beta, b);
        // Gauss-Newton step per parameter: minimize |B(u) − p|.
        let d1 = best.deriv();
        for (p, t) in pts.iter().zip(u.iter_mut()) {
            let diff = best.eval(*t) - *p;
            let f1 = d1.eval(*t).to_vec2();
            let den = f1.dot(f1);
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
