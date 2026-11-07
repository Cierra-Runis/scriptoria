//! Global refinement of a piece's skeleton, by analysis by synthesis: circle centers, radii and
//! centerline handles are adjusted together so that every Goodnotes subpath's outline lies on the
//! boundary of the region swept along its own stretch of the skeleton.
//!
//! The local estimate (circles fitted to cap arcs, centerlines from the sides) is the starting
//! point. Robust nonlinear least squares (Levenberg–Marquardt on Huber-weighted residuals) then
//! corrects what local fitting gets wrong: oversized circles at degenerate ends, centerlines biased
//! by uneven sides, radii that the cap arcs alone do not pin down. A weak pull towards the starting
//! point keeps parameters that no outline point constrains where they were.
//!
//! Parameters, per sample i: `x, y, r` at `7i`; per centerline segment i (from sample i to i + 1):
//! the handles `p1 − p0` and `p2 − p3` at `7i + 3`. A residual involves one segment and its two
//! samples, i.e. 10 consecutive parameters, so the normal equations are banded.

use kurbo::{CubicBez, ParamCurve, ParamCurveArclen, PathSeg, Point, Vec2};

use crate::{Piece, Sample};

/// One subpath of the piece, explained by the piece's samples `first..=last`.
pub(crate) struct Target<'a> {
    pub segs: &'a [PathSeg],
    pub first: usize,
    pub last: usize,
}

const MAX_ITERATIONS: usize = 25;
/// Weight of the pull towards the starting point, per parameter (both are lengths, so it has no
/// unit).
const PROXIMAL: f64 = 0.1;
/// Huber threshold, as a fraction of the largest radius of the subpath's stretch: misfit beyond it
/// (outline stretches that are no swept circle at all) counts linearly, not squared. Relative to the
/// radius, so it does not depend on the drawing's scale. (A threshold from the residuals' own noise
/// scale, as in textbook M-estimation, stalls refinement from a bad start: the misfit is then
/// systematic, the median residual small, and the misfitting points end up with almost no weight.)
const HUBER: f64 = 0.05;
/// Outline points are taken about this far apart (× the radius of the stretch).
const SPACING: f64 = 0.4;

const STRIDE: usize = 7;
/// Half bandwidth of the normal equations (a residual spans 10 consecutive parameters).
const BAND: usize = 9;

struct OutlinePoint {
    x: Point,
    first: usize,
    last: usize,
    delta: f64,
}

/// Refines `piece` in place against the outlines of `targets`.
pub(crate) fn refine(piece: &mut Piece, targets: &[Target]) {
    let n = piece.samples.len();
    if n == 0 || piece.spine.len() + 1 != n {
        return;
    }
    let points = outline_points(piece, targets);
    if points.is_empty() {
        return;
    }
    let rmax = piece.samples.iter().map(|s| s.r).fold(0.0, f64::max).max(1e-12);
    let r_min = 1e-3 * rmax;
    let v0 = pack(piece);
    let mut v = v0.clone();
    let mut lambda = 1e-3;
    for _ in 0..MAX_ITERATIONS {
        let current = cost(&v, &v0, n, &points);
        let (a, g) = normal_equations(&v, &v0, n, &points);
        let rhs: Vec<f64> = g.iter().map(|x| -x).collect();
        let mut improved = false;
        for _ in 0..10 {
            let mut m = a.clone();
            for i in 0..m.n {
                let d = m.get(i, i);
                m.add(i, i, lambda * d);
            }
            let Some(step) = m.solve(&rhs) else {
                lambda *= 10.0;
                continue;
            };
            let mut candidate: Vec<f64> = v.iter().zip(&step).map(|(a, b)| a + b).collect();
            for i in 0..n {
                candidate[STRIDE * i + 2] = candidate[STRIDE * i + 2].max(r_min);
            }
            let c = cost(&candidate, &v0, n, &points);
            if c < current {
                improved = (current - c) > 1e-5 * current;
                v = candidate;
                lambda = (lambda * 0.3).max(1e-9);
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    let (samples, spine) = unpack(&v, n);
    piece.samples = samples;
    piece.spine = spine;
}

/// How far the outlines of `targets` are from the regions swept along their stretches of `piece`:
/// the mean absolute distance over points spread evenly along each outline, overall and per
/// target. The points depend only on the outlines, so different pieces can be compared.
pub(crate) fn fit_error(piece: &Piece, targets: &[Target]) -> (f64, Vec<f64>) {
    const POINTS: usize = 48;
    let (mut total, mut count) = (0.0, 0);
    let mut per_target = Vec::with_capacity(targets.len());
    for target in targets {
        let lengths: Vec<f64> = target.segs.iter().map(|s| s.arclen(1e-4)).collect();
        let outline: f64 = lengths.iter().sum();
        let mut sum = 0.0;
        let mut n = 0;
        for (seg, &len) in target.segs.iter().zip(&lengths) {
            let k = ((len / outline.max(1e-12) * POINTS as f64).round() as usize).max(1);
            for i in 0..k {
                let x = seg.eval((i as f64 + 0.5) / k as f64);
                sum += closest(x, &piece.samples, &piece.spine, target.first, target.last).res.abs();
                n += 1;
            }
        }
        per_target.push(sum / n.max(1) as f64);
        total += sum;
        count += n;
    }
    (total / count.max(1) as f64, per_target)
}

/// Points along every subpath outline, about `SPACING` radii apart (zero-length marker segments,
/// shorter than 1% of the radius, skipped).
fn outline_points(piece: &Piece, targets: &[Target]) -> Vec<OutlinePoint> {
    let mut out = Vec::new();
    for target in targets {
        let (rmin, rmax) = piece.samples[target.first..=target.last]
            .iter()
            .fold((f64::INFINITY, 0.0f64), |(lo, hi), s| (lo.min(s.r), hi.max(s.r)));
        let spacing = SPACING * rmin.max(0.1 * rmax);
        let delta = HUBER * rmax;
        for seg in target.segs {
            let len = seg.arclen(1e-4);
            if len < 0.01 * rmax {
                continue;
            }
            let count = ((len / spacing).ceil() as usize).clamp(1, 64);
            for i in 0..count {
                let u = (i as f64 + 0.5) / count as f64;
                out.push(OutlinePoint { x: seg.eval(u), first: target.first, last: target.last, delta });
            }
        }
    }
    out
}

fn pack(piece: &Piece) -> Vec<f64> {
    let n = piece.samples.len();
    let mut v = vec![0.0; STRIDE * (n - 1) + 3];
    for (i, s) in piece.samples.iter().enumerate() {
        v[STRIDE * i..STRIDE * i + 3].copy_from_slice(&[s.p.x, s.p.y, s.r]);
    }
    for (i, c) in piece.spine.iter().enumerate() {
        let (h0, h1) = (c.p1 - c.p0, c.p2 - c.p3);
        v[STRIDE * i + 3..STRIDE * i + 7].copy_from_slice(&[h0.x, h0.y, h1.x, h1.y]);
    }
    v
}

fn unpack(v: &[f64], n: usize) -> (Vec<Sample>, Vec<CubicBez>) {
    let samples: Vec<Sample> =
        (0..n).map(|i| Sample { p: Point::new(v[STRIDE * i], v[STRIDE * i + 1]), r: v[STRIDE * i + 2] }).collect();
    let spine = (0..n.saturating_sub(1))
        .map(|i| {
            let (a, b) = (samples[i].p, samples[i + 1].p);
            let h = &v[STRIDE * i + 3..STRIDE * i + 7];
            CubicBez::new(a, a + Vec2::new(h[0], h[1]), b + Vec2::new(h[2], h[3]), b)
        })
        .collect();
    (samples, spine)
}

/// The circle of the sweep that `x` is closest to (relative to its radius).
struct Hit {
    /// Distance to the circle's center minus its radius (the signed distance to the sweep).
    res: f64,
    /// The centerline segment and parameter of that circle; `None`: the single sample `first`.
    seg: Option<usize>,
    t: f64,
    /// Unit vector from the circle's center to `x`.
    u: Vec2,
}

fn closest(x: Point, samples: &[Sample], spine: &[CubicBez], first: usize, last: usize) -> Hit {
    let unit = |d: Vec2| if d.hypot() > 1e-12 { d / d.hypot() } else { Vec2::ZERO };
    if first == last {
        let s = samples[first];
        return Hit { res: (x - s.p).hypot() - s.r, seg: None, t: 0.0, u: unit(x - s.p) };
    }
    let mut best = (f64::INFINITY, first, 0.0);
    for j in first..last {
        let c = spine[j];
        let (r0, r1) = (samples[j].r, samples[j + 1].r);
        let f = |t: f64| (x - c.eval(t)).hypot() - (r0 + (r1 - r0) * t);
        // A grid fine enough to land in the right valley, then golden-section search in it.
        let poly = (c.p1 - c.p0).hypot() + (c.p2 - c.p1).hypot() + (c.p3 - c.p2).hypot();
        let steps = ((poly / (0.5 * r0.min(r1).max(1e-12))).ceil() as usize).clamp(4, 200);
        let h = 1.0 / steps as f64;
        let (mut bi, mut bv) = (0, f64::INFINITY);
        for i in 0..=steps {
            let value = f(i as f64 * h);
            if value < bv {
                (bi, bv) = (i, value);
            }
        }
        let (mut t, mut value) = golden(&f, (bi as f64 - 1.0).max(0.0) * h, ((bi + 1) as f64 * h).min(1.0));
        if bv <= value {
            (t, value) = (bi as f64 * h, bv);
        }
        if value < best.0 {
            best = (value, j, t);
        }
    }
    let (res, j, t) = best;
    Hit { res, seg: Some(j), t, u: unit(x - spine[j].eval(t)) }
}

/// Minimum of `f` on `[a, b]` by golden-section search: (argument, value).
fn golden(f: &impl Fn(f64) -> f64, mut a: f64, mut b: f64) -> (f64, f64) {
    const G: f64 = 0.618_033_988_749_895;
    let (mut c, mut d) = (b - G * (b - a), a + G * (b - a));
    let (mut fc, mut fd) = (f(c), f(d));
    for _ in 0..16 {
        if fc < fd {
            (b, d, fd) = (d, c, fc);
            c = b - G * (b - a);
            fc = f(c);
        } else {
            (a, c, fc) = (c, d, fd);
            d = a + G * (b - a);
            fd = f(d);
        }
    }
    let t = 0.5 * (a + b);
    (t, f(t))
}

/// The residual's derivatives: first parameter index and the values for the following ones.
fn jacobian_row(hit: &Hit, first: usize) -> (usize, [f64; 10], usize) {
    let u = hit.u;
    match hit.seg {
        None => (STRIDE * first, [-u.x, -u.y, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], 3),
        Some(j) => {
            let (t, s) = (hit.t, 1.0 - hit.t);
            let (b0, b1, b2, b3) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            let row = [
                -u.x * (b0 + b1),
                -u.y * (b0 + b1),
                -s,
                -u.x * b1,
                -u.y * b1,
                -u.x * b2,
                -u.y * b2,
                -u.x * (b2 + b3),
                -u.y * (b2 + b3),
                -t,
            ];
            (STRIDE * j, row, 10)
        }
    }
}

fn huber(res: f64, delta: f64) -> f64 {
    if res.abs() <= delta { 0.5 * res * res } else { delta * (res.abs() - 0.5 * delta) }
}

fn huber_weight(res: f64, delta: f64) -> f64 {
    if res.abs() <= delta { 1.0 } else { delta / res.abs() }
}

fn cost(v: &[f64], v0: &[f64], n: usize, points: &[OutlinePoint]) -> f64 {
    let (samples, spine) = unpack(v, n);
    let data: f64 = points.iter().map(|pt| huber(closest(pt.x, &samples, &spine, pt.first, pt.last).res, pt.delta)).sum();
    let prox: f64 = v.iter().zip(v0).map(|(a, b)| (a - b) * (a - b)).sum();
    data + 0.5 * PROXIMAL * PROXIMAL * prox
}

/// Gauss–Newton normal equations (Huber weights from the current residuals): JᵀWJ and JᵀWr,
/// including the pull towards the starting point.
fn normal_equations(v: &[f64], v0: &[f64], n: usize, points: &[OutlinePoint]) -> (Band, Vec<f64>) {
    let (samples, spine) = unpack(v, n);
    let size = v.len();
    let mut a = Band::new(size);
    let mut g = vec![0.0; size];
    for pt in points {
        let hit = closest(pt.x, &samples, &spine, pt.first, pt.last);
        let w = huber_weight(hit.res, pt.delta);
        let (base, row, len) = jacobian_row(&hit, pt.first);
        for p in 0..len {
            g[base + p] += w * row[p] * hit.res;
            for q in p..len {
                a.add(base + q, base + p, w * row[p] * row[q]);
            }
        }
    }
    let w2 = PROXIMAL * PROXIMAL;
    for i in 0..size {
        a.add(i, i, w2);
        g[i] += w2 * (v[i] - v0[i]);
    }
    (a, g)
}

/// A symmetric band matrix (lower half stored: entry (i, j) with j ≤ i ≤ j + BAND).
#[derive(Clone)]
struct Band {
    n: usize,
    a: Vec<f64>,
}

impl Band {
    fn new(n: usize) -> Self {
        Self { n, a: vec![0.0; n * (BAND + 1)] }
    }

    fn index(i: usize, j: usize) -> usize {
        debug_assert!(j <= i && i - j <= BAND);
        i * (BAND + 1) + (i - j)
    }

    fn get(&self, i: usize, j: usize) -> f64 {
        self.a[Self::index(i, j)]
    }

    fn add(&mut self, i: usize, j: usize, value: f64) {
        self.a[Self::index(i, j)] += value;
    }

    /// Solves A x = b by band Cholesky; `None` if A is not positive definite.
    fn solve(&self, b: &[f64]) -> Option<Vec<f64>> {
        let n = self.n;
        let mut l = vec![0.0; n * (BAND + 1)];
        for i in 0..n {
            let lo = i.saturating_sub(BAND);
            for j in lo..=i {
                let mut s = self.get(i, j);
                for k in lo.max(j.saturating_sub(BAND))..j {
                    s -= l[Self::index(i, k)] * l[Self::index(j, k)];
                }
                if i == j {
                    if !(s > 0.0 && s.is_finite()) {
                        return None;
                    }
                    l[Self::index(i, i)] = s.sqrt();
                } else {
                    l[Self::index(i, j)] = s / l[Self::index(j, j)];
                }
            }
        }
        let mut y = vec![0.0; n];
        for i in 0..n {
            let mut s = b[i];
            for k in i.saturating_sub(BAND)..i {
                s -= l[Self::index(i, k)] * y[k];
            }
            y[i] = s / l[Self::index(i, i)];
        }
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let mut s = y[i];
            for k in i + 1..(i + BAND + 1).min(n) {
                s -= l[Self::index(k, i)] * x[k];
            }
            x[i] = s / l[Self::index(i, i)];
        }
        Some(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capsule_outline;
    use crate::circle::Circle;
    use crate::split_segments;

    #[test]
    fn band_cholesky_solves() {
        // Tridiagonal (2, −1) system with a known solution.
        let n = 30;
        let mut a = Band::new(n);
        for i in 0..n {
            a.add(i, i, 2.0);
            if i > 0 {
                a.add(i, i - 1, -1.0);
            }
        }
        let x: Vec<f64> = (0..n).map(|i| (i as f64 * 0.37).sin()).collect();
        let b: Vec<f64> = (0..n)
            .map(|i| 2.0 * x[i] - if i > 0 { x[i - 1] } else { 0.0 } - if i + 1 < n { x[i + 1] } else { 0.0 })
            .collect();
        let got = a.solve(&b).unwrap();
        assert!(got.iter().zip(&x).all(|(g, w)| (g - w).abs() < 1e-12));
    }

    /// A wrong starting point (one radius 60% too large, one center off by half a radius) is
    /// pulled back to the circles the outline was drawn from.
    #[test]
    fn corrects_a_bad_start() {
        let truth: Vec<Circle> = (0..6)
            .map(|i| Circle { c: Point::new(i as f64 * 0.3, 0.1 * (i as f64).sin()), r: 0.12 + 0.02 * i as f64 })
            .collect();
        let mut path = kurbo::BezPath::new();
        for w in truth.windows(2) {
            path.extend(capsule_outline(w[0], w[1]));
        }
        let subs = split_segments(&path);
        let mut samples: Vec<Sample> = truth.iter().map(|c| Sample { p: c.c, r: c.r }).collect();
        samples[0].r *= 1.6;
        samples[3].p.y += 0.5 * samples[3].r;
        let spine = samples.windows(2).map(|w| crate::straight(w[0].p, w[1].p)).collect();
        let mut piece = Piece { samples, spine };
        let targets: Vec<Target> = subs.iter().enumerate().map(|(k, segs)| Target { segs, first: k, last: k + 1 }).collect();
        refine(&mut piece, &targets);
        for (got, want) in piece.samples.iter().zip(&truth) {
            assert!((got.p - want.c).hypot() < 0.02 * want.r, "center {got:?} vs {want:?}");
            assert!((got.r - want.r).abs() < 0.02 * want.r, "radius {got:?} vs {want:?}");
        }
    }
}
