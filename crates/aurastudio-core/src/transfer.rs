//! `TransferCurve`: a one-dimensional curve, one value in, one value out
//! (0..1 both): the global pressure curve, per-brush curves, filters'
//! curves, camera easing, the curve editor. Control points joined by a
//! monotone cubic (no overshoot between points: a curve that only rises
//! never dips), sampled once into a table, read by interpolation.

/// Samples of the table (as Krita's tablet curve).
const SAMPLES: usize = 1025;

#[derive(Clone, Debug, PartialEq)]
pub struct TransferCurve {
    /// Sorted by x, x in 0..1, at least two (first x = 0, last x = 1).
    points: Vec<(f32, f32)>,
    table:  Vec<f32>,
}

impl Default for TransferCurve {
    fn default() -> Self { Self::linear() }
}

impl TransferCurve {
    /// Out = in.
    pub fn linear() -> Self { Self::new(&[(0.0, 0.0), (1.0, 1.0)]) }

    /// Through `points` (any order; clamped to 0..1; ends added at x = 0
    /// and 1 from the nearest point when missing; same x: the last wins).
    pub fn new(points: &[(f32, f32)]) -> Self {
        let mut p: Vec<(f32, f32)> = points.iter().filter(|(x, y)| x.is_finite() && y.is_finite()).map(|&(x, y)| (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0))).collect();
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p.dedup_by(|b, a| { if (a.0 - b.0).abs() < 1e-6 { a.1 = b.1; true } else { false } });
        if p.is_empty() { p = vec![(0.0, 0.0), (1.0, 1.0)]; }
        if p[0].0 > 0.0 { let y = p[0].1; p.insert(0, (0.0, y)); }
        if let Some(&(x, y)) = p.last() { if x < 1.0 { p.push((1.0, y)); } }
        let table = sample(&p);
        Self { points: p, table }
    }

    pub fn points(&self) -> &[(f32, f32)] { &self.points }

    pub fn is_linear(&self) -> bool { self.points.iter().all(|(x, y)| (x - y).abs() < 1e-6) }

    /// The value at `x` (clamped to 0..1).
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let t = x.clamp(0.0, 1.0) * (SAMPLES - 1) as f32;
        let i = (t as usize).min(SAMPLES - 2);
        let f = t - i as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * f
    }
}

/// Monotone cubic (Fritsch–Carlson) through `p`, sampled.
fn sample(p: &[(f32, f32)]) -> Vec<f32> {
    let n = p.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (p[i + 1].1 - p[i].1) / (p[i + 1].0 - p[i].0).max(1e-9)).collect();
    let mut m = vec![0.0f32; n];
    m[0] = d[0];
    m[n - 1] = d[n - 2];
    for i in 1..n - 1 { m[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 }; }
    for i in 0..n - 1 {
        if d[i] == 0.0 { m[i] = 0.0; m[i + 1] = 0.0; continue; }
        let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
        let s = a * a + b * b;
        if s > 9.0 { let t = 3.0 / s.sqrt(); m[i] = t * a * d[i]; m[i + 1] = t * b * d[i]; }
    }
    let mut out = Vec::with_capacity(SAMPLES);
    let mut k = 0;
    for j in 0..SAMPLES {
        let x = j as f32 / (SAMPLES - 1) as f32;
        while k + 2 < n && x > p[k + 1].0 { k += 1; }
        let (x0, y0, x1, y1) = (p[k].0, p[k].1, p[k + 1].0, p[k + 1].1);
        let h = (x1 - x0).max(1e-9);
        let t = ((x - x0) / h).clamp(0.0, 1.0);
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * y0 + (t3 - 2.0 * t2 + t) * h * m[k] + (-2.0 * t3 + 3.0 * t2) * y1 + (t3 - t2) * h * m[k + 1];
        out.push(y.clamp(0.0, 1.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_and_points() {
        let l = TransferCurve::linear();
        for x in [0.0, 0.1, 0.5, 0.99, 1.0] { assert!((l.eval(x) - x).abs() < 1e-4); }
        assert!(l.is_linear());
        // Krita's Basic-5 size curve.
        let c = TransferCurve::new(&[(0.0, 0.0), (0.35, 0.1), (1.0, 1.0)]);
        assert!((c.eval(0.35) - 0.1).abs() < 1e-3);
        assert!(c.eval(0.2) < 0.1 && c.eval(0.2) > 0.0);
    }

    #[test]
    fn rising_points_never_dip() {
        let c = TransferCurve::new(&[(0.0, 0.0), (0.1, 0.8), (0.2, 0.82), (1.0, 1.0)]);
        let mut prev = 0.0;
        for i in 0..=1000 { let v = c.eval(i as f32 / 1000.0); assert!(v + 1e-6 >= prev, "dip at {i}"); prev = v; }
    }

    #[test]
    fn ends_are_added_and_bad_points_dropped() {
        let c = TransferCurve::new(&[(0.5, 0.3), (f32::NAN, 0.0)]);
        assert_eq!(c.points().len(), 3);
        assert!((c.eval(0.0) - 0.3).abs() < 1e-4 && (c.eval(1.0) - 0.3).abs() < 1e-4);
        assert!(TransferCurve::new(&[]).is_linear());
    }
}
