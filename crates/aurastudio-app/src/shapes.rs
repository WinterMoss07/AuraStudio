//! Geometry tools: the path of each shape, in canvas pixels, as points
//! close enough together for the brush to follow (the brush engine spaces
//! its dabs itself). The shape is then painted as a stroke with the current
//! brush, so it gets the brush's look, the selection and undo for free.

type P = (f32, f32);

/// Largest distance between two points of a path, canvas pixels.
const STEP: f32 = 2.0;
/// Most points in one path: a huge shape (dragged far off the canvas,
/// zoomed out) gets longer straight pieces instead of millions of points.
const MAX_POINTS: usize = 20_000;

/// Number of pieces for a path `len` long (at least `min`).
fn pieces(len: f32, min: f32) -> usize {
    if !len.is_finite() { return min as usize; }
    ((len / STEP).ceil().max(min) as usize).min(MAX_POINTS)
}

pub fn line(a: P, b: P) -> Vec<P> {
    let n = pieces(dist(a, b), 1.0);
    (0..=n).map(|i| lerp(a, b, i as f32 / n as f32)).collect()
}

/// Rectangle with corners `a` and `b`, closed (ends where it starts).
pub fn rectangle(a: P, b: P) -> Vec<P> {
    let corners = [a, (b.0, a.1), b, (a.0, b.1), a];
    let mut out = vec![a];
    for w in corners.windows(2) {
        out.extend(line(w[0], w[1]).into_iter().skip(1));
    }
    out
}

/// Ellipse inside the box with corners `a` and `b`, closed.
pub fn ellipse(a: P, b: P) -> Vec<P> {
    let c = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let (rx, ry) = ((b.0 - a.0).abs() / 2.0, (b.1 - a.1).abs() / 2.0);
    // Even angle steps: the points are furthest apart where the radius is
    // largest, so count them from it.
    let n = pieces(std::f32::consts::TAU * rx.max(ry), 8.0);
    (0..=n).map(|i| {
        // Starts at the top, like a hand-drawn circle.
        let t = i as f32 / n as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        (c.0 + rx * t.cos(), c.1 + ry * t.sin())
    }).collect()
}

/// The box for a rectangle / ellipse dragged from `start` to `end`.
/// `square`: same width and height (Shift); `centered`: `start` is the
/// center (Alt).
pub fn drag_box(start: P, end: P, square: bool, centered: bool) -> (P, P) {
    let (mut dx, mut dy) = (end.0 - start.0, end.1 - start.1);
    if square {
        let s = dx.abs().max(dy.abs());
        dx = s.copysign(dx);
        dy = s.copysign(dy);
    }
    if centered { ((start.0 - dx, start.1 - dy), (start.0 + dx, start.1 + dy)) } else { (start, (start.0 + dx, start.1 + dy)) }
}

/// Line end snapped to steps of 15° around `start` (Shift).
pub fn snap_angle(start: P, end: P) -> P {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let step = 15f32.to_radians();
    let a = (dy.atan2(dx) / step).round() * step;
    let r = (dx * dx + dy * dy).sqrt();
    (start.0 + r * a.cos(), start.1 + r * a.sin())
}

/// Arc from `a` to `b` pulled toward `control` (OpenToonz's arc tool: a
/// quadratic Bézier curve whose control point is the cursor). Straight
/// when the control point is on the line.
pub fn arc(a: P, b: P, c: P) -> Vec<P> {
    // The curve moves fastest at its ends (twice the control legs per unit
    // of t): count the points from there.
    let n = pieces(2.0 * dist(a, c).max(dist(c, b)), 1.0);
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1)
    }).collect()
}

fn lerp(a: P, b: P, t: f32) -> P { (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t) }

fn dist(a: P, b: P) -> f32 { ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() }

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P) -> bool { dist(a, b) < 0.01 }

    fn dense(path: &[P]) -> bool { path.windows(2).all(|w| dist(w[0], w[1]) <= STEP + 0.01) }

    #[test]
    fn shapes_are_dense_and_closed() {
        let r = rectangle((10.0, 10.0), (50.0, 30.0));
        assert!(dense(&r) && close(r[0], *r.last().unwrap()));
        assert!(r.contains(&(50.0, 10.0)) && r.contains(&(10.0, 30.0)));
        let e = ellipse((0.0, 0.0), (100.0, 40.0));
        assert!(dense(&e) && close(e[0], *e.last().unwrap()));
        assert!(close(e[0], (50.0, 0.0)));
        assert!(e.iter().all(|p| ((p.0 - 50.0) / 50.0).powi(2) + ((p.1 - 20.0) / 20.0).powi(2) < 1.001));
        let l = line((0.0, 0.0), (10.0, 0.0));
        assert!(dense(&l) && close(l[0], (0.0, 0.0)) && close(*l.last().unwrap(), (10.0, 0.0)));
    }

    #[test]
    fn arcs_bulge_toward_the_control_point() {
        let (a, b) = ((0.0, 0.0), (100.0, 0.0));
        // Control point down right: the curve's top is toward it.
        let p = arc(a, b, (80.0, 60.0));
        assert!(dense(&p) && close(p[0], a) && dist(*p.last().unwrap(), b) < 0.05);
        let deepest = p.iter().copied().fold(a, |m, q| if q.1 > m.1 { q } else { m });
        assert!(deepest.0 > 50.0 && deepest.1 > 25.0, "{deepest:?}");
        // Halfway between the chord and the control point at t = 0.5.
        let mid = p[p.len() / 2];
        assert!(dist(mid, (65.0, 30.0)) < 2.0, "{mid:?}");
        // On the line: straight.
        assert!(arc(a, b, (50.0, 0.0)).iter().all(|q| q.1.abs() < 1e-3));
    }

    #[test]
    fn huge_shapes_stay_small() {
        assert!(ellipse((-1e7, -1e7), (1e7, 1e7)).len() <= MAX_POINTS + 1);
        assert!(line((0.0, 0.0), (1e9, 0.0)).len() <= MAX_POINTS + 1);
        assert!(arc((0.0, 0.0), (1e7, 0.0), (5e6, 1e7)).len() <= MAX_POINTS + 1);
        assert!(line((0.0, 0.0), (f32::INFINITY, 0.0)).len() <= 2);
    }

    #[test]
    fn drag_boxes_and_snaps() {
        assert_eq!(drag_box((10.0, 10.0), (30.0, 20.0), true, false), ((10.0, 10.0), (30.0, 30.0)));
        assert_eq!(drag_box((10.0, 10.0), (5.0, 20.0), false, true), ((15.0, 0.0), (5.0, 20.0)));
        let s = snap_angle((0.0, 0.0), (10.0, 1.0));
        assert!(close(s, ((101.0f32).sqrt(), 0.0)));
    }
}
