//! Shape comparison on rasters.
//!
//! Goodnotes outlines are made of many overlapping pieces, so their path boundaries include
//! interior edges. Comparing filled areas on a fine grid (non-zero winding rule) avoids that.

use kurbo::{BezPath, PathEl, Rect, Shape};

/// How a candidate shape differs from a reference shape.
#[derive(Clone, Copy, Debug)]
pub struct Comparison {
    /// Intersection over union of the filled areas.
    pub iou: f64,
    /// Symmetric Hausdorff distance between the filled areas, in page points.
    pub hausdorff: f64,
    /// Candidate boundary length ÷ reference boundary length. Burrs and wiggles make it > 1.
    pub edge_ratio: f64,
    /// Grid step used, in page points.
    pub step: f64,
}

/// Compares two filled shapes on a grid of 1/1500 of their size, between 0.002 and 0.01 pt.
pub fn compare(reference: &BezPath, candidate: &BezPath) -> Comparison {
    let bbox = reference.bounding_box().union(candidate.bounding_box());
    compare_at(reference, candidate, (bbox.width().max(bbox.height()) / 1500.0).clamp(0.002, 0.01))
}

/// Compares two filled shapes on a grid with the given step (pt). Thin strokes need a step of a
/// few percent of their radius, or one-pixel boundary shifts dominate the IoU.
pub fn compare_at(reference: &BezPath, candidate: &BezPath, step: f64) -> Comparison {
    let bbox = reference.bounding_box().union(candidate.bounding_box());
    let bbox = bbox.inflate(3.0 * step, 3.0 * step);
    let a = Raster::fill(reference, bbox, step);
    let b = Raster::fill(candidate, bbox, step);

    let (mut inter, mut union) = (0usize, 0usize);
    for (x, y) in a.bits.iter().zip(&b.bits) {
        inter += (*x && *y) as usize;
        union += (*x || *y) as usize;
    }
    let iou = if union == 0 { 1.0 } else { inter as f64 / union as f64 };

    let (da, db) = (a.distance_field(), b.distance_field());
    let mut h2: f64 = 0.0;
    for i in 0..a.bits.len() {
        if a.bits[i] {
            h2 = h2.max(db[i]);
        }
        if b.bits[i] {
            h2 = h2.max(da[i]);
        }
    }
    let edge_ratio = b.boundary_len() as f64 / (a.boundary_len() as f64).max(1.0);
    Comparison { iou, hausdorff: h2.sqrt() * step, edge_ratio, step }
}

/// A filled shape sampled at pixel centers.
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub bits: Vec<bool>,
}

impl Raster {
    /// Scanline fill with the non-zero winding rule.
    pub fn fill(path: &BezPath, bbox: Rect, step: f64) -> Self {
        let w = (bbox.width() / step).ceil().max(1.0) as usize;
        let h = (bbox.height() / step).ceil().max(1.0) as usize;
        let mut rows: Vec<Vec<(f64, i32)>> = vec![Vec::new(); h];

        let mut edge = |a: kurbo::Point, b: kurbo::Point| {
            if a.y == b.y {
                return;
            }
            let dir = if b.y > a.y { 1 } else { -1 };
            let (lo, hi) = if a.y < b.y { (a, b) } else { (b, a) };
            // Rows whose center y lies in [lo.y, hi.y).
            let j0 = ((lo.y - bbox.y0) / step - 0.5).ceil().max(0.0) as usize;
            let j1 = (((hi.y - bbox.y0) / step - 0.5).ceil().max(0.0) as usize).min(h);
            for (j, row) in rows.iter_mut().enumerate().take(j1).skip(j0) {
                let y = bbox.y0 + (j as f64 + 0.5) * step;
                let x = lo.x + (y - lo.y) * (hi.x - lo.x) / (hi.y - lo.y);
                row.push((x, dir));
            }
        };
        let (mut start, mut last) = (kurbo::Point::ORIGIN, kurbo::Point::ORIGIN);
        kurbo::flatten(path.elements().iter().copied(), step * 0.1, |el| match el {
            PathEl::MoveTo(p) => {
                if last != start {
                    edge(last, start);
                }
                start = p;
                last = p;
            }
            PathEl::LineTo(p) => {
                edge(last, p);
                last = p;
            }
            PathEl::ClosePath => {
                edge(last, start);
                last = start;
            }
            _ => {}
        });
        if last != start {
            edge(last, start);
        }

        let mut bits = vec![false; w * h];
        for (j, row) in rows.iter_mut().enumerate() {
            row.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for k in 0..row.len() {
                winding += row[k].1;
                if winding != 0 && k + 1 < row.len() {
                    // Pixel centers in [row[k].x, row[k+1].x).
                    let i0 = ((row[k].0 - bbox.x0) / step - 0.5).ceil().max(0.0) as usize;
                    let i1 = (((row[k + 1].0 - bbox.x0) / step - 0.5).ceil().max(0.0) as usize).min(w);
                    for i in i0..i1 {
                        bits[j * w + i] = true;
                    }
                }
            }
        }
        Self { w, h, bits }
    }

    /// Squared Euclidean distance (in pixels²) from every pixel to the nearest filled pixel.
    /// Felzenszwalb–Huttenlocher separable transform.
    pub fn distance_field(&self) -> Vec<f64> {
        const INF: f64 = 1e20;
        let mut d: Vec<f64> = self.bits.iter().map(|&b| if b { 0.0 } else { INF }).collect();
        let mut buf = vec![0.0; self.w.max(self.h)];
        for x in 0..self.w {
            for y in 0..self.h {
                buf[y] = d[y * self.w + x];
            }
            let out = dt1(&buf[..self.h]);
            for y in 0..self.h {
                d[y * self.w + x] = out[y];
            }
        }
        for y in 0..self.h {
            let out = dt1(&d[y * self.w..(y + 1) * self.w]);
            d[y * self.w..(y + 1) * self.w].copy_from_slice(&out);
        }
        d
    }

    /// Number of filled pixels with an empty 4-neighbour.
    pub fn boundary_len(&self) -> usize {
        let at = |x: isize, y: isize| {
            x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.bits[y as usize * self.w + x as usize]
        };
        let mut n = 0;
        for y in 0..self.h as isize {
            for x in 0..self.w as isize {
                if at(x, y) && !(at(x - 1, y) && at(x + 1, y) && at(x, y - 1) && at(x, y + 1)) {
                    n += 1;
                }
            }
        }
        n
    }
}

/// 1-D squared distance transform of a sampled function (lower envelope of parabolas).
fn dt1(f: &[f64]) -> Vec<f64> {
    let n = f.len();
    let mut d = vec![0.0; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0; n + 1];
    let mut k = 0;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64)) / (2.0 * q as f64 - 2.0 * p as f64);
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: replace the first parabola
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
                break;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f64::INFINITY;
            break;
        }
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *out = (q as f64 - p as f64).powi(2) + f[p];
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::Circle;

    #[test]
    fn identical_shapes_match() {
        let c = Circle::new((0.0, 0.0), 1.0).to_path(1e-4);
        let cmp = compare(&c, &c);
        assert!(cmp.iou > 0.999, "{cmp:?}");
        assert!(cmp.hausdorff < 1e-9, "{cmp:?}");
        assert!((cmp.edge_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn shifted_circle_hausdorff_equals_shift() {
        let a = Circle::new((0.0, 0.0), 1.0).to_path(1e-4);
        let b = Circle::new((0.2, 0.0), 1.0).to_path(1e-4);
        let cmp = compare(&a, &b);
        assert!((cmp.hausdorff - 0.2).abs() < 2.0 * cmp.step, "{cmp:?}");
        // Two unit disks 0.2 apart: overlap 2·acos(0.1) − 0.1·√3.96 ≈ 2.7423, union 2π − overlap ≈ 3.5409.
        let expected_iou = 0.7744;
        assert!((cmp.iou - expected_iou).abs() < 0.01, "{cmp:?}");
    }

    #[test]
    fn spike_shows_in_hausdorff_and_edges() {
        let a = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-4);
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((1.0, 0.0));
        b.line_to((1.0, 0.45));
        b.line_to((1.5, 0.5)); // a burr
        b.line_to((1.0, 0.55));
        b.line_to((1.0, 1.0));
        b.line_to((0.0, 1.0));
        b.close_path();
        let cmp = compare(&a, &b);
        assert!(cmp.iou > 0.95, "{cmp:?}");
        assert!((cmp.hausdorff - 0.5).abs() < 0.05, "{cmp:?}");
        assert!(cmp.edge_ratio > 1.05, "{cmp:?}");
    }

    #[test]
    fn overlapping_subpaths_use_nonzero() {
        let mut p = Rect::new(0.0, 0.0, 1.0, 1.0).to_path(1e-4);
        p.extend(Rect::new(0.5, 0.0, 1.5, 1.0).to_path(1e-4));
        let union = Rect::new(0.0, 0.0, 1.5, 1.0).to_path(1e-4);
        let cmp = compare(&union, &p);
        assert!(cmp.iou > 0.995, "{cmp:?}");
    }
}
