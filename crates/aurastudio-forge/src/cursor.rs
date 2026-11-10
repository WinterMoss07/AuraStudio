//! The brush cursor: the dab a stroke would lay under the pen, as its
//! outline shows it. Whatever changes the dab's shape changes it (the tip,
//! the size, the angle from its setting, the pen or the direction, the
//! roundness from the tilt, the flips, the projection); what doesn't
//! (opacity, flow, color, texture) doesn't. The random parts are left out
//! (the dab without jitter), and while the pen hovers its pressure reads
//! as full.

use crate::brush::{BrushSettings, Control, TipShape};
use crate::dynamics::{apply_pose, base_diameter, dynamic_angle, scalar, StepInput};
use crate::tile::GrayTile;
use crate::StrokePoint;

/// The dab under the pen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorDab {
    /// A canvas offset from the dab's centre (pixels, y down) to the tip's
    /// own coordinates, row-major: a round tip's outline is where they are
    /// 1 from the centre; a sampled tip's picture spans −1..1 both ways
    /// (`(u, v) × ½ + ½`, rows from the top).
    pub to_tip: [f32; 4],
    /// How far the dab can reach from its centre (canvas pixels).
    pub reach: f32,
    /// The outline's radius (canvas pixels): too small, a crosshair.
    pub radius: f32,
    /// A sampled tip (its picture: `cursor_mask`), not a round one.
    pub sampled: bool,
}

/// A sampled tip's picture for the cursor: its alpha, at most `max` on its
/// longer side, with a clear one-pixel border (the outline closes at the
/// tip's edges).
#[derive(Clone, Debug, PartialEq)]
pub struct CursorMask {
    /// The same for the same tip (its pixels are shared, not compared).
    pub id:     u64,
    pub width:  u32,
    pub height: u32,
    pub alpha:  Vec<u8>,
}

fn tip_of(b: &BrushSettings) -> Option<&GrayTile> {
    match &b.tip {
        TipShape::Round => None,
        TipShape::Sampled(g) => Some(g),
        TipShape::Stored(t) => t.full.as_deref(),
    }
}

impl BrushSettings {
    /// The dab at `point` going `direction` (radians, counter-clockwise,
    /// y up), without its random parts. `hover`: the pen is up (its
    /// pressure read as full).
    pub fn cursor_dab(&self, mut point: StrokePoint, direction: f32, hover: bool) -> CursorDab {
        if hover { point.pressure = 1.0; }
        let s = StepInput { point: apply_pose(self, point), direction, initial_direction: direction, step: 0, velocity: 0.0 };
        let sd = &self.shape_dynamics;
        let mut angle = self.angle;
        let mut roundness = self.roundness.clamp(0.01, 1.0);
        let (mut proj_angle, mut proj_scale) = (0.0f32, 1.0f32);
        if sd.enabled {
            angle += dynamic_angle(&sd.angle, &s);
            // (0: no jitter.)
            roundness = (roundness * scalar(&sd.roundness, &s, 0.0)).clamp(0.01, 1.0);
            let tilt = s.point.tilt_x.hypot(s.point.tilt_y).clamp(0.0, 90.0);
            if sd.size.link(Control::PenTilt).is_some() && sd.tilt_scale > 0.0 {
                roundness = (roundness * (1.0 - sd.tilt_scale.clamp(0.0, 2.0) * 0.5 * tilt / 90.0)).clamp(0.01, 1.0);
            }
            if sd.brush_projection {
                angle += s.point.rotation;
                if tilt > 0.0 {
                    proj_angle = (-s.point.tilt_y).atan2(s.point.tilt_x);
                    proj_scale = tilt.to_radians().cos().max(0.05);
                }
            }
        }
        let r = (base_diameter(self, &s) / 2.0).max(0.5);
        let ro = roundness.max(0.5 / r).min(1.0);
        // Canvas offset -> tip space, as the engine samples a dab (see
        // render.rs `to_tip`): y up, projected, turned by −angle, flipped.
        let (sn, cs) = angle.to_radians().sin_cos();
        let (fx, fy) = (if self.flip_x { -1.0 } else { 1.0 }, if self.flip_y { -1.0 } else { 1.0 });
        let y_up = [1.0, 0.0, 0.0, -1.0];
        let proj = if proj_scale < 0.999 {
            let (ps, pc) = proj_angle.sin_cos();
            let k = 1.0 / proj_scale - 1.0;
            [1.0 + k * pc * pc, k * pc * ps, k * pc * ps, 1.0 + k * ps * ps]
        } else {
            [1.0, 0.0, 0.0, 1.0]
        };
        let turn = [cs * fx, sn * fx, -sn * fy, cs * fy];
        let to_space = mul(turn, mul(proj, y_up));
        // Tip space -> the outline's units.
        let (scale, outline, sampled) = match tip_of(self) {
            Some(g) => {
                let big = g.width.max(g.height).max(1) as f32;
                // (Texture x = u·big/2r + w/2, row = −(v/ro)·big/2r + h/2.)
                ([big / (r * g.width.max(1) as f32), 0.0, 0.0, -big / (ro * r * g.height.max(1) as f32)], r, true)
            }
            None => {
                let rh = if self.aliased { r } else { crate::half_coverage_radius(r, self.hardness) };
                ([1.0 / rh, 0.0, 0.0, 1.0 / (rh * ro)], rh, false)
            }
        };
        let reach = r * if sampled { std::f32::consts::SQRT_2 } else { 1.0 } / proj_scale;
        CursorDab { to_tip: mul(scale, to_space), reach, radius: outline, sampled }
    }

    /// The id `cursor_mask` would give (cheap: to know if it is needed).
    pub fn cursor_mask_id(&self, max: u32) -> Option<u64> { tip_of(self).map(|g| mask_id(g, max)) }

    /// The sampled tip's picture for the cursor (None: a round tip).
    pub fn cursor_mask(&self, max: u32) -> Option<CursorMask> {
        let g = tip_of(self)?;
        let (w0, h0) = (g.width.max(1), g.height.max(1));
        let k = (max.max(8) as f32 / w0.max(h0) as f32).min(1.0);
        let (w, h) = (((w0 as f32 * k).round() as u32).max(1), ((h0 as f32 * k).round() as u32).max(1));
        let mut alpha = vec![0u8; ((w + 2) * (h + 2)) as usize];
        // Each pixel: the mean of the tip's pixels under it.
        for y in 0..h {
            let (sy0, sy1) = (y * h0 / h, ((y + 1) * h0 / h).max(y * h0 / h + 1).min(h0));
            for x in 0..w {
                let (sx0, sx1) = (x * w0 / w, ((x + 1) * w0 / w).max(x * w0 / w + 1).min(w0));
                let mut sum = 0u64;
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 { sum += u64::from(g.data[(sy * w0 + sx) as usize]); }
                }
                let n = u64::from((sy1 - sy0) * (sx1 - sx0)).max(1);
                alpha[((y + 1) * (w + 2) + x + 1) as usize] = (sum / n / 257) as u8;
            }
        }
        // Softened a little: the outline follows the tip's shape, not its
        // grain (a chalk's speckles made rings inside).
        let r = (w.max(h) / 48).max(1) as usize;
        for _ in 0..2 { box_blur(&mut alpha, (w + 2) as usize, (h + 2) as usize, r); }
        Some(CursorMask { id: mask_id(g, max), width: w + 2, height: h + 2, alpha })
    }
}

/// A box blur of radius `r` across then down (the border stays clear
/// enough: the outline closes).
fn box_blur(a: &mut [u8], w: usize, h: usize, r: usize) {
    let pass = |a: &mut [u8], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut line = vec![0u32; len];
        for k in 0..count {
            for (i, v) in line.iter_mut().enumerate() { *v = u32::from(a[at(k, i)]); }
            for i in 0..len {
                let (lo, hi) = (i.saturating_sub(r), (i + r).min(len - 1));
                let sum: u32 = line[lo..=hi].iter().sum();
                a[at(k, i)] = (sum / (2 * r as u32 + 1)) as u8;
            }
        }
    };
    pass(a, w, h, &|y, x| y * w + x);
    pass(a, h, w, &|x, y| y * w + x);
}

/// A tip picture's id: where its pixels are (shared, not compared), its
/// size, the size asked.
fn mask_id(g: &GrayTile, max: u32) -> u64 {
    (std::sync::Arc::as_ptr(&g.data) as *const u16 as usize as u64) ^ (u64::from(g.width) << 32) ^ u64::from(g.height) ^ (u64::from(max) << 48)
}

/// `a · b`, 2 × 2 row-major.
fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0] + a[1] * b[2], a[0] * b[1] + a[1] * b[3], a[2] * b[0] + a[3] * b[2], a[2] * b[1] + a[3] * b[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::{Dynamic, Link};

    fn at(m: [f32; 4], x: f32, y: f32) -> (f32, f32) { (m[0] * x + m[1] * y, m[2] * x + m[3] * y) }

    fn pen(pressure: f32, tilt: (f32, f32), rotation: f32) -> StrokePoint {
        StrokePoint { x: 0.0, y: 0.0, pressure, tilt_x: tilt.0, tilt_y: tilt.1, rotation, wheel: 0.0, time: 0.0 }
    }

    /// A hard round 40 px brush: its outline 20 px away, all around.
    #[test]
    fn a_round_tip_outlines_its_radius() {
        let b = BrushSettings { size: 40.0, hardness: 1.0, ..BrushSettings::default() };
        let d = b.cursor_dab(pen(0.3, (0.0, 0.0), 0.0), 0.0, true);
        assert!(!d.sampled);
        // Hovering: full pressure, whatever the pen last said.
        for (x, y) in [(d.radius, 0.0), (0.0, d.radius), (-d.radius * std::f32::consts::FRAC_1_SQRT_2, d.radius * std::f32::consts::FRAC_1_SQRT_2)] {
            let (u, v) = at(d.to_tip, x, y);
            assert!((u.hypot(v) - 1.0).abs() < 1e-3, "{x} {y}: {u} {v}");
        }
        assert!((d.radius - 20.0).abs() < 1.5, "{}", d.radius);
        // Pressing lightly with pressure on size: a smaller dab.
        let pressed = b.cursor_dab(pen(0.25, (0.0, 0.0), 0.0), 0.0, false);
        assert!(pressed.radius < d.radius * 0.4);
    }

    /// An elliptic tip at 90°: its long side stands up; flipped or not,
    /// the same outline for a symmetric tip.
    #[test]
    fn the_angle_and_roundness_turn_the_outline() {
        let b = BrushSettings { size: 40.0, hardness: 1.0, roundness: 0.5, angle: 90.0, ..BrushSettings::default() };
        let d = b.cursor_dab(pen(1.0, (0.0, 0.0), 0.0), 0.0, true);
        let len = |x: f32, y: f32| { let (u, v) = at(d.to_tip, x, y); u.hypot(v) };
        // Up 20 px: on the outline; right 10 px: on it too.
        assert!((len(0.0, -d.radius) - 1.0).abs() < 1e-3);
        assert!((len(d.radius * 0.5, 0.0) - 1.0).abs() < 1e-3);
        assert!(len(d.radius, 0.0) > 1.5);
    }

    /// The angle from the pen's barrel rotation turns the cursor; its
    /// random part does not.
    #[test]
    fn the_pen_turns_it_and_jitter_does_not() {
        let mut b = BrushSettings { size: 40.0, hardness: 1.0, roundness: 0.3, ..BrushSettings::default() };
        b.shape_dynamics.enabled = true;
        b.shape_dynamics.angle = Dynamic { jitter: 1.0, ..Dynamic::default() };
        let still = b.cursor_dab(pen(1.0, (0.0, 0.0), 0.0), 0.0, true);
        assert_eq!(still.to_tip, b.cursor_dab(pen(1.0, (0.0, 0.0), 0.0), 0.0, true).to_tip);
        let _ = b.shape_dynamics.angle.set_link(Control::Rotation, Some(Link { input: Control::Rotation, ..Link::default() }));
        let turned = b.cursor_dab(pen(1.0, (0.0, 0.0), 90.0), 0.0, true);
        assert_ne!(still.to_tip, turned.to_tip);
    }

    /// A sampled tip: its picture, the border clear; the corners of its
    /// box at ±1.
    #[test]
    fn a_sampled_tip_maps_its_box() {
        let g = GrayTile::from_fn(64, 32, |_, _| 1.0);
        let b = BrushSettings { size: 40.0, tip: TipShape::Sampled(g), ..BrushSettings::default() };
        let d = b.cursor_dab(pen(1.0, (0.0, 0.0), 0.0), 0.0, true);
        assert!(d.sampled);
        // 40 px wide, 20 high: (20, −10) is the top right corner.
        let (u, v) = at(d.to_tip, 20.0, -10.0);
        assert!((u - 1.0).abs() < 1e-3 && (v + 1.0).abs() < 1e-3, "{u} {v}");
        let m = b.cursor_mask(16).unwrap();
        assert_eq!((m.width, m.height), (18, 10));
        // (Softened: the border under half, the middle full.)
        assert!(m.alpha[0] < 128);
        assert_eq!(m.alpha[(m.height / 2 * m.width + m.width / 2) as usize], 255);
        assert_eq!(m.id, b.cursor_mask(16).unwrap().id);
    }
}
