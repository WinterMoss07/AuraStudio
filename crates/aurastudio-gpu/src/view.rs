//! Canvas view: screen = area_center + pan + zoom·R(angle)·(canvas −
//! size / 2), and its exact inverse for the pen.

/// Zoom limits: 1 % to 6400 % (Krita goes to 10000 %, CSP to 6400 %).
pub const MIN_ZOOM: f32 = 0.01;
pub const MAX_ZOOM: f32 = 64.0;
/// Free space kept around the canvas when fitting it to the window.
const FIT_MARGIN: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasView {
    pub zoom:  f32,
    pub angle: f32,
    pub pan:   (f32, f32),
    /// Keep the whole canvas fitted to the area (at startup and after
    /// "Fit"), following window / dock resizes. Any manual zoom, pan or
    /// rotation leaves this mode.
    pub fit:   bool,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self { zoom: 1.0, angle: 0.0, pan: (0.0, 0.0), fit: true }
    }
}

/// Area of the window where the canvas is drawn: x, y, width, height.
pub type Area = [f32; 4];

fn center(area: Area) -> (f32, f32) {
    (area[0] + area[2] / 2.0, area[1] + area[3] / 2.0)
}

impl CanvasView {
    /// Zoom that shows the whole (rotated) canvas inside `area`.
    pub fn fit_zoom(&self, canvas: (u32, u32), area: Area) -> f32 {
        let (w, h) = (canvas.0 as f32, canvas.1 as f32);
        let (s, c) = self.angle.sin_cos();
        let bw = (w * c).abs() + (h * s).abs();
        let bh = (w * s).abs() + (h * c).abs();
        let aw = (area[2] - 2.0 * FIT_MARGIN).max(1.0);
        let ah = (area[3] - 2.0 * FIT_MARGIN).max(1.0);
        (aw / bw).min(ah / bh).clamp(MIN_ZOOM, MAX_ZOOM)
    }

    /// In fit mode, follow the current area size. Call before using the
    /// view each frame.
    pub fn update_fit(&mut self, canvas: (u32, u32), area: Area) {
        if self.fit {
            self.zoom = self.fit_zoom(canvas, area);
            self.pan = (0.0, 0.0);
        }
    }

    pub fn canvas_to_screen(&self, p: (f32, f32), canvas: (u32, u32), area: Area) -> (f32, f32) {
        let (s, c) = self.angle.sin_cos();
        let qx = p.0 - canvas.0 as f32 / 2.0;
        let qy = p.1 - canvas.1 as f32 / 2.0;
        let (cx, cy) = center(area);
        (
            cx + self.pan.0 + self.zoom * (c * qx - s * qy),
            cy + self.pan.1 + self.zoom * (s * qx + c * qy),
        )
    }

    pub fn screen_to_canvas(&self, p: (f32, f32), canvas: (u32, u32), area: Area) -> (f32, f32) {
        let (s, c) = self.angle.sin_cos();
        let (cx, cy) = center(area);
        let dx = (p.0 - cx - self.pan.0) / self.zoom;
        let dy = (p.1 - cy - self.pan.1) / self.zoom;
        // Inverse rotation: R(-angle).
        (
            c * dx + s * dy + canvas.0 as f32 / 2.0,
            -s * dx + c * dy + canvas.1 as f32 / 2.0,
        )
    }

    /// Change the view while keeping the canvas point under `pivot` (a
    /// screen position) exactly where it is: zoom toward the cursor,
    /// rotate around the view center...
    fn keep_pivot(&mut self, pivot: (f32, f32), canvas: (u32, u32), area: Area, change: impl FnOnce(&mut Self)) {
        let anchor = self.screen_to_canvas(pivot, canvas, area);
        change(self);
        let moved = self.canvas_to_screen(anchor, canvas, area);
        self.pan.0 += pivot.0 - moved.0;
        self.pan.1 += pivot.1 - moved.1;
        self.fit = false;
    }

    /// Where the view turns: the picture's center while it fits in the
    /// area (it spins in place), else the area's middle.
    pub fn rotation_pivot(&self, canvas: (u32, u32), area: Area) -> (f32, f32) {
        if self.zoom <= self.fit_zoom(canvas, area) * 1.001 {
            self.canvas_to_screen((canvas.0 as f32 / 2.0, canvas.1 as f32 / 2.0), canvas, area)
        } else {
            center(area)
        }
    }

    pub fn zoom_at(&mut self, factor: f32, pivot: (f32, f32), canvas: (u32, u32), area: Area) {
        self.keep_pivot(pivot, canvas, area, |v| v.zoom = (v.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM));
    }

    /// Set an absolute zoom (1.0 = 100 %), around `pivot`.
    pub fn set_zoom(&mut self, zoom: f32, pivot: (f32, f32), canvas: (u32, u32), area: Area) {
        self.keep_pivot(pivot, canvas, area, |v| v.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM));
    }

    /// Rotate by `delta` radians around `pivot`.
    pub fn rotate_at(&mut self, delta: f32, pivot: (f32, f32), canvas: (u32, u32), area: Area) {
        self.keep_pivot(pivot, canvas, area, |v| v.angle = wrap_angle(v.angle + delta));
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan.0 += dx;
        self.pan.1 += dy;
        self.fit = false;
    }

    /// Back to the whole canvas in view (keeps the rotation).
    pub fn fit_to_area(&mut self) {
        self.fit = true;
    }

    /// Canvas pixel -> normalized device coordinates of the render pass,
    /// whose viewport is `area`. Rows of a 2x3 affine matrix, padded to
    /// vec4 for the uniform layout.
    pub fn ndc_matrix(&self, canvas: (u32, u32), area: Area) -> [[f32; 4]; 2] {
        // Screen of canvas (0,0), and the screen steps of +1 canvas x / y.
        let o  = self.canvas_to_screen((0.0, 0.0), canvas, area);
        let px = self.canvas_to_screen((1.0, 0.0), canvas, area);
        let py = self.canvas_to_screen((0.0, 1.0), canvas, area);
        // screen -> NDC inside the pass viewport (y up).
        let sx = 2.0 / area[2];
        let sy = -2.0 / area[3];
        let tx = -1.0 - area[0] * sx;
        let ty = 1.0 - area[1] * sy;
        [
            [(px.0 - o.0) * sx, (py.0 - o.0) * sx, o.0 * sx + tx, 0.0],
            [(px.1 - o.1) * sy, (py.1 - o.1) * sy, o.1 * sy + ty, 0.0],
        ]
    }
}

/// Angle in (-PI, PI].
pub fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut a = a % TAU;
    if a <= -PI { a += TAU; }
    if a > PI { a -= TAU; }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (2000, 1000);
    const AREA: Area = [100.0, 50.0, 1200.0, 800.0];

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-2 && (a.1 - b.1).abs() < 1e-2
    }

    #[test]
    fn rotation_pivot_depends_on_zoom_not_position() {
        let mut v = CanvasView::default();
        v.update_fit(CANVAS, AREA);
        v.fit = false;
        // Zoomed out, picture pushed half out of the area: still its own
        // center.
        v.zoom_at(0.5, (200.0, 200.0), CANVAS, AREA);
        v.pan_by(-700.0, 0.0);
        let mid = v.canvas_to_screen((1000.0, 500.0), CANVAS, AREA);
        assert!(close(v.rotation_pivot(CANVAS, AREA), mid));
        assert!(!close(mid, center(AREA)));
        // Zoomed in past "fit", even with the picture centered: the middle
        // of the area.
        v.zoom_at(4.0, (300.0, 300.0), CANVAS, AREA);
        v.pan = (0.0, 0.0);
        v.pan_by(5.0, 0.0);
        assert!(close(v.rotation_pivot(CANVAS, AREA), center(AREA)));
    }

    #[test]
    fn screen_and_canvas_are_inverse() {
        let v = CanvasView { zoom: 2.7, angle: 0.6, pan: (33.0, -12.0), fit: false };
        for p in [(0.0, 0.0), (1999.0, 999.0), (512.5, 77.25)] {
            let s = v.canvas_to_screen(p, CANVAS, AREA);
            assert!(close(v.screen_to_canvas(s, CANVAS, AREA), p));
        }
    }

    #[test]
    fn zoom_and_rotation_keep_the_point_under_the_cursor() {
        let mut v = CanvasView::default();
        v.update_fit(CANVAS, AREA);
        let cursor = (400.0, 300.0);
        let under = v.screen_to_canvas(cursor, CANVAS, AREA);
        v.zoom_at(3.0, cursor, CANVAS, AREA);
        assert!(close(v.canvas_to_screen(under, CANVAS, AREA), cursor));
        v.rotate_at(0.8, cursor, CANVAS, AREA);
        assert!(close(v.canvas_to_screen(under, CANVAS, AREA), cursor));
        assert!(!v.fit);
    }

    #[test]
    fn fit_shows_the_whole_rotated_canvas() {
        let mut v = CanvasView { angle: 0.5, ..Default::default() };
        v.update_fit(CANVAS, AREA);
        for c in [(0.0, 0.0), (2000.0, 0.0), (0.0, 1000.0), (2000.0, 1000.0)] {
            let s = v.canvas_to_screen(c, CANVAS, AREA);
            assert!(s.0 >= AREA[0] - 0.01 && s.0 <= AREA[0] + AREA[2] + 0.01, "{s:?}");
            assert!(s.1 >= AREA[1] - 0.01 && s.1 <= AREA[1] + AREA[3] + 0.01, "{s:?}");
        }
    }

    #[test]
    fn ndc_matrix_matches_screen_mapping() {
        let v = CanvasView { zoom: 0.8, angle: -1.1, pan: (5.0, 9.0), fit: false };
        let m = v.ndc_matrix(CANVAS, AREA);
        let p = (300.0, 700.0);
        let ndc = (m[0][0] * p.0 + m[0][1] * p.1 + m[0][2], m[1][0] * p.0 + m[1][1] * p.1 + m[1][2]);
        let s = v.canvas_to_screen(p, CANVAS, AREA);
        let expect = ((s.0 - AREA[0]) / AREA[2] * 2.0 - 1.0, 1.0 - (s.1 - AREA[1]) / AREA[3] * 2.0);
        assert!(close(ndc, expect), "{ndc:?} vs {expect:?}");
    }

    #[test]
    fn angle_wraps() {
        assert!((wrap_angle(7.0) - (7.0 - std::f32::consts::TAU)).abs() < 1e-5);
        assert!(wrap_angle(-4.0) > 0.0);
    }
}
