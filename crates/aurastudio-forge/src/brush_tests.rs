use aurastudio_core::{FlatSurface, Point, Rect};
use crate::render::{texture_combine, BrushContext};
use crate::stroke::StrokeRenderer;

use crate::brush::*;
use crate::dynamics::{Smoother, smooth_points};
use crate::tile::GrayTile;
use crate::{Dab, Stroke, StrokePoint, dabs};

fn brush() -> BrushSettings {
    BrushSettings { pressure_size: false, spacing: 0.25, ..Default::default() }
}

fn line(x0: f64, x1: f64, y: f64) -> Vec<StrokePoint> {
    vec![StrokePoint::new(x0, y, 1.0), StrokePoint::new(x1, y, 1.0)]
}

fn dabs_of(b: &BrushSettings, pts: &[StrokePoint]) -> Vec<Dab> {
    dabs(&Stroke { brush: b.clone(), points: pts.to_vec() })
}

fn raster(b: &BrushSettings, d: &Dab) -> (Rect, Vec<f32>) {
    let ctx = BrushContext::new(b);
    let r = ctx.dab_rect(d, false);
    let mut v = Vec::new();
    ctx.rasterize(d, false, r, &mut v);
    (r, v)
}

/// A stroke painted on a transparent 512 × 512 layer.
fn paint(b: &BrushSettings, pts: &[StrokePoint], w: i32, h: i32) -> FlatSurface {
    let _ = w + h;
    let mut s = FlatSurface::new(512, 512, [255, 255, 255, 0]);
    let mut r = StrokeRenderer::new(b, 1.0);
    r.push(pts);
    r.finish();
    r.paint(&mut s, true);
    s
}

/// Alpha summed over `r`, 0..1 a pixel.
fn alpha_sum(s: &FlatSurface, r: Rect) -> f32 {
    let mut t = 0.0;
    for y in r.y0..r.y1 { for x in r.x0..r.x1 { t += s.pixel(x as u32, y as u32)[3] as f32 / 255.0; } }
    t
}

/// The alpha down column `x`, rows 100..170.
fn cross_section(b: &BrushSettings, pts: &[StrokePoint], x: i32) -> Vec<f32> {
    let s = paint(b, pts, 0, 0);
    (100..170).map(|y| s.pixel(x as u32, y)[3] as f32 / 255.0).collect()
}

fn at(r: &(Rect, Vec<f32>), x: i32, y: i32) -> f32 {
    r.1[((y - r.0.y0) * r.0.w() as i32 + (x - r.0.x0)) as usize]
}

// ---------- tips ----------

#[test]
fn round_tip_roundness_and_angle() {
    let b = brush();
    let mut d = Dab::round(Point::new(50.0, 50.0), 20.0, 1.0);
    d.roundness = 0.25;
    let r = raster(&b, &d);
    // Wide along x, thin along y.
    assert!(at(&r, 50 + 15, 50) > 0.99);
    assert!(at(&r, 50, 50 + 8) < 0.01);
    assert!(at(&r, 50, 50 + 3) > 0.99);
    // Rotated 90° counter-clockwise: now tall.
    d.angle = std::f32::consts::FRAC_PI_2;
    let r = raster(&b, &d);
    assert!(at(&r, 50, 50 - 15) > 0.99);
    assert!(at(&r, 50 + 8, 50) < 0.01);
    // 45°: the long axis runs up-right (y-down image space: x+, y−).
    d.angle = std::f32::consts::FRAC_PI_4;
    let r = raster(&b, &d);
    assert!(at(&r, 60, 40) > 0.99, "{}", at(&r, 60, 40));
    assert!(at(&r, 60, 60) < 0.01);
}

#[test]
fn hardness_profile_is_monotonic() {
    for h in [0.0f32, 0.5, 1.0] {
        let b = BrushSettings { hardness: h, ..brush() };
        let r = raster(&b, &Dab::round(Point::new(50.0, 50.0), 20.0, 1.0));
        // A soft tip paints past its 20 px radius, as Photoshop's does, out to its reach.
        let n = crate::tip_reach(20.0, h).ceil() as i32 + 1;
        let prof: Vec<f32> = (0..n).map(|i| at(&r, 50 + i, 50)).collect();
        assert!(prof.windows(2).all(|w| w[1] <= w[0] + 1e-6), "h={h} {prof:?}");
        assert!(prof[0] > 0.99);
        assert!(*prof.last().unwrap() < 0.002, "h={h} {prof:?}");
        let mid = at(&r, 60, 50);
        if h == 1.0 {
            assert!(mid > 0.99);
        } else if h == 0.0 {
            assert!(mid > 0.2 && mid < 0.8, "{mid}");
        }
    }
}

#[test]
fn hard_edge_is_sharp() {
    // A hardness of 1 is a steep rim: fully opaque inside the nominal radius, empty just past
    // it, with only the anti-aliasing band in between.
    let b = BrushSettings { hardness: 1.0, ..brush() };
    let r = raster(&b, &Dab::round(Point::new(50.0, 50.0), 20.0, 1.0));
    let prof: Vec<f32> = (0..22).map(|i| at(&r, 50 + i, 50)).collect();
    let band = prof.iter().filter(|&&a| (0.01..0.99).contains(&a)).count();
    assert!(band <= 3, "hardness 1 rim is {band} px wide: {prof:?}");
    assert!(prof[17] > 0.99, "translucent inside the rim: {prof:?}");
    assert!(*prof.last().unwrap() < 0.01, "{prof:?}");
    // A soft tip spreads the same crossing over many pixels instead.
    let soft = BrushSettings { hardness: 0.0, ..brush() };
    let rs = raster(&soft, &Dab::round(Point::new(50.0, 50.0), 20.0, 1.0));
    let profs: Vec<f32> = (0..22).map(|i| at(&rs, 50 + i, 50)).collect();
    let soft_band = profs.iter().filter(|&&a| (0.01..0.99).contains(&a)).count();
    assert!(soft_band >= 10, "soft rim is only {soft_band} px: {profs:?}");
}

#[test]
fn sampled_tip_is_scaled_and_oriented() {
    // Tip: left half painted.
    let tip = GrayTile::from_fn(16, 16, |x, _| if x < 8 { 1.0 } else { 0.0 });
    let b = BrushSettings { tip: TipShape::Sampled(tip), ..brush() };
    let d = Dab::round(Point::new(50.0, 50.0), 20.0, 1.0);
    let r = raster(&b, &d);
    assert!(at(&r, 40, 50) > 0.99 && at(&r, 60, 50) < 0.01);
    // Flip X mirrors it.
    let r = raster(&b, &Dab { flip_x: true, ..d });
    assert!(at(&r, 40, 50) < 0.01 && at(&r, 60, 50) > 0.99);
    // Rotated 90° CCW: the painted (left) half moves to the bottom.
    let r = raster(&b, &Dab { angle: std::f32::consts::FRAC_PI_2, ..d });
    assert!(at(&r, 50, 60) > 0.99 && at(&r, 50, 40) < 0.01);
    // Heavily downscaled uses mips: still sensible coverage.
    let r = raster(&b, &Dab::round(Point::new(50.0, 50.0), 2.0, 1.0));
    assert!(r.1.iter().sum::<f32>() > 1.0);
}
// ---------- spacing ----------

#[test]
fn spacing_sets_dab_count() {
    for (spacing, len, expect) in [(0.25f32, 100.0, 21usize), (1.0, 100.0, 6), (0.5, 200.0, 21)] {
        let b = BrushSettings { spacing, size: 20.0, ..brush() };
        assert_eq!(dabs_of(&b, &line(0.0, len, 0.0)).len(), expect, "spacing {spacing}");
    }
    // Spacing carries across segments: a polyline gives the same count as the straight line.
    let b = BrushSettings { spacing: 0.5, size: 20.0, ..brush() };
    let pts: Vec<StrokePoint> = (0..=20).map(|i| StrokePoint::new(i as f64 * 10.0, 0.0, 1.0)).collect();
    assert_eq!(dabs_of(&b, &pts).len(), 21);
    let d = dabs_of(&b, &pts);
    assert!(d.windows(2).all(|w| ((w[1].center.x - w[0].center.x) - 10.0).abs() < 1e-6));
}

// ---------- dynamics ----------

#[test]
fn pressure_controls_size_with_minimum() {
    let b = BrushSettings {
        size: 40.0,
        shape_dynamics: ShapeDynamics {
            enabled: true,
            size: Dynamic { control: Control::PenPressure, minimum: 0.25, ..Default::default() },
            ..Default::default()
        },
        ..brush()
    };
    let pts = vec![StrokePoint::new(0.0, 0.0, 0.0), StrokePoint::new(200.0, 0.0, 1.0)];
    let d = dabs_of(&b, &pts);
    assert!((d[0].radius - 5.0).abs() < 1e-3, "min diameter 25 % of 40 → r 5, got {}", d[0].radius);
    assert!((d.last().unwrap().radius - 20.0).abs() < 0.5);
    assert!(d.windows(2).all(|w| w[1].radius >= w[0].radius - 1e-4));
}

#[test]
fn each_dynamic_responds_through_its_own_curve() {
    // A hard size curve (half pressure gives a fifth of the size range) and a linear opacity.
    let curve = crate::ResponseCurve::new(&[[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]);
    let b = BrushSettings {
        size: 40.0,
        shape_dynamics: ShapeDynamics { enabled: true, size: Dynamic { curve, ..Dynamic::controlled(Control::PenPressure) }, ..Default::default() },
        transfer: Transfer { enabled: true, opacity: Dynamic::controlled(Control::PenPressure), ..Default::default() },
        ..brush()
    };
    let d = dabs_of(&b, &[StrokePoint::new(0.0, 0.0, 0.5), StrokePoint::new(1.0, 0.0, 0.5)]);
    assert!((d[0].radius - 4.0).abs() < 1e-3, "20 % of a 40 px diameter → r 4, got {}", d[0].radius);
    assert!((d[0].opacity - 0.5).abs() < 1e-3, "opacity keeps its linear response: {}", d[0].opacity);
    // The curve is part of the brush's JSON.
    let json = serde_json::to_value(&b).unwrap();
    let stored: Vec<[f32; 2]> = serde_json::from_value(json["shapeDynamics"]["size"]["curve"].clone()).unwrap();
    assert_eq!(stored, vec![[0.0, 0.0], [0.5, 0.2], [1.0, 1.0]]);
    assert_eq!(json["transfer"]["opacity"]["curve"], serde_json::json!([[0.0, 0.0], [1.0, 1.0]]));
    let back: BrushSettings = serde_json::from_value(json).unwrap();
    assert_eq!(back.shape_dynamics.size.curve, curve);
}

#[test]
fn fade_ramps_over_steps() {
    let b = BrushSettings {
        size: 20.0,
        spacing: 0.5,
        shape_dynamics: ShapeDynamics {
            enabled: true,
            size: Dynamic { control: Control::Fade, fade_steps: 10, minimum: 0.0, ..Default::default() },
            ..Default::default()
        },
        ..brush()
    };
    let d = dabs_of(&b, &line(0.0, 300.0, 0.0));
    assert!((d[0].radius - 10.0).abs() < 1e-3);
    assert!(d[5].radius < d[0].radius * 0.6, "{}", d[5].radius);
    assert!(d.iter().skip(10).all(|x| x.radius <= 0.5 + 1e-6));
    // Fade on opacity (Transfer).
    let b = BrushSettings {
        transfer: Transfer { enabled: true, opacity: Dynamic { control: Control::Fade, fade_steps: 4, ..Default::default() }, ..Default::default() },
        ..brush()
    };
    let d = dabs_of(&b, &line(0.0, 100.0, 0.0));
    assert_eq!(d[0].opacity, 1.0);
    assert_eq!(d[4].opacity, 0.0);
}

#[test]
fn tilt_direction_and_rotation_drive_angle() {
    let mk = |c: Control| BrushSettings { shape_dynamics: ShapeDynamics { enabled: true, angle: Dynamic::controlled(c), ..Default::default() }, ..brush() };
    let mut p = StrokePoint::new(0.0, 0.0, 1.0);
    p.tilt_x = 0.0;
    p.tilt_y = -45.0; // pen leaning "up" the screen
    let pts = vec![p, StrokePoint { x: 50.0, ..p }];
    let d = dabs_of(&mk(Control::PenTilt), &pts);
    assert!((d[0].angle.to_degrees() - 90.0).abs() < 1e-3, "{}", d[0].angle.to_degrees());
    // Direction: travelling down the screen = -90° (counter-clockwise positive).
    let pts = vec![StrokePoint::new(0.0, 0.0, 1.0), StrokePoint::new(0.0, 50.0, 1.0)];
    let d = dabs_of(&mk(Control::Direction), &pts);
    assert!(d.iter().all(|x| (x.angle.to_degrees() + 90.0).abs() < 1e-3));
    // Rotation.
    let mut q = StrokePoint::new(0.0, 0.0, 1.0);
    q.rotation = 30.0;
    let d = dabs_of(&mk(Control::Rotation), &[q, StrokePoint { x: 20.0, ..q }]);
    assert!((d[0].angle.to_degrees() - 30.0).abs() < 1e-3);
    // Pen tilt on size: upright pen full size, flat pen minimum.
    let b = BrushSettings {
        shape_dynamics: ShapeDynamics { enabled: true, size: Dynamic::controlled(Control::PenTilt), ..Default::default() },
        size: 20.0,
        ..brush()
    };
    let mut flat = StrokePoint::new(0.0, 0.0, 1.0);
    flat.tilt_x = 90.0;
    let d = dabs_of(&b, &[flat]);
    assert!(d[0].radius <= 0.5 + 1e-6);
}

#[test]
fn jitter_is_deterministic_per_seed() {
    let b = BrushSettings {
        size: 30.0,
        seed: 42,
        shape_dynamics: ShapeDynamics {
            enabled: true,
            size: Dynamic::jitter(1.0),
            angle: Dynamic::jitter(1.0),
            roundness: Dynamic { jitter: 1.0, minimum: 0.2, ..Default::default() },
            flip_x_jitter: true,
            flip_y_jitter: true,
            ..Default::default()
        },
        scattering: Scattering { enabled: true, scatter: Dynamic::jitter(2.0), both_axes: true, count: 3, count_jitter: Dynamic::jitter(1.0) },
        ..brush()
    };
    let pts = line(0.0, 300.0, 50.0);
    let a = dabs_of(&b, &pts);
    assert_eq!(a, dabs_of(&b, &pts));
    let c = dabs_of(&BrushSettings { seed: 43, ..b.clone() }, &pts);
    assert_ne!(a, c);
    // Jitter actually varies things, within their ranges.
    let radii: Vec<f32> = a.iter().map(|d| d.radius).collect();
    assert!(radii.iter().any(|&r| r < 10.0) && radii.iter().all(|&r| r <= 15.0 + 1e-4));
    assert!(a.iter().all(|d| d.roundness >= 0.2 - 1e-6 && d.roundness <= 1.0));
    assert!(a.iter().any(|d| d.flip_x) && a.iter().any(|d| !d.flip_x));
    // Chunked feeding gives identical dabs.
    let mut r = StrokeRenderer::new(&b, 1.0).record_dabs();
    let many: Vec<StrokePoint> = (0..=30).map(|i| StrokePoint::new(i as f64 * 10.0, 50.0, 1.0)).collect();
    for ch in many.chunks(4) {
        r.push(ch);
    }
    r.finish();
    assert_eq!(r.dabs(), dabs_of(&b, &many).as_slice());
}

#[test]
fn scattering_stays_in_bounds() {
    for both in [false, true] {
        let b = BrushSettings {
            size: 20.0,
            scattering: Scattering { enabled: true, scatter: Dynamic::jitter(2.0), both_axes: both, count: 4, ..Default::default() },
            ..brush()
        };
        let d = dabs_of(&b, &line(0.0, 400.0, 100.0));
        // count 4 per spacing step (5 px) → 81 steps.
        assert_eq!(d.len(), 81 * 4);
        let max_off = 2.0 * 10.0;
        assert!(d.iter().all(|x| (x.center.y - 100.0).abs() <= max_off + 1e-6));
        assert!(d.iter().any(|x| (x.center.y - 100.0).abs() > max_off * 0.5), "scatter too small");
        if !both {
            // Perpendicular only: x stays on the spacing grid.
            assert!(d.iter().all(|x| (x.center.x / 5.0 - (x.center.x / 5.0).round()).abs() < 1e-6));
        } else {
            assert!(d.iter().any(|x| (x.center.x / 5.0 - (x.center.x / 5.0).round()).abs() > 0.1));
        }
    }
}

#[test]
fn zero_scatter_keeps_dabs_on_the_stroke() {
    // Scatter at 0 % must not move a dab, even with the section switched on.
    let on = BrushSettings { scattering: Scattering { enabled: true, ..Default::default() }, ..brush() };
    for b in [brush(), on] {
        let d = dabs_of(&b, &line(10.0, 90.0, 40.5));
        assert!(d.len() > 4);
        assert!(
            d.iter().all(|x| (x.center.y - 40.5).abs() < 1e-9),
            "scatter moved dabs off the stroke: {:?}",
            d.iter().map(|x| x.center.y).collect::<Vec<_>>()
        );
    }
}

#[test]
fn huge_airbrush_time_gap_emits_bounded_catch_up() {
    let b = BrushSettings { build_up: true, build_up_rate: 1000.0, ..brush() };
    let a = StrokePoint { time: 0.0, ..StrokePoint::new(20.0, 20.0, 1.0) };
    let brief_pause = StrokePoint { time: 200.0, ..a };
    assert_eq!(dabs_of(&b, &[a, brief_pause]).len(), 201, "a 200 ms pause should retain its airbrush build-up");

    let z = StrokePoint { time: f64::MAX, ..a };
    let d = dabs_of(&b, &[a, z]);
    assert!(d.len() <= 2, "a huge pause should resume with one dab, not replay its time backlog: {}", d.len());
    assert_eq!(d.last().map(|dab| dab.center), Some(Point::new(20.0, 20.0)));

    let infinite = StrokePoint { time: f64::INFINITY, ..a };
    assert!(dabs_of(&b, &[a, infinite]).len() <= 1);
}

#[test]
fn unbounded_brush_dimensions_are_safe_in_the_public_renderer() {
    let brush = BrushSettings { size: f32::MAX, dual_brush: DualBrush { enabled: true, size: f32::INFINITY, ..Default::default() }, ..brush() };
    let ctx = BrushContext::new(&brush);
    assert_eq!(ctx.brush.size, crate::MAX_BRUSH_SIZE);
    assert_eq!(ctx.brush.dual_brush.size, 0.5);

    let d = dabs_of(&brush, &[StrokePoint::new(10.0, 10.0, 1.0)]);
    assert!(d.iter().all(|dab| dab.radius <= crate::MAX_BRUSH_SIZE / 2.0));
}

/// Texture Brightness and Contrast as Photoshop applies them, measured by stroking a 0..255 grey
/// ramp pattern in Multiply mode at 100 % depth (the stroke's opacity is then the pattern value):
/// (setting, ramp value, opacity read back), in levels of 255.
#[test]
fn texture_brightness_and_contrast_match_photoshop() {
    let level = |brightness: f32, contrast: f32, invert: bool, v: f32| {
        // The model's ranges: brightness ±150 → ±1; contrast -50..0 → -1..0, 0..100 → 0..1.
        let c = if contrast < 0.0 { contrast / 50.0 } else { contrast / 100.0 };
        let t = Texture { brightness: brightness / 150.0, contrast: c, invert, ..Default::default() };
        t.level(v / 255.0) * 255.0
    };
    for (b, c, inv, v, want) in [
        (87.0, 0.0, false, 8.0, 95.0),
        (87.0, 0.0, false, 200.0, 255.0),
        (-87.0, 0.0, false, 104.0, 17.0),
        (150.0, 0.0, false, 8.0, 158.0),
        (0.0, 50.0, false, 104.0, 81.0),
        (0.0, 50.0, false, 136.0, 145.0),
        (0.0, -50.0, false, 8.0, 67.0),
        (0.0, -50.0, false, 248.0, 188.0),
        (0.0, 100.0, false, 120.0, 0.0),
        (0.0, 100.0, false, 136.0, 255.0),
        // Positive contrast after the shift, negative contrast before it.
        (40.0, 50.0, false, 104.0, 161.0),
        (40.0, 50.0, false, 40.0, 33.0),
        (-40.0, -30.0, false, 120.0, 82.0),
        (-40.0, -30.0, false, 248.0, 172.0),
        // Invert comes last.
        (40.0, 0.0, true, 8.0, 207.0),
        (0.0, 50.0, true, 72.0, 238.0),
    ] {
        let got = level(b, c, inv, v);
        assert!((got - want).abs() <= 2.0, "brightness {b}, contrast {c}, invert {inv}, ramp {v}: {got} (Photoshop {want})");
    }
}

/// The Texture modes that differ from the Dual Brush's, against single Photoshop dabs over a
/// 0..255 ramp: (mode, depth, brush coverage, ramp value, opacity read back), in levels of 255.
#[test]
fn texture_modes_match_photoshop() {
    use MaskMode::*;
    for (m, d, v, t, want) in [
        // Height: a low depth paints the dark valleys only, more depth fills the pattern.
        (Height, 0.0, 255.0, 8.0, 0.0),
        (Height, 0.01, 255.0, 8.0, 28.0),
        (Height, 0.05, 255.0, 8.0, 148.0),
        (Height, 0.05, 255.0, 136.0, 20.0),
        (Height, 0.13, 255.0, 8.0, 255.0),
        (Height, 0.13, 255.0, 152.0, 244.0),
        (Height, 0.13, 255.0, 248.0, 148.0),
        (Height, 0.13, 186.0, 88.0, 201.0),
        (Height, 0.13, 142.0, 104.0, 116.0),
        (Height, 0.25, 255.0, 248.0, 255.0),
        // Linear Height: graded by the pattern while the reach stays below its top.
        (LinearHeight, 0.01, 255.0, 152.0, 15.0),
        (LinearHeight, 0.05, 255.0, 8.0, 151.0),
        (LinearHeight, 0.05, 255.0, 152.0, 63.0),
        (LinearHeight, 0.10, 255.0, 72.0, 240.0),
        (LinearHeight, 0.13, 101.0, 120.0, 82.0),
        (LinearHeight, 0.13, 142.0, 104.0, 130.0),
        // Subtract: the light parts take paint away.
        (Subtract, 1.0, 255.0, 8.0, 247.0),
        (Subtract, 1.0, 255.0, 248.0, 7.0),
        (Subtract, 0.5, 224.0, 72.0, 188.0),
        // Color Burn / Color Dodge below full depth.
        (ColorBurn, 0.5, 224.0, 72.0, 207.0),
        (ColorDodge, 0.5, 99.0, 120.0, 130.0),
        // Unchanged modes.
        (Multiply, 0.13, 255.0, 8.0, 223.0),
        (Darken, 0.5, 255.0, 8.0, 131.0),
    ] {
        let got = texture_combine(m, v / 255.0, t / 255.0, d) * 255.0;
        assert!((got - want).abs() <= 3.0, "{m:?} depth {d}, coverage {v}, ramp {t}: {got} (Photoshop {want})");
    }
}

#[test]
fn smoothing_modes() {
    let zig: Vec<StrokePoint> = (0..40).map(|i| StrokePoint::new(i as f64 * 5.0, if i % 2 == 0 { 0.0 } else { 6.0 }, 1.0)).collect();
    let dev = |pts: &[StrokePoint]| pts.iter().map(|p| (p.y - 3.0).abs()).sum::<f64>() / pts.len() as f64;
    // Exponential smoothing reduces the zig-zag.
    let cfg = Smoothing { amount: 0.8, ..Default::default() };
    let sm = smooth_points(&cfg, 1.0, &zig);
    assert!(dev(&sm[5..sm.len() - 1]) < dev(&zig) * 0.5);
    // Catch-up on end finishes at the last input point; without it, the stroke ends short.
    assert_eq!(sm.last().map(|p| (p.x, p.y)), zig.last().map(|p| (p.x, p.y)));
    let no = smooth_points(&Smoothing { catch_up_on_end: false, ..cfg.clone() }, 1.0, &zig);
    assert!(no.last().unwrap().x < zig.last().unwrap().x - 1.0);
    // Pulled string: jitter smaller than the string moves nothing; the brush trails by the string.
    let ps = Smoothing { amount: 0.2, pulled_string: true, catch_up_on_end: false, ..Default::default() };
    let jitter: Vec<StrokePoint> = (0..20).map(|i| StrokePoint::new(if i % 2 == 0 { 0.0 } else { 5.0 }, 0.0, 1.0)).collect();
    assert_eq!(smooth_points(&ps, 1.0, &jitter).len(), 1, "within the 20 px string: no motion");
    let far = smooth_points(&ps, 1.0, &[StrokePoint::new(0.0, 0.0, 1.0), StrokePoint::new(100.0, 0.0, 1.0)]);
    assert!((far.last().unwrap().x - 80.0).abs() < 1e-4);
    // Adjust for zoom: at 200 % zoom the string is half as long in image pixels.
    let far = smooth_points(&ps, 2.0, &[StrokePoint::new(0.0, 0.0, 1.0), StrokePoint::new(100.0, 0.0, 1.0)]);
    assert!((far.last().unwrap().x - 90.0).abs() < 1e-4);
    // Stroke catch-up: a pause (timestamps) lets the brush converge on the pointer.
    let mut a = StrokePoint::new(0.0, 0.0, 1.0);
    a.time = 0.0;
    let b = StrokePoint { x: 100.0, time: 16.0, ..a };
    let c = StrokePoint { time: 500.0, ..b };
    let mut s = Smoother::new(&Smoothing { amount: 0.8, catch_up: true, catch_up_on_end: false, ..Default::default() }, 1.0);
    let mut out = Vec::new();
    for p in [a, b, c] {
        s.push(p, &mut out);
    }
    assert!(out.last().unwrap().x > 99.0, "{}", out.last().unwrap().x);
    let mut s = Smoother::new(&Smoothing { amount: 0.8, catch_up: false, catch_up_on_end: false, ..Default::default() }, 1.0);
    let mut out = Vec::new();
    for p in [a, b, c] {
        s.push(p, &mut out);
    }
    assert!(out.last().unwrap().x < 70.0, "{}", out.last().unwrap().x);
}

#[test]
fn smoothing_string_runs_from_the_lagging_brush_to_the_pointer() {
    // the canvas draws the line between the brush and the pointer.
    let cfg = Smoothing { amount: 0.8, ..Default::default() };
    let mut s = Smoother::new(&cfg, 1.0);
    let mut out = Vec::new();
    assert_eq!(s.string(), None, "no stroke yet");
    s.push(StrokePoint::new(0.0, 0.0, 1.0), &mut out);
    assert_eq!(s.string(), None, "the first point is the brush");
    s.push(StrokePoint::new(100.0, 0.0, 1.0), &mut out);
    let (brush, pointer) = s.string().unwrap();
    assert!(brush[0] > 0.0 && brush[0] < 100.0, "the brush lags: {brush:?}");
    assert_eq!(pointer, [100.0, 0.0]);
    let mut off = Smoother::new(&Smoothing { amount: 0.0, ..cfg }, 1.0);
    off.push(StrokePoint::new(0.0, 0.0, 1.0), &mut out);
    off.push(StrokePoint::new(100.0, 0.0, 1.0), &mut out);
    assert_eq!(off.string(), None, "smoothing off: no string");
}

#[test]
fn smoothing_pulls_by_time_not_by_how_often_the_device_reports() {
    // The pointer jumps 100 px and is held there for 32 ms, reported at 1000 Hz or about 60 Hz.
    let cfg = Smoothing { amount: 0.8, catch_up: true, catch_up_on_end: false, ..Default::default() };
    let run = |interval: f64| {
        let mut s = Smoother::new(&cfg, 1.0);
        let mut out = Vec::new();
        s.push(StrokePoint::new(0.0, 0.0, 1.0), &mut out);
        let mut t = 0.0;
        while t < 32.0 {
            t += interval;
            s.push(StrokePoint { time: t, ..StrokePoint::new(100.0, 0.0, 1.0) }, &mut out);
        }
        out.last().map_or(0.0, |p| p.x)
    };
    let (fast, slow) = (run(1.0), run(16.0));
    // Two 16 ms steps of a 0.2 pull: 1 - 0.8² = 36 % of the way.
    assert!((slow - 36.0).abs() < 1e-4, "{slow}");
    assert!((fast - slow).abs() < 1e-4, "1000 Hz {fast} vs 60 Hz {slow}");
    // Without timestamps each point is one step, as before.
    let mut s = Smoother::new(&cfg, 1.0);
    let mut out = Vec::new();
    for x in [0.0, 100.0] {
        s.push(StrokePoint::new(x, 0.0, 1.0), &mut out);
    }
    assert!((out.last().map_or(0.0, |p| p.x) - 20.0).abs() < 1e-4);
}

#[test]
fn smoothing_without_catch_up_stops_while_the_pointer_pauses() {
    let cfg = Smoothing { amount: 0.8, catch_up: false, catch_up_on_end: false, ..Default::default() };
    let mut s = Smoother::new(&cfg, 1.0);
    let mut out = Vec::new();
    s.push(StrokePoint::new(0.0, 0.0, 1.0), &mut out);
    s.push(StrokePoint { time: 16.0, ..StrokePoint::new(100.0, 0.0, 1.0) }, &mut out);
    let n = out.len();
    for t in [32.0, 48.0, 64.0] {
        s.push(StrokePoint { time: t, ..StrokePoint::new(100.0, 0.0, 1.0) }, &mut out);
    }
    assert_eq!(out.len(), n, "held still: the brush doesn't move");
}

#[test]
fn a_smoothed_brush_keeps_the_pressure_of_where_it_is_when_the_pen_lifts() {
    // a fast stroke at full pressure, lifted in the last sample. The brush trails far
    // behind the pointer; it used to take the lift's pressure there, and the end catch-up drew
    // a hairline from the trailing brush to the pointer.
    let pts: Vec<StrokePoint> =
        (0..=20).map(|i| StrokePoint { time: f64::from(i) * 4.0, ..StrokePoint::new(f64::from(i) * 10.0, 0.0, if i < 20 { 1.0 } else { 0.05 }) }).collect();
    let out = smooth_points(&Smoothing { amount: 0.8, ..Default::default() }, 1.0, &pts);
    let (last, rest) = out.split_last().unwrap();
    assert_eq!((last.x, last.pressure), (200.0, 0.05), "the catch-up still ends at the pen's last sample");
    let lifted = rest.last().unwrap();
    assert!(lifted.x < 190.0, "the brush lags the pointer: {}", lifted.x);
    for p in rest {
        assert!(p.pressure > 0.5, "the trailing brush at x {} has pressure {}", p.x, p.pressure);
    }
}

#[test]
fn a_pulled_string_brush_takes_the_pose_along_the_string() {
    let ps = Smoothing { amount: 0.2, pulled_string: true, catch_up_on_end: false, ..Default::default() };
    let a = StrokePoint { rotation: 350.0, ..StrokePoint::new(0.0, 0.0, 1.0) };
    let b = StrokePoint { rotation: 10.0, tilt_x: 40.0, ..StrokePoint::new(100.0, 0.0, 0.0) };
    let out = smooth_points(&ps, 1.0, &[a, b]);
    // The 20 px string leaves the brush 80 % of the way to the pointer.
    let p = out.last().unwrap();
    assert!((p.x - 80.0).abs() < 1e-4, "{}", p.x);
    assert!((p.pressure - 0.2).abs() < 1e-4, "{}", p.pressure);
    assert!((p.tilt_x - 32.0).abs() < 1e-3, "{}", p.tilt_x);
    // Rotation the short way round: 350° + 80 % of 20°.
    assert!((p.rotation - 6.0).abs() < 1e-3, "{}", p.rotation);
}

#[test]
fn a_smoothed_brush_catches_up_with_a_pen_lifted_in_place() {
    // only the pressure changes while the pointer stands still. The brush lags in pose,
    // so catch-up must keep running and the stroke end at the pen's last pressure.
    let cfg = Smoothing { amount: 0.8, ..Default::default() };
    let mut s = Smoother::new(&cfg, 1.0);
    let mut out = Vec::new();
    s.push(StrokePoint::new(50.0, 50.0, 1.0), &mut out);
    s.push(StrokePoint { time: 32.0, ..StrokePoint::new(50.0, 50.0, 0.0) }, &mut out);
    assert!(out.last().unwrap().pressure > 0.5, "the brush trails in pressure: {:?}", out.last());
    assert!(s.lagging(), "a pose behind the pointer is still catching up");
    // Held long enough, the brush arrives and stops asking for time.
    for i in 1..=200 {
        s.push(StrokePoint { time: 32.0 + f64::from(i) * 16.0, ..StrokePoint::new(50.0, 50.0, 0.0) }, &mut out);
    }
    assert!(!s.lagging(), "{:?}", out.last());
    s.finish(&mut out);
    assert_eq!(out.last().unwrap().pressure, 0.0);
}

#[test]
fn catch_up_on_stroke_end_finishes_the_pose_of_a_still_pen() {
    // tilt, rotation and wheel changed in place; the end catch-up lands on all of them.
    let cfg = Smoothing { amount: 0.8, ..Default::default() };
    let a = StrokePoint { rotation: 350.0, ..StrokePoint::new(50.0, 50.0, 1.0) };
    let b = StrokePoint { time: 16.0, tilt_x: 40.0, tilt_y: -20.0, rotation: 10.0, wheel: 0.5, ..StrokePoint::new(50.0, 50.0, 1.0) };
    let mut s = Smoother::new(&cfg, 1.0);
    let mut out = Vec::new();
    s.push(a, &mut out);
    s.push(b, &mut out);
    assert!(s.lagging());
    s.finish(&mut out);
    let p = out.last().unwrap();
    assert_eq!((p.tilt_x, p.tilt_y, p.rotation, p.wheel), (40.0, -20.0, 10.0, 0.5));
    // Nothing left to catch up: no extra point.
    let n = out.len();
    s.finish(&mut out);
    assert_eq!(out.len(), n);
}

#[test]
fn pose_axes_override_only_the_selected_stylus_components() {
    let input = StrokePoint { x: 20.0, y: 30.0, pressure: 0.71, tilt_x: 13.0, tilt_y: -27.0, rotation: 67.0, wheel: 0.24, time: 73.0 };
    for enabled in [false, true] {
        for (x, y) in [(false, false), (true, false), (false, true), (true, true)] {
            let b: BrushSettings = serde_json::from_value(serde_json::json!({"pose": {
                "enabled": enabled, "tiltX": 45, "tiltY": -18,
                "overrideTiltX": x, "overrideTiltY": y,
                "pressure": 0.4, "rotation": 120
            }}))
            .unwrap();
            let mut expected = input;
            if enabled && x {
                expected.tilt_x = 45.0;
            }
            if enabled && y {
                expected.tilt_y = -18.0;
            }
            assert_eq!(crate::dynamics::apply_pose(&b, input), expected, "enabled={enabled}, X={x}, Y={y}");
        }
    }
}

#[test]
fn pose_axes_legacy_combined_override_and_existing_rotation_pressure_are_preserved() {
    let input = StrokePoint { tilt_x: 13.0, tilt_y: -27.0, pressure: 0.71, rotation: 67.0, ..Default::default() };
    for enabled in [false, true] {
        for legacy in [false, true] {
            let b: BrushSettings = serde_json::from_value(serde_json::json!({"pose": {
                "enabled": enabled, "tiltX": 45, "tiltY": -18, "overrideTilt": legacy,
                "pressure": 0.4, "overridePressure": true, "rotation": 120, "overrideRotation": true
            }}))
            .unwrap();
            let expected = StrokePoint {
                tilt_x: if enabled && legacy { 45.0 } else { input.tilt_x },
                tilt_y: if enabled && legacy { -18.0 } else { input.tilt_y },
                pressure: if enabled { 0.4 } else { input.pressure },
                rotation: if enabled { 120.0 } else { input.rotation },
                ..input
            };
            assert_eq!(crate::dynamics::apply_pose(&b, input), expected);
            let stored = serde_json::to_vec(&b).unwrap();
            let restored: BrushSettings = serde_json::from_slice(&stored).unwrap();
            assert_eq!(crate::dynamics::apply_pose(&restored, input), expected, "legacy brush settings retain their painting behavior through persistence");
        }
    }
}

#[test]
fn pose_overrides_stylus_values() {
    let b = BrushSettings {
        size: 40.0,
        pose: Pose { enabled: true, pressure: 0.5, override_pressure: true, ..Default::default() },
        shape_dynamics: ShapeDynamics { enabled: true, size: Dynamic::controlled(Control::PenPressure), ..Default::default() },
        ..brush()
    };
    let d = dabs_of(&b, &line(0.0, 50.0, 0.0));
    assert!(d.iter().all(|x| (x.radius - 10.0).abs() < 1e-4));
}

#[test]
fn protect_texture_keeps_pattern() {
    let presets = crate::presets::builtin();
    let chalk = crate::presets::find(&presets, "chalk").unwrap().brush.clone();
    let canvas = crate::presets::find(&presets, "Canvas Texture").unwrap().brush.clone();
    let cur = BrushSettings { protect_texture: true, ..chalk.clone() };
    let next = canvas.clone().with_protected_texture(&cur);
    assert_eq!(next.texture.pattern, chalk.texture.pattern);
    assert_eq!(next.texture.mode, canvas.texture.mode);
    assert_eq!(canvas.clone().with_protected_texture(&chalk).texture.pattern, canvas.texture.pattern);
}

// ---------- Tilt Scale, Brush Projection, speed spacing, locks, mixer transfer ----------

/// Coverage extent (pixels above half coverage) of a rasterised dab along x and y through its centre.
fn extents(b: &BrushSettings, d: &Dab) -> (usize, usize) {
    let r = raster(b, d);
    let (cx, cy) = (d.center.x.floor() as i32, d.center.y.floor() as i32);
    let w = (r.0.x0..r.0.x1).filter(|&x| at(&r, x, cy) > 0.5).count();
    let h = (r.0.y0..r.0.y1).filter(|&y| at(&r, cx, y) > 0.5).count();
    (w, h)
}

fn pen(x: f64, y: f64, tilt_x: f32, tilt_y: f32, rotation: f32) -> StrokePoint {
    StrokePoint { tilt_x, tilt_y, rotation, ..StrokePoint::new(x, y, 1.0) }
}

#[test]
fn tilt_scale_squashes_the_tip_height_with_pen_tilt() {
    // Size control on Pen Tilt with a 100 % minimum, so only Tilt Scale changes the shape.
    let mk = |scale: f32| BrushSettings {
        size: 40.0,
        shape_dynamics: ShapeDynamics {
            enabled: true,
            size: Dynamic { control: Control::PenTilt, minimum: 1.0, ..Default::default() },
            tilt_scale: scale,
            ..Default::default()
        },
        ..brush()
    };
    let upright = [pen(50.0, 50.0, 0.0, 0.0, 0.0)];
    let tilted = [pen(50.0, 50.0, 45.0, 0.0, 0.0)];
    // 0 % (the default) and an upright pen leave the tip round.
    let d = dabs_of(&mk(0.0), &tilted)[0];
    assert_eq!(d.roundness, 1.0);
    let d = dabs_of(&mk(2.0), &upright)[0];
    assert_eq!(d.roundness, 1.0);
    // 100 % at 45° tilt: height × (1 − 0.5 × 0.5) = 0.75; 200 %: 0.5.
    let d = dabs_of(&mk(1.0), &tilted)[0];
    assert!((d.roundness - 0.75).abs() < 1e-4, "{}", d.roundness);
    let d2 = dabs_of(&mk(2.0), &tilted)[0];
    assert!((d2.roundness - 0.5).abs() < 1e-4, "{}", d2.roundness);
    let (w, h) = extents(&mk(2.0), &d2);
    assert!(w >= 38 && (h as i32 - 20).abs() <= 2, "{w}×{h}");
    // Only with the size control on Pen Tilt (Photoshop greys the slider otherwise).
    let mut b = mk(2.0);
    b.shape_dynamics.size.control = Control::PenPressure;
    assert_eq!(dabs_of(&b, &tilted)[0].roundness, 1.0);
    // Rendered strokes differ.
    let a = paint(&mk(0.0), &[pen(20.0, 40.0, 60.0, 0.0, 0.0), pen(120.0, 40.0, 60.0, 0.0, 0.0)], 0, 0);
    let c = paint(&mk(2.0), &[pen(20.0, 40.0, 60.0, 0.0, 0.0), pen(120.0, 40.0, 60.0, 0.0, 0.0)], 0, 0);
    let area = Rect::new(0, 0, 160, 80);
    assert!(alpha_sum(&c, area) < alpha_sum(&a, area) * 0.75, "{} vs {}", alpha_sum(&c, area), alpha_sum(&a, area));
}

#[test]
fn brush_projection_foreshortens_along_tilt_and_turns_with_rotation() {
    let mk = |on: bool| BrushSettings { size: 40.0, shape_dynamics: ShapeDynamics { enabled: true, brush_projection: on, ..Default::default() }, ..brush() };
    // Tilted 60° towards +x: the tip is foreshortened to cos 60° = ½ along x.
    let pts = [pen(50.0, 50.0, 60.0, 0.0, 0.0)];
    let d = dabs_of(&mk(true), &pts)[0];
    assert!((d.proj_scale - 0.5).abs() < 1e-4 && d.proj_angle.abs() < 1e-5, "{d:?}");
    let (w, h) = extents(&mk(true), &d);
    assert!((w as i32 - 20).abs() <= 2 && h >= 38, "{w}×{h}");
    // Tilted along y instead: foreshortened vertically.
    let d = dabs_of(&mk(true), &[pen(50.0, 50.0, 0.0, 60.0, 0.0)])[0];
    let (w, h) = extents(&mk(true), &d);
    assert!(w >= 38 && (h as i32 - 20).abs() <= 2, "{w}×{h}");
    // Off: the tilt doesn't touch the tip.
    let d = dabs_of(&mk(false), &pts)[0];
    assert_eq!((d.proj_scale, d.angle), (1.0, 0.0));
    assert_eq!(extents(&mk(false), &d), (40, 40));
    // Barrel rotation turns the tip (an elliptical one shows it).
    let mut b = mk(true);
    b.roundness = 0.5;
    let d = dabs_of(&b, &[pen(50.0, 50.0, 0.0, 0.0, 90.0)])[0];
    assert!((d.angle.to_degrees() - 90.0).abs() < 1e-3);
    let (w, h) = extents(&b, &d);
    assert!(w < h, "{w}×{h}");
    // Dual-brush dabs are never projected.
    let ctx = BrushContext::new(&mk(true));
    let dd = Dab { proj_scale: 0.3, ..Dab::round(Point::new(50.0, 50.0), 20.0, 1.0) };
    let (r, mut a, mut c) = (ctx.dab_rect(&dd, true), Vec::new(), Vec::new());
    ctx.rasterize(&dd, true, r, &mut a);
    ctx.rasterize(&Dab { proj_scale: 1.0, ..dd }, true, r, &mut c);
    assert_eq!(a, c);
}

#[test]
fn spacing_off_spaces_dabs_by_pointer_speed() {
    let mut b = BrushSettings { size: 20.0, spacing_enabled: false, ..brush() };
    let timed = |ms: f64| vec![StrokePoint { time: 0.0, ..StrokePoint::new(0.0, 10.0, 1.0) }, StrokePoint { time: ms, ..StrokePoint::new(100.0, 10.0, 1.0) }];
    // 100 px in 400 ms: a dab every 8 ms = every 2 px; in 40 ms: every 20 px.
    let slow = dabs_of(&b, &timed(400.0)).len();
    let fast = dabs_of(&b, &timed(40.0)).len();
    assert_eq!((slow, fast), (51, 6));
    // Never denser than half a pixel, however slow.
    assert!(dabs_of(&b, &timed(1.0e9)).len() <= 201);
    // No timestamps (a ⇧-click line): spaced by distance like Spacing on, not a dab per point.
    let pts = [StrokePoint::new(0.0, 10.0, 1.0), StrokePoint::new(90.0, 10.0, 1.0), StrokePoint::new(100.0, 10.0, 1.0)];
    let checked = BrushSettings { spacing_enabled: true, ..b.clone() };
    assert_eq!(dabs_of(&b, &pts).len(), dabs_of(&checked, &pts).len());
    assert!(dabs_of(&b, &pts).len() > 3);
    // Checked: fixed spacing (25 % of 20 px = 5 px) whatever the speed.
    b.spacing_enabled = true;
    assert_eq!(dabs_of(&b, &timed(400.0)).len(), dabs_of(&b, &timed(40.0)).len());
    // Chunked input gives the same dabs.
    b.spacing_enabled = false;
    let pts: Vec<StrokePoint> = (0..20).map(|i| StrokePoint { time: f64::from(i) * 13.0, ..StrokePoint::new(f64::from(i) * 7.0, 10.0, 1.0) }).collect();
    let one = dabs_of(&b, &pts);
    let mut g = crate::dynamics::DabGenerator::new(&b, 1.0);
    let (mut out, mut dual) = (Vec::new(), Vec::new());
    for c in pts.chunks(3) {
        g.push(c, &mut out, &mut dual);
    }
    g.finish(&mut out, &mut dual);
    assert_eq!(one, out);
}

#[test]
fn locked_sections_survive_picking_a_preset() {
    let presets = crate::presets::builtin();
    let chalk = crate::presets::find(&presets, "chalk").unwrap().brush.clone();
    let mut cur = BrushSettings {
        scattering: Scattering { enabled: true, count: 5, ..Default::default() },
        transfer: Transfer { enabled: true, wetness: Dynamic::jitter(0.4), ..Default::default() },
        noise: true,
        ..brush()
    };
    // Unlocked: the preset's sections win.
    let next = chalk.clone().picked_over(&cur);
    assert_eq!(next.scattering, chalk.scattering);
    assert_eq!(next.noise, chalk.noise);
    // Locked: the current sections stay, and so do the locks.
    cur.locks = SectionLocks { scattering: true, transfer: true, noise: true, ..Default::default() };
    let next = chalk.clone().picked_over(&cur);
    assert_eq!(next.scattering, cur.scattering);
    assert_eq!(next.transfer, cur.transfer);
    assert!(next.noise);
    assert_eq!(next.locks, cur.locks);
    // Unlocked sections still come from the preset.
    assert_eq!(next.texture, chalk.texture);
    assert_eq!(next.tip, chalk.tip);
}

#[test]
fn transfer_wetness_and_mix_vary_per_dab() {
    let b = BrushSettings {
        transfer: Transfer {
            enabled: true,
            wetness: Dynamic::controlled(Control::PenPressure),
            mix: Dynamic { jitter: 1.0, minimum: 0.25, ..Default::default() },
            ..Default::default()
        },
        ..brush()
    };
    let pts = [StrokePoint::new(0.0, 0.0, 0.3), StrokePoint::new(60.0, 0.0, 0.3)];
    let d = dabs_of(&b, &pts);
    assert!(d.iter().all(|x| (x.wet - 0.3).abs() < 1e-5));
    assert!(d.iter().all(|x| (0.25..=1.0).contains(&x.mix)) && d.iter().any(|x| x.mix < 0.9));
    // Transfer off: as set.
    let d = dabs_of(&brush(), &pts);
    assert!(d.iter().all(|x| x.wet == 1.0 && x.mix == 1.0));
    // New fields round-trip and old JSON loads with neutral values.
    let full = BrushSettings {
        spacing_enabled: false,
        shape_dynamics: ShapeDynamics { tilt_scale: 1.5, brush_projection: true, ..Default::default() },
        locks: SectionLocks { texture: true, ..Default::default() },
        mixer: crate::mixer::MixerSettings { wet: 0.9, sample_all_layers: true, ..Default::default() },
        ..b
    };
    let back: BrushSettings = serde_json::from_value(serde_json::to_value(&full).unwrap()).unwrap();
    assert_eq!(back, full);
    let old: BrushSettings = serde_json::from_str(r#"{"size": 12, "shapeDynamics": {"enabled": true}, "transfer": {"enabled": true}}"#).unwrap();
    assert!(old.spacing_enabled && !old.shape_dynamics.brush_projection && old.shape_dynamics.tilt_scale == 0.0 && old.locks == SectionLocks::default());
    assert_eq!(old.mixer, crate::mixer::MixerSettings::default());
}

/// Primary and Dual Brush dabs of a stroke (the generator, as the renderer drives it).
fn all_dabs(b: &BrushSettings, pts: &[StrokePoint]) -> (Vec<Dab>, Vec<Dab>) {
    let mut g = crate::dynamics::DabGenerator::new(b, 1.0);
    let (mut dabs, mut duals) = (Vec::new(), Vec::new());
    g.push(pts, &mut dabs, &mut duals);
    g.finish(&mut dabs, &mut duals);
    (dabs, duals)
}

/// Control: an ordinary Scatter amount above 100 % still scatters, unchanged by the cap.
#[test]
fn ordinary_scatter_is_unchanged_by_the_cap() {
    let b = BrushSettings {
        size: 20.0,
        scattering: Scattering { enabled: true, scatter: Dynamic::jitter(2.0), both_axes: true, count: 3, ..Default::default() },
        dual_brush: DualBrush { enabled: true, size: 6.0, scatter: 1.5, both_axes: true, ..Default::default() },
        ..brush()
    };
    assert_eq!(b.bounded_for_render(), b);
    let (dabs, duals) = all_dabs(&b, &line(10.0, 200.0, 50.0));
    assert!(dabs.iter().any(|d| (d.center.y - 50.0).abs() > 10.0) && dabs.iter().all(|d| (d.center.y - 50.0).abs() <= 20.0));
    assert!(duals.iter().any(|d| (d.center.y - 50.0).abs() > 1.0) && duals.iter().all(|d| (d.center.y - 50.0).abs() <= 4.5));
}

/// Rows from where the profile drops below 90 % to where it drops below 10 %.
fn soft_edge_width(profile: &[f32]) -> usize {
    let below = |v: f32| profile.iter().position(|a| *a < v).unwrap_or(profile.len());
    below(0.1).saturating_sub(below(0.9))
}

#[test]
fn a_soft_stroke_keeps_most_of_its_tips_softness() {
    // The default brush (10 % spacing), 100 px, hardness 0. Overlapping dabs build up (Flow); a
    // Gaussian-like tip keeps most of its area faint, so the stroke keeps about two thirds of the
    // tip's soft edge (half with the old smoothstep falloff).
    let b = BrushSettings { size: 100.0, hardness: 0.0, pressure_size: false, ..Default::default() };
    let stroke = cross_section(&b, &line(0.0, 400.0, 100.5), 200);
    let dab = cross_section(&b, &[StrokePoint::new(200.5, 100.5, 1.0)], 200);
    let (s, d) = (soft_edge_width(&stroke), soft_edge_width(&dab));
    assert!(d >= 20 && s * 100 >= d * 65, "stroke soft edge {s} px vs tip {d} px");
}

#[test]
fn the_tip_matches_photoshops_measured_dabs() {
    use crate::tip_falloff;
    // Photoshop 27.11, single 40 px dabs (R = 20): mean alpha over rings 0.1 R wide starting at
    // the given fraction of the radius (area-weighted, as measured).
    let measured: [(f32, [(f32, f32); 4]); 3] = [
        (0.0, [(0.2, 0.861), (0.5, 0.509), (0.9, 0.129), (1.2, 0.027)]),
        (0.5, [(0.6, 0.97), (0.8, 0.561), (1.0, 0.135), (1.2, 0.012)]),
        (1.0, [(0.8, 1.0), (0.9, 0.821), (1.2, 0.0), (1.3, 0.0)]),
    ];
    for (h, rings) in measured {
        for (lo, want) in rings {
            let (mut sum, mut area) = (0.0, 0.0);
            for k in 0..64 {
                let r = (lo + 0.1 * (k as f32 + 0.5) / 64.0) * 20.0;
                sum += tip_falloff(r, 20.0, h) * r;
                area += r;
            }
            let got = sum / area;
            assert!((got - want).abs() < 0.05, "hardness {h} ring at {lo} R: {got} vs {want}");
        }
    }
    assert_eq!(tip_falloff(0.0, 10.0, 0.0), 1.0);
    assert_eq!(tip_falloff(4.0, 10.0, 0.5), 1.0, "inside the hard core");
    let v: Vec<f32> = (0..=40).map(|i| tip_falloff(i as f32 * 0.5, 10.0, 0.0)).collect();
    assert!(v.windows(2).all(|w| w[1] <= w[0]), "{v:?}");
    assert_eq!(tip_falloff(crate::tip_reach(10.0, 0.0) * 4.0, 10.0, 0.0), 0.0, "far past the reach");
}

#[test]
fn half_coverage_radius_is_the_tips_50_percent_contour() {
    use crate::{half_coverage_radius, tip_falloff};
    // The Normal Brush Tip cursor ring: where the soft falloff crosses 50 %.
    for h in [0.0, 0.25, 0.5, 0.75, 0.99] {
        let d = half_coverage_radius(10.5, h);
        assert!((tip_falloff(d, 10.5, h) - 0.5).abs() < 1e-3, "hardness {h}: ring at {d}");
    }
    // A hard tip's ring is its edge.
    assert!((half_coverage_radius(10.5, 1.0) - 10.5).abs() < 0.5);
    assert!(half_coverage_radius(10.5, f32::NAN).is_finite());
}

/// Photoshop (measured): a 100 px brush with a 20 px Dual Brush tip resized to 50 / 2 / 1 px has a
/// 10 / 0.4 / 0.2 px dual tip, unrounded and with no floor, and back at 100 px it is 20 px again; past
/// 5000 px the dual tip is capped there and the proportion is lost.
#[test]
fn resizing_scales_the_dual_tip_like_photoshop() {
    let mut b = BrushSettings { size: 100.0, ..brush() };
    b.dual_brush.enabled = true;
    b.dual_brush.size = 20.0;
    for (main, dual) in [(50.0, 10.0), (2.0, 0.4), (1.0, 0.2), (100.0, 20.0), (240.0, 48.0), (100.0, 20.0)] {
        b.resize_with_dual_brush(main);
        assert_eq!(b.size, main);
        assert!((b.dual_brush.size - dual).abs() < 1e-4, "main {main}: dual {} != {dual}", b.dual_brush.size);
    }
    // The cap: 300 px at 100 px would be 15000 px at 5000 px; Photoshop stores 5000 and scales from it.
    b.dual_brush.size = 300.0;
    b.resize_with_dual_brush(MAX_BRUSH_SIZE);
    assert_eq!(b.dual_brush.size, MAX_BRUSH_SIZE);
    b.resize_with_dual_brush(100.0);
    assert_eq!(b.dual_brush.size, 100.0);
    // A sub-pixel dual tip renders at the renderer's smallest diameter.
    b.dual_brush.size = 20.0;
    b.resize_with_dual_brush(1.0);
    assert_eq!(b.bounded_for_render().dual_brush.size, 0.5);
}

#[test]
fn resizing_with_bad_sizes_leaves_the_dual_tip_alone() {
    let mut b = BrushSettings { size: 100.0, ..brush() };
    b.dual_brush.enabled = true;
    b.dual_brush.size = 20.0;
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -5.0] {
        b.size = 100.0;
        b.resize_with_dual_brush(bad);
        assert_eq!(b.dual_brush.size, 20.0, "{bad}");
    }
    for bad_dual in [f32::NAN, 0.0, -3.0] {
        b.size = 100.0;
        b.dual_brush.size = bad_dual;
        b.resize_with_dual_brush(50.0);
        assert_eq!(b.dual_brush.size.to_bits(), bad_dual.to_bits(), "a bad dual size {bad_dual} is left as it was");
    }
    // A disabled dual tip keeps its size.
    b.dual_brush = DualBrush { enabled: false, size: 20.0, ..Default::default() };
    b.size = 100.0;
    b.resize_with_dual_brush(50.0);
    assert_eq!(b.dual_brush.size, 20.0);
}


/// Several inputs drive one setting at once (CSP): each lerps from its
/// minimum, the results multiply; links are set and dropped by input; a
/// brush saved before keeps its single control.
#[test]
fn several_inputs_drive_one_setting() {
    use crate::dynamics::{base_diameter, StepInput};
    let mut b = BrushSettings { size: 100.0, ..BrushSettings::default() };
    b.shape_dynamics.enabled = true;
    let d = &mut b.shape_dynamics.size;
    assert!(d.set_link(Control::PenPressure, Some(Link { minimum: 0.2, ..Link::default() })));
    assert!(d.set_link(Control::PenTilt, Some(Link { minimum: 0.5, ..Link::default() })));
    assert_eq!(d.control, Control::PenPressure);
    assert_eq!(d.links().count(), 2);
    let step = |pressure: f32, tilt: f32| StepInput { point: StrokePoint { pressure, tilt_x: tilt, ..StrokePoint::default() }, direction: 0.0, initial_direction: 0.0, step: 0, velocity: 0.0 };
    // Full pressure, upright: full size; half pressure, flat pen: 0.6 × 0.5.
    assert!((base_diameter(&b, &step(1.0, 0.0)) - 100.0).abs() < 1e-3);
    assert!((base_diameter(&b, &step(0.5, 90.0)) - 30.0).abs() < 1e-3);
    // Pressure dropped: tilt becomes the control.
    let d = &mut b.shape_dynamics.size;
    assert!(d.set_link(Control::PenPressure, None));
    assert_eq!((d.control, d.minimum), (Control::PenTilt, 0.5));
    assert!(d.link(Control::PenPressure).is_none());
    // Saved without `more`: read back as one control; written back the same.
    let old: Dynamic = serde_json::from_str(r#"{"control":"penPressure","minimum":0.25}"#).unwrap();
    assert_eq!(old.links().count(), 1);
    assert!(!serde_json::to_string(&old).unwrap().contains("more"));
}
