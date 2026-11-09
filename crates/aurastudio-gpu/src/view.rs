//! Canvas view: screen = area_center + pan + zoom·R(angle)·F·(canvas −
//! size / 2), F the mirror (a flip of x and / or y), and its exact
//! inverse for the pen.

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
    /// The view mirrored left-right / top-bottom (the picture is not).
    pub mirror: (bool, bool),
}

impl Default for CanvasView {
    fn default() -> Self {
        Self { zoom: 1.0, angle: 0.0, pan: (0.0, 0.0), fit: true, mirror: (false, false) }
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

    /// The mirror's signs on x and y.
    pub fn signs(&self) -> (f32, f32) { (if self.mirror.0 { -1.0 } else { 1.0 }, if self.mirror.1 { -1.0 } else { 1.0 }) }

    pub fn canvas_to_screen(&self, p: (f32, f32), canvas: (u32, u32), area: Area) -> (f32, f32) {
        let (s, c) = self.angle.sin_cos();
        let (fx, fy) = self.signs();
        let qx = fx * (p.0 - canvas.0 as f32 / 2.0);
        let qy = fy * (p.1 - canvas.1 as f32 / 2.0);
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
        // Inverse rotation R(-angle), then the mirror (its own inverse).
        let (fx, fy) = self.signs();
        (
            fx * (c * dx + s * dy) + canvas.0 as f32 / 2.0,
            fy * (-s * dx + c * dy) + canvas.1 as f32 / 2.0,
        )
    }

    /// Mirror the view left-right (`horizontal`) or top-bottom, around the
    /// screen point `pivot` (it keeps showing the same canvas point).
    pub fn mirror_at(&mut self, horizontal: bool, pivot: (f32, f32), canvas: (u32, u32), area: Area) {
        let fit = self.fit;
        self.keep_pivot(pivot, canvas, area, |v| if horizontal { v.mirror.0 = !v.mirror.0 } else { v.mirror.1 = !v.mirror.1 });
        // (A fitted view stays fitted: the mirror doesn't change its size.)
        self.fit = fit;
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

use crate::GpuState;

/// The view at one moment (see `GpuState::view_snapshot`).
#[derive(Clone, Copy, Debug)]
pub struct ViewSnapshot {
    view:   CanvasView,
    canvas: (u32, u32),
    area:   Area,
    ppp:    f32,
}

impl ViewSnapshot {
    /// Canvas pixel -> interface point.
    pub fn to_points(&self, p: (f32, f32)) -> (f32, f32) {
        let (x, y) = self.view.canvas_to_screen(p, self.canvas, self.area);
        (x / self.ppp, y / self.ppp)
    }
}

/// The view, through the window: the methods the app calls.
impl GpuState {
    /// The display scale the window is at (2.0 at 200 %).
    pub fn dpi_scale(&self) -> f32 { self.dpi_scale }

    /// Physical screen pixels per canvas pixel at 100 % zoom.
    pub fn zoom_unit(&self) -> f32 { if self.pixel_exact { 1.0 } else { self.dpi_scale } }

    /// The zoom as shown to the user (1.0 = 100 %, see `zoom_unit`).
    pub fn display_zoom(&self) -> f32 { self.view.zoom / self.zoom_unit() }

    /// Switch "1 image pixel = 1 screen pixel"; the zoom percentage stays,
    /// so the picture changes size on a high-density screen.
    pub fn set_pixel_exact(&mut self, on: bool) {
        if on == self.pixel_exact { return; }
        let shown = self.display_zoom();
        self.pixel_exact = on;
        let (unit, c) = (self.zoom_unit(), self.view_center());
        if !self.view.fit { self.view_set_zoom(shown * unit, c); }
    }

    /// The display scale the window is at. The picture keeps its size on
    /// screen. True if it changed.
    pub fn set_dpi_scale(&mut self, new: f32) -> bool {
        if (new - self.dpi_scale).abs() < 1e-3 { return false; }
        let k = new / self.dpi_scale;
        self.dpi_scale = new;
        if !self.pixel_exact && !self.view.fit {
            self.view.zoom = (self.view.zoom * k).clamp(MIN_ZOOM, MAX_ZOOM);
            self.view.pan = (self.view.pan.0 * k, self.view.pan.1 * k);
        }
        true
    }

    /// Current zoom (screen pixels per canvas pixel).
    pub fn zoom(&self) -> f32 { self.view.zoom }

    pub(crate) fn canvas_size(&self) -> (u32, u32) { (self.canvas.width, self.canvas.height) }

    /// Window position (physical pixels) -> canvas pixel, through the
    /// view. Not clamped: off-canvas is fine (the canvas is unbounded).
    pub fn screen_to_canvas(&self, pos: (f32, f32)) -> (f32, f32) { self.view.screen_to_canvas(pos, self.canvas_size(), self.viewport) }

    /// Canvas pixel -> window position (physical pixels).
    pub fn canvas_to_screen(&self, p: (f32, f32)) -> (f32, f32) { self.view.canvas_to_screen(p, self.canvas_size(), self.viewport) }

    /// The current view, to place what is drawn over the canvas (selection
    /// outline...) in points of `ppp` physical pixels.
    pub fn view_snapshot(&self, ppp: f32) -> ViewSnapshot { ViewSnapshot { view: self.view, canvas: self.canvas_size(), area: self.viewport, ppp } }

    /// Window area (physical pixels) where the canvas is drawn: x, y,
    /// width, height. Set from the interface's layout each frame.
    pub fn set_viewport(&mut self, area: Area) { self.viewport = area; }

    /// The canvas area now (x, y, width, height).
    pub fn viewport(&self) -> Area { self.viewport }

    /// `pos` (physical pixels) is in the canvas area.
    pub fn in_viewport(&self, pos: (f32, f32)) -> bool {
        let [vx, vy, vw, vh] = self.viewport;
        pos.0 >= vx && pos.0 < vx + vw && pos.1 >= vy && pos.1 < vy + vh
    }

    /// Center of the canvas area (pivot for keyboard zoom / rotation).
    pub fn view_center(&self) -> (f32, f32) { center(self.viewport) }

    /// Pivot for turning the view (see `CanvasView::rotation_pivot`).
    pub fn rotation_pivot(&self) -> (f32, f32) { self.view.rotation_pivot(self.canvas_size(), self.viewport) }

    pub fn view_zoom_at(&mut self, factor: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.zoom_at(factor, pivot, c, a);
    }

    pub fn view_set_zoom(&mut self, zoom: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.set_zoom(zoom, pivot, c, a);
    }

    pub fn view_rotate_at(&mut self, delta: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.rotate_at(delta, pivot, c, a);
    }

    /// Mirror the view left-right or top-bottom, around the area's middle.
    pub fn view_mirror(&mut self, horizontal: bool) {
        let (c, a) = (self.canvas_size(), self.viewport);
        let pivot = (a[0] + a[2] / 2.0, a[1] + a[3] / 2.0);
        self.view.mirror_at(horizontal, pivot, c, a);
    }

    /// The view's mirror: (left-right, top-bottom).
    pub fn view_mirrored(&self) -> (bool, bool) { self.view.mirror }

    pub fn view_reset_rotation(&mut self) {
        let (c, a, pivot) = (self.canvas_size(), self.viewport, self.rotation_pivot());
        let back = -self.view.angle;
        self.view.rotate_at(back, pivot, c, a);
    }
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
        for mirror in [(false, false), (true, false), (false, true), (true, true)] {
            let v = CanvasView { zoom: 2.7, angle: 0.6, pan: (33.0, -12.0), fit: false, mirror };
            for p in [(0.0, 0.0), (1999.0, 999.0), (512.5, 77.25)] {
                let s = v.canvas_to_screen(p, CANVAS, AREA);
                assert!(close(v.screen_to_canvas(s, CANVAS, AREA), p));
                // The shader's matrix agrees.
                let m = crate::display::screen_to_canvas(&v, CANVAS, AREA);
                let q = ((m[0][0] * s.0 as f64 + m[0][1] * s.1 as f64 + m[0][2]) as f32, (m[1][0] * s.0 as f64 + m[1][1] * s.1 as f64 + m[1][2]) as f32);
                assert!(close(q, p), "{mirror:?}: {q:?} vs {p:?}");
            }
        }
    }

    /// Mirrored left-right: what was left of the middle shows right of it;
    /// the point under the pivot stays.
    #[test]
    fn a_mirrored_view_swaps_left_and_right() {
        let mut v = CanvasView::default();
        v.update_fit(CANVAS, AREA);
        let mid = (AREA[0] + AREA[2] / 2.0, AREA[1] + AREA[3] / 2.0);
        let left = v.canvas_to_screen((100.0, 500.0), CANVAS, AREA);
        v.mirror_at(true, mid, CANVAS, AREA);
        let now = v.canvas_to_screen((100.0, 500.0), CANVAS, AREA);
        assert!(left.0 < mid.0 && now.0 > mid.0 && (now.1 - left.1).abs() < 1e-3);
        assert!(v.fit, "a fitted view stays fitted");
        v.mirror_at(true, mid, CANVAS, AREA);
        assert!(close(v.canvas_to_screen((100.0, 500.0), CANVAS, AREA), left));
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
        let v = CanvasView { zoom: 0.8, angle: -1.1, pan: (5.0, 9.0), fit: false, mirror: (false, false) };
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
