//! Geometry tools: each shape's path as close points, painted as a stroke
//! with the current brush. Curves are walked by arc length: points evenly
//! spaced whatever the shape (a thin ellipse gets no crowd of points on
//! its flat sides).

use aurastudio_core::Point;

/// Largest distance between two points of a path, canvas pixels.
pub const STEP: f64 = 2.0;
/// Most points in one path: a huge shape (dragged far off the canvas,
/// zoomed out) gets longer straight pieces instead of millions of points.
pub const MAX_POINTS: usize = 20_000;

/// Number of pieces for a path `len` long (at least `min`).
fn pieces(len: f64, min: f64) -> usize {
    if !len.is_finite() { return min as usize; }
    ((len / STEP).ceil().max(min) as usize).min(MAX_POINTS)
}

pub fn line(a: Point, b: Point) -> Vec<Point> {
    let n = pieces(a.distance(b), 1.0);
    (0..=n).map(|i| a.lerp(b, i as f64 / n as f64)).collect()
}

/// Rectangle with corners `a` and `b`, closed (ends where it starts).
pub fn rectangle(a: Point, b: Point) -> Vec<Point> {
    let corners = [a, Point::new(b.x, a.y), b, Point::new(a.x, b.y), a];
    let mut out = vec![a];
    for w in corners.windows(2) { out.extend(line(w[0], w[1]).into_iter().skip(1)); }
    out
}

/// The curve `f` on t ∈ 0..1 with points about `STEP` apart along it:
/// its length measured on a table (four entries per point to place, from
/// a rough first measure), then walked by equal lengths.
fn by_length(f: impl Fn(f64) -> Point, min: f64) -> Vec<Point> {
    let rough: f64 = (0..64).map(|k| f(k as f64 / 64.0).distance(f((k + 1) as f64 / 64.0))).sum();
    let fine = (pieces(rough, min) * 4).clamp(64, 1 << 16);
    by_table(&f, min, fine)
}

fn by_table(f: &impl Fn(f64) -> Point, min: f64, fine: usize) -> Vec<Point> {
        let mut acc = Vec::with_capacity(fine + 1);
    let (mut total, mut prev) = (0.0, f(0.0));
    acc.push(0.0);
    for k in 1..=fine {
        let p = f(k as f64 / fine as f64);
        total += prev.distance(p);
        acc.push(total);
        prev = p;
    }
    let n = pieces(total, min);
    if total.is_nan() || total <= 0.0 { return (0..=n).map(|i| f(i as f64 / n as f64)).collect(); }
    let mut out = Vec::with_capacity(n + 1);
    let mut k = 0;
    for i in 0..=n {
        let s = total * i as f64 / n as f64;
        while k + 1 < fine && acc[k + 1] < s { k += 1; }
        let seg = acc[k + 1] - acc[k];
        let t = (k as f64 + if seg > 0.0 { (s - acc[k]) / seg } else { 0.0 }) / fine as f64;
        out.push(f(t.min(1.0)));
    }
    out
}

/// Ellipse inside the box with corners `a` and `b`, closed; starts at the
/// top, like a hand-drawn circle.
pub fn ellipse(a: Point, b: Point) -> Vec<Point> {
    let c = a.lerp(b, 0.5);
    let (rx, ry) = ((b.x - a.x).abs() / 2.0, (b.y - a.y).abs() / 2.0);
    let tau = std::f64::consts::TAU;
    let mut v = by_length(|t| { let t = t * tau - std::f64::consts::FRAC_PI_2; Point::new(c.x + rx * t.cos(), c.y + ry * t.sin()) }, 8.0);
    // Closed exactly.
    if let Some(&first) = v.first() { if let Some(last) = v.last_mut() { *last = first; } }
    v
}

/// The box for a rectangle / ellipse dragged from `start` to `end`.
/// `square`: same width and height (Shift); `centered`: `start` is the
/// center (Alt).
pub fn drag_box(start: Point, end: Point, square: bool, centered: bool) -> (Point, Point) {
    let mut d = end - start;
    if square {
        let s = d.x.abs().max(d.y.abs());
        d = Point::new(s.copysign(d.x), s.copysign(d.y));
    }
    if centered { (start - d, start + d) } else { (start, start + d) }
}

/// Line end snapped to steps of 15° around `start` (Shift).
pub fn snap_angle(start: Point, end: Point) -> Point {
    let d = end - start;
    let step = 15f64.to_radians();
    let a = (d.y.atan2(d.x) / step).round() * step;
    let r = d.length();
    start + Point::new(r * a.cos(), r * a.sin())
}

/// Arc from `a` to `b` pulled toward `c` (OpenToonz's arc tool: a
/// quadratic Bézier curve whose control point is the cursor). Straight
/// when the control point is on the line.
pub fn arc(a: Point, b: Point, c: Point) -> Vec<Point> {
    let mut v = by_length(|t| { let u = 1.0 - t; a * (u * u) + c * (2.0 * u * t) + b * (t * t) }, 1.0);
    if let Some(last) = v.last_mut() { *last = b; }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f64, y: f64) -> Point { Point::new(x, y) }
    fn close(a: Point, b: Point) -> bool { a.distance(b) < 0.01 }
    fn dense(path: &[Point]) -> bool { path.windows(2).all(|w| w[0].distance(w[1]) <= STEP + 0.01) }

    #[test]
    fn shapes_are_dense_and_closed() {
        let r = rectangle(pt(10.0, 10.0), pt(50.0, 30.0));
        assert!(dense(&r) && close(r[0], *r.last().unwrap()));
        assert!(r.contains(&pt(50.0, 10.0)) && r.contains(&pt(10.0, 30.0)));
        let e = ellipse(pt(0.0, 0.0), pt(100.0, 40.0));
        assert!(dense(&e) && close(e[0], *e.last().unwrap()));
        assert!(close(e[0], pt(50.0, 0.0)));
        assert!(e.iter().all(|p| ((p.x - 50.0) / 50.0).powi(2) + ((p.y - 20.0) / 20.0).powi(2) < 1.001));
        let l = line(pt(0.0, 0.0), pt(10.0, 0.0));
        assert!(dense(&l) && close(l[0], pt(0.0, 0.0)) && close(*l.last().unwrap(), pt(10.0, 0.0)));
    }

    /// Evenly spaced along the curve: a thin ellipse has no crowd of points.
    #[test]
    fn curves_are_walked_by_length() {
        let e = ellipse(pt(0.0, 0.0), pt(400.0, 60.0));
        let gaps: Vec<f64> = e.windows(2).map(|w| w[0].distance(w[1])).collect();
        let (lo, hi) = (gaps.iter().copied().fold(f64::MAX, f64::min), gaps.iter().copied().fold(0.0, f64::max));
        assert!(hi <= STEP + 0.01 && lo > hi * 0.8, "{lo} .. {hi}");
        // About its perimeter / STEP points (Ramanujan: ≈ 825 px), not
        // the largest radius' (628 with even angles).
        assert!((405..=425).contains(&e.len()), "{}", e.len());
    }

    #[test]
    fn arcs_bulge_toward_the_control_point() {
        let (a, b) = (pt(0.0, 0.0), pt(100.0, 0.0));
        let p = arc(a, b, pt(80.0, 60.0));
        assert!(dense(&p) && close(p[0], a) && p.last().unwrap().distance(b) < 0.05);
        let deepest = p.iter().copied().fold(a, |m, q| if q.y > m.y { q } else { m });
        assert!(deepest.x > 50.0 && deepest.y > 25.0, "{deepest:?}");
        // The point at t = 0.5 (halfway between the chord and the control).
        assert!(p.iter().any(|q| q.distance(pt(65.0, 30.0)) < 2.0));
        assert!(arc(a, b, pt(50.0, 0.0)).iter().all(|q| q.y.abs() < 1e-3));
    }

    #[test]
    fn huge_shapes_stay_small() {
        assert!(ellipse(pt(-1e7, -1e7), pt(1e7, 1e7)).len() <= MAX_POINTS + 1);
        assert!(line(pt(0.0, 0.0), pt(1e9, 0.0)).len() <= MAX_POINTS + 1);
        assert!(arc(pt(0.0, 0.0), pt(1e7, 0.0), pt(5e6, 1e7)).len() <= MAX_POINTS + 1);
        assert!(line(pt(0.0, 0.0), pt(f64::INFINITY, 0.0)).len() <= 2);
        assert!(!ellipse(pt(3.0, 3.0), pt(3.0, 3.0)).is_empty());
    }

    #[test]
    fn drag_boxes_and_snaps() {
        assert_eq!(drag_box(pt(10.0, 10.0), pt(30.0, 20.0), true, false), (pt(10.0, 10.0), pt(30.0, 30.0)));
        assert_eq!(drag_box(pt(10.0, 10.0), pt(5.0, 20.0), false, true), (pt(15.0, 0.0), pt(5.0, 20.0)));
        assert!(close(snap_angle(pt(0.0, 0.0), pt(10.0, 1.0)), pt(101f64.sqrt(), 0.0)));
    }
}
