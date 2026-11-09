//! Positions and angles from what devices report to the stream's units:
//! window pixels with fractions, tilt in degrees (W3C), rotation 0..360.

/// WinTab is asked for screen coordinates this many times finer than a
/// pixel: the screen's own mapping (the driver's), with fractions.
pub const WINTAB_SCALE: i32 = 16;

/// `v` from the span [from_lo, from_lo + from_size) to [to_lo, to_lo + to_size).
fn remap(v: f64, from_lo: f64, from_size: f64, to_lo: f64, to_size: f64) -> f64 {
    to_lo + (v - from_lo) * to_size / from_size
}

/// A Windows Ink position in the pen device's HIMETRIC space (0.01 mm) to
/// screen pixels, with the device's rectangle and the screen rectangle it
/// maps to (left, top, right, bottom). `None`: an empty rectangle.
pub fn himetric_to_screen(him: (i32, i32), device: [i32; 4], display: [i32; 4]) -> Option<(f64, f64)> {
    let (dw, dh) = ((device[2] - device[0]) as f64, (device[3] - device[1]) as f64);
    let (sw, sh) = ((display[2] - display[0]) as f64, (display[3] - display[1]) as f64);
    if dw <= 0.0 || dh <= 0.0 || sw <= 0.0 || sh <= 0.0 { return None; }
    Some((remap(him.0 as f64, device[0] as f64, dw, display[0] as f64, sw), remap(him.1 as f64, device[1] as f64, dh, display[1] as f64, sh)))
}

/// The fine position if it agrees with the whole-pixel one (within 1.5 px
/// each way; a rotated or oddly mapped tablet would not), else the whole
/// pixel one.
pub fn finer_if_agrees(fine: Option<(f64, f64)>, pixel: (i32, i32)) -> (f64, f64) {
    let p = (pixel.0 as f64, pixel.1 as f64);
    match fine {
        Some(f) if (f.0 - p.0).abs() <= 1.5 && (f.1 - p.1).abs() <= 1.5 => f,
        _ => p,
    }
}

/// A WinTab position (screen pixels × WINTAB_SCALE) to screen pixels.
pub fn wintab_to_screen(x: i32, y: i32) -> (f64, f64) { (x as f64 / WINTAB_SCALE as f64, y as f64 / WINTAB_SCALE as f64) }

/// WinTab orientation (azimuth clockwise from up, altitude above the
/// tablet, both in tenths of a degree; altitude negative for the eraser)
/// to tilt toward +x / +y in degrees (W3C: the angle between the pen's
/// projection on the x-z plane and the z axis, same for y).
pub fn wintab_tilt(azimuth: i32, altitude: i32) -> (f32, f32) {
    let az = (azimuth as f64 / 10.0).to_radians();
    let alt = ((altitude.unsigned_abs() as f64) / 10.0).clamp(0.0, 90.0).to_radians();
    if alt >= std::f64::consts::FRAC_PI_2 - 1e-9 { return (0.0, 0.0); }
    // The pen's direction from the tip: horizontal part toward the
    // azimuth (y down on screen), vertical part sin(alt).
    let (h, v) = (alt.cos(), alt.sin());
    let (dx, dy) = (h * az.sin(), -h * az.cos());
    let tx = dx.atan2(v).to_degrees().clamp(-90.0, 90.0) as f32;
    let ty = dy.atan2(v).to_degrees().clamp(-90.0, 90.0) as f32;
    (tx, ty)
}

/// WinTab twist (tenths of a degree) to rotation 0..360.
#[cfg(test)]
pub fn wintab_rotation(twist: i32) -> f32 { ((twist as f32 / 10.0) % 360.0 + 360.0) % 360.0 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn himetric_positions_keep_their_fraction() {
        let dev = [0, 0, 30_000, 20_000];
        let disp = [-3000, 0, 0, 2000];
        assert_eq!(himetric_to_screen((0, 0), dev, disp), Some((-3000.0, 0.0)));
        let (x, y) = himetric_to_screen((15_005, 10_003), dev, disp).unwrap_or_default();
        assert!((x - -1499.5).abs() < 1e-9 && (y - 1000.3).abs() < 1e-9, "{x} {y}");
        assert_eq!(himetric_to_screen((1, 1), [0, 0, 0, 10], disp), None);
    }

    #[test]
    fn a_fine_position_far_from_the_pixel_one_is_not_trusted() {
        assert_eq!(finer_if_agrees(Some((10.4, 20.7)), (10, 21)), (10.4, 20.7));
        assert_eq!(finer_if_agrees(Some((14.0, 20.0)), (10, 20)), (10.0, 20.0));
        assert_eq!(finer_if_agrees(None, (3, 4)), (3.0, 4.0));
    }

    #[test]
    fn wintab_positions_are_sixteenths_of_a_pixel() {
        assert_eq!(wintab_to_screen(16 * 100 + 8, -16 * 5 - 4), (100.5, -5.25));
    }

    #[test]
    fn wintab_orientation_to_degrees() {
        // Upright: no tilt.
        assert_eq!(wintab_tilt(0, 900), (0.0, 0.0));
        // Leaning 30° to the right (azimuth 90°, altitude 60°).
        let (tx, ty) = wintab_tilt(900, 600);
        assert!((tx - 30.0).abs() < 0.01 && ty.abs() < 0.01, "{tx} {ty}");
        // Leaning toward the user (down on screen: azimuth 180°).
        let (tx, ty) = wintab_tilt(1800, 450);
        assert!(tx.abs() < 0.01 && (ty - 45.0).abs() < 0.01, "{tx} {ty}");
        // The eraser end reports a negative altitude: same tilt.
        assert_eq!(wintab_tilt(900, -600), wintab_tilt(900, 600));
        assert_eq!(wintab_rotation(-900), 270.0);
    }
}
