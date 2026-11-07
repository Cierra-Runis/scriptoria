//! Circle fitting: an algebraic (Kasa) fit as the initial guess, refined by Levenberg–Marquardt
//! geometric (distance) least squares.

use kurbo::{Point, Vec2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub c: Point,
    pub r: f64,
}

impl Circle {
    /// Signed distance from a point to the circle (positive outside).
    pub fn signed_distance(&self, p: Point) -> f64 {
        (p - self.c).hypot() - self.r
    }

    /// RMS distance of the points to the circle, divided by the radius.
    pub fn relative_rms(&self, pts: &[Point]) -> f64 {
        if pts.is_empty() || self.r <= 0.0 {
            return f64::INFINITY;
        }
        let ss: f64 = pts.iter().map(|&p| self.signed_distance(p).powi(2)).sum();
        (ss / pts.len() as f64).sqrt() / self.r
    }
}

/// Fits a circle. `None` with fewer than 3 points or (nearly) collinear points.
pub fn fit(pts: &[Point]) -> Option<Circle> {
    let init = kasa(pts)?;
    let circle = refine(pts, init);
    // Nearly collinear points fit huge circles: treat them as degenerate. Cap points usually span
    // about half a circle, so the radius stays below 1.5 × their spread (true for arcs ≥ ~40°).
    let spread = pts.iter().flat_map(|a| pts.iter().map(move |b| (*a - *b).hypot())).fold(0.0, f64::max);
    (circle.r.is_finite() && circle.r <= spread * 1.5).then_some(circle)
}

/// Kasa algebraic fit: minimizes Σ(x² + y² + Dx + Ey + F)², centered on the centroid for stability.
fn kasa(pts: &[Point]) -> Option<Circle> {
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let mean = pts.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2()) / n;
    let (mut sxx, mut syy, mut sxy, mut sx, mut sy, mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (x, y) = (p.x - mean.x, p.y - mean.y);
        let z = x * x + y * y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
        sx += x;
        sy += y;
        sxz += x * z;
        syz += y * z;
        sz += z;
    }
    let [d, e, f] = solve3([[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]], [-sxz, -syz, -sz])?;
    let (cx, cy) = (-d / 2.0, -e / 2.0);
    let rr = cx * cx + cy * cy - f;
    (rr > 0.0 && rr.is_finite()).then(|| Circle { c: Point::new(cx + mean.x, cy + mean.y), r: rr.sqrt() })
}

/// Levenberg–Marquardt on the residuals dᵢ = |pᵢ − c| − r.
fn refine(pts: &[Point], mut circle: Circle) -> Circle {
    let cost = |c: &Circle| pts.iter().map(|&p| c.signed_distance(p).powi(2)).sum::<f64>();
    let mut current = cost(&circle);
    let mut lambda = 1e-3;
    for _ in 0..50 {
        // JᵀJ and Jᵀr
        let mut jtj = [[0.0; 3]; 3];
        let mut jtr = [0.0; 3];
        for &p in pts {
            let v = p - circle.c;
            let d = v.hypot();
            if d < 1e-12 {
                continue;
            }
            let res = d - circle.r;
            let j = [-v.x / d, -v.y / d, -1.0];
            for a in 0..3 {
                jtr[a] += j[a] * res;
                for b in 0..3 {
                    jtj[a][b] += j[a] * j[b];
                }
            }
        }
        let mut improved = false;
        for _ in 0..8 {
            let mut m = jtj;
            for (a, row) in m.iter_mut().enumerate() {
                row[a] *= 1.0 + lambda;
            }
            let Some(step) = solve3(m, [-jtr[0], -jtr[1], -jtr[2]]) else { break };
            let cand = Circle { c: Point::new(circle.c.x + step[0], circle.c.y + step[1]), r: (circle.r + step[2]).abs() };
            let c = cost(&cand);
            if c < current {
                let rel = (current - c) / current.max(1e-300);
                circle = cand;
                current = c;
                lambda = (lambda * 0.3).max(1e-12);
                improved = rel > 1e-12;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    circle
}

/// Solves a 3×3 linear system (Gaussian elimination with partial pivoting).
fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..3 {
            let f = a[row][col] / a[col][col];
            for k in col..3 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 3];
    for row in (0..3).rev() {
        let s: f64 = (row + 1..3).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_exact_arc() {
        let truth = Circle { c: Point::new(94.684, 356.214), r: 0.123 };
        // Half a circle only (caps usually span about 180°).
        let pts: Vec<_> = (0..=8)
            .map(|i| {
                let a = std::f64::consts::PI * i as f64 / 8.0 + 0.3;
                truth.c + Vec2::new(a.cos(), a.sin()) * truth.r
            })
            .collect();
        let got = fit(&pts).unwrap();
        assert!((got.c - truth.c).hypot() < 1e-9, "{got:?}");
        assert!((got.r - truth.r).abs() < 1e-9, "{got:?}");
        assert!(got.relative_rms(&pts) < 1e-9);
    }

    #[test]
    fn rejects_degenerate() {
        assert!(fit(&[Point::new(0.0, 0.0), Point::new(1.0, 1.0)]).is_none());
        let collinear: Vec<_> = (0..5).map(|i| Point::new(i as f64, 2.0 * i as f64)).collect();
        assert!(fit(&collinear).is_none());
    }
}
