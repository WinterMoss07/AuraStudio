//! Mouse cursors: the app picks them and draws its own hand and rotate
//! cursors (Windows has no hand cursor).

use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use winit::{
    event_loop::ActiveEventLoop,
    window::{CursorIcon, CustomCursor, Window},
};

/// The cursor wanted over the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wanted {
    /// None: the brush outline is drawn instead.
    Hidden,
    /// Space held over the canvas: ready to pan.
    Hand,
    /// Panning.
    HandClosed,
    /// Shift + Space held, or rotating.
    Rotate,
    /// Ctrl + Space held, or zooming.
    Zoom,
    /// Color picker (hotspot at the tip).
    Pipette,
    /// Selection tools: a thin cross, with what a press does to the
    /// selection.
    Cross(Mark),
    System(CursorIcon),
}

/// The sign by the selection cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark { None, Add, Subtract, Intersect }

pub struct Cursors {
    hand:        CustomCursor,
    hand_closed: CustomCursor,
    rotate:      CustomCursor,
    zoom:        CustomCursor,
    pipette:     CustomCursor,
    /// By Mark (None, Add, Subtract, Intersect).
    cross:       [CustomCursor; 4],
    /// What the window shows now (None: unknown, set it again).
    applied:     Option<Wanted>,
}

/// Cursor bitmaps are drawn on a grid this many units wide and high, one
/// unit per screen pixel at 100 % display scaling.
const SIZE: u32 = 32;

impl Cursors {
    /// The cursors at `scale` screen pixels per unit (the display scaling:
    /// Windows doesn't scale custom cursors, at 200 % they came out half
    /// the size of the system's).
    pub fn new(event_loop: &ActiveEventLoop, scale: f64) -> Self {
        let scale = (scale as f32).clamp(1.0, 4.0);
        // Hotspot at grid point (hx, hy).
        let make_at = |pixmap: Pixmap, hx: f32, hy: f32| {
            let side = pixmap.width() as u16;
            let k = side as f32 / SIZE as f32;
            let rgba = straight_rgba(&pixmap);
            let source = CustomCursor::from_rgba(rgba, side, side, (hx * k) as u16, (hy * k) as u16).expect("cursor bitmap");
            event_loop.create_custom_cursor(source)
        };
        let make = |pixmap: Pixmap| make_at(pixmap, SIZE as f32 / 2.0, SIZE as f32 / 2.0);
        Self {
            hand: make(hand(false, scale)),
            hand_closed: make(hand(true, scale)),
            rotate: make(rotate(scale)),
            zoom: make(magnifier(scale)),
            pipette: make_at(pipette(scale), PIPETTE_TIP.0, PIPETTE_TIP.1),
            cross: [Mark::None, Mark::Add, Mark::Subtract, Mark::Intersect].map(|m| make(cross(m, scale))),
            applied: None,
        }
    }

    /// Show `wanted` (only touches the window when it changes).
    pub fn apply(&mut self, window: &Window, wanted: Wanted) {
        if self.applied == Some(wanted) { return; }
        self.applied = Some(wanted);
        match wanted {
            Wanted::Hidden => window.set_cursor_visible(false),
            Wanted::Hand => { window.set_cursor(self.hand.clone()); window.set_cursor_visible(true); }
            Wanted::HandClosed => { window.set_cursor(self.hand_closed.clone()); window.set_cursor_visible(true); }
            Wanted::Rotate => { window.set_cursor(self.rotate.clone()); window.set_cursor_visible(true); }
            Wanted::Zoom => { window.set_cursor(self.zoom.clone()); window.set_cursor_visible(true); }
            Wanted::Pipette => { window.set_cursor(self.pipette.clone()); window.set_cursor_visible(true); }
            Wanted::Cross(m) => { window.set_cursor(self.cross[m as usize].clone()); window.set_cursor_visible(true); }
            Wanted::System(icon) => { window.set_cursor(icon); window.set_cursor_visible(true); }
        }
    }

    /// The window's cursor may have been changed behind our back (the
    /// pointer left and came back): set it again next time.
    pub fn invalidate(&mut self) {
        self.applied = None;
    }
}

/// egui's suggestion over its widgets, as a winit cursor. Dragging (egui
/// and egui_dock use grab / move for tabs and handles) gets our hand.
pub fn from_egui(icon: egui::CursorIcon) -> Wanted {
    use egui::CursorIcon as E;
    let system = match icon {
        E::None => return Wanted::Hidden,
        E::Grab => return Wanted::Hand,
        E::Grabbing | E::Move | E::AllScroll => return Wanted::HandClosed,
        E::PointingHand => CursorIcon::Pointer,
        E::Text => CursorIcon::Text,
        E::VerticalText => CursorIcon::VerticalText,
        E::Crosshair => CursorIcon::Crosshair,
        E::NotAllowed | E::NoDrop => CursorIcon::NotAllowed,
        E::Wait => CursorIcon::Wait,
        E::Progress => CursorIcon::Progress,
        E::Help => CursorIcon::Help,
        E::ContextMenu => CursorIcon::ContextMenu,
        E::Cell => CursorIcon::Cell,
        E::Copy => CursorIcon::Copy,
        E::Alias => CursorIcon::Alias,
        E::ZoomIn => CursorIcon::ZoomIn,
        E::ZoomOut => CursorIcon::ZoomOut,
        E::ResizeHorizontal | E::ResizeColumn | E::ResizeEast | E::ResizeWest => CursorIcon::EwResize,
        E::ResizeVertical | E::ResizeRow | E::ResizeNorth | E::ResizeSouth => CursorIcon::NsResize,
        E::ResizeNeSw | E::ResizeNorthEast | E::ResizeSouthWest => CursorIcon::NeswResize,
        E::ResizeNwSe | E::ResizeNorthWest | E::ResizeSouthEast => CursorIcon::NwseResize,
        _ => CursorIcon::Default,
    };
    Wanted::System(system)
}

/// White shape with a dark outline, the way system cursors look.
fn outline_then_fill(pixmap: &mut Pixmap, draw: impl Fn(&mut Pixmap, &Paint, f32)) {
    let mut dark = Paint::default();
    dark.set_color_rgba8(20, 20, 20, 255);
    dark.anti_alias = true;
    let mut light = Paint::default();
    light.set_color_rgba8(255, 255, 255, 255);
    light.anti_alias = true;
    // Outline: the same shapes, 1.6 px wider all around.
    draw(pixmap, &dark, 3.2);
    draw(pixmap, &light, 0.0);
}

/// Open (fingers spread) or closed (fingers curled) hand.
fn hand(closed: bool, scale: f32) -> Pixmap {
    let (mut pixmap, t) = canvas(scale);
    // Finger x positions and fingertip heights.
    let fingers: [(f32, f32); 4] = if closed {
        [(10.5, 12.5), (14.5, 11.5), (18.5, 11.5), (22.5, 13.0)]
    } else {
        [(10.5, 6.0), (14.5, 4.0), (18.5, 5.0), (22.5, 8.0)]
    };
    let thumb = if closed { ((7.5, 17.0), (10.0, 20.5)) } else { ((5.0, 14.0), (10.0, 21.0)) };
    outline_then_fill(&mut pixmap, |pm, paint, extra| {
        let mut stroke = Stroke { width: 3.6 + extra, line_cap: LineCap::Round, ..Default::default() };
        for &(x, top) in &fingers {
            let mut pb = PathBuilder::new();
            pb.move_to(x, top);
            pb.line_to(x, 18.0);
            let path = pb.finish().unwrap();
            pm.stroke_path(&path, paint, &stroke, t, None);
        }
        let mut pb = PathBuilder::new();
        pb.move_to(thumb.0 .0, thumb.0 .1);
        pb.line_to(thumb.1 .0, thumb.1 .1);
        let path = pb.finish().unwrap();
        stroke.width = 3.6 + extra;
        pm.stroke_path(&path, paint, &stroke, t, None);
        // Palm: a rounded block under the fingers.
        let grow = extra / 2.0;
        let rect = tiny_skia::Rect::from_ltrb(8.6 - grow, 14.0 - grow, 24.4 + grow, 27.0 + grow).unwrap();
        let path = rounded_rect(rect, 4.5 + grow);
        pm.fill_path(&path, paint, FillRule::Winding, t, None);
    });
    pixmap
}

/// A circle of two arrows (rotate the view).
fn rotate(scale: f32) -> Pixmap {
    let (mut pixmap, t) = canvas(scale);
    let (cx, cy, r) = (16.0f32, 16.0f32, 9.0f32);
    outline_then_fill(&mut pixmap, |pm, paint, extra| {
        let stroke = Stroke { width: 2.4 + extra, line_cap: LineCap::Butt, ..Default::default() };
        // Two arcs, each ending in an arrowhead, turning the same way.
        for start in [0.25f32, 0.25 + std::f32::consts::PI] {
            let sweep = 2.3f32;
            let mut pb = PathBuilder::new();
            let steps = 24;
            for i in 0..=steps {
                let a = start + sweep * i as f32 / steps as f32;
                let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
                if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
            }
            let path = pb.finish().unwrap();
            pm.stroke_path(&path, paint, &stroke, t, None);
            // Arrowhead at the end, along the turning direction.
            let a = start + sweep;
            let (ex, ey) = (cx + r * a.cos(), cy + r * a.sin());
            let (tx, ty) = (-a.sin(), a.cos());
            let (nx, ny) = (a.cos(), a.sin());
            let (len, half) = (5.0 + extra * 0.8, 4.2 + extra * 0.8);
            let back = extra * 0.5;
            let mut pb = PathBuilder::new();
            pb.move_to(ex + tx * len, ey + ty * len);
            pb.line_to(ex - tx * back + nx * half, ey - ty * back + ny * half);
            pb.line_to(ex - tx * back - nx * half, ey - ty * back - ny * half);
            pb.close();
            let path = pb.finish().unwrap();
            pm.fill_path(&path, paint, FillRule::Winding, t, None);
        }
    });
    pixmap
}

/// A magnifying glass (zoom the view).
fn magnifier(scale: f32) -> Pixmap {
    let (mut pixmap, t) = canvas(scale);
    let (cx, cy, r) = (14.0f32, 14.0f32, 7.5f32);
    outline_then_fill(&mut pixmap, |pm, paint, extra| {
        let ring = Stroke { width: 2.6 + extra, ..Default::default() };
        let circle = PathBuilder::from_circle(cx, cy, r).unwrap();
        pm.stroke_path(&circle, paint, &ring, t, None);
        let handle = Stroke { width: 3.6 + extra, line_cap: LineCap::Round, ..Default::default() };
        let mut pb = PathBuilder::new();
        let k = std::f32::consts::FRAC_1_SQRT_2;
        pb.move_to(cx + (r + 1.5) * k, cy + (r + 1.5) * k);
        pb.line_to(26.0, 26.0);
        let path = pb.finish().unwrap();
        pm.stroke_path(&path, paint, &handle, t, None);
    });
    pixmap
}

/// Where the pipette's tip is, grid units.
const PIPETTE_TIP: (f32, f32) = (3.0, 28.0);

/// An eyedropper, tip at the bottom left (the pixel taken).
fn pipette(scale: f32) -> Pixmap {
    let (mut pixmap, t) = canvas(scale);
    let (tx, ty) = PIPETTE_TIP;
    // Along the tube, toward the top right.
    let k = std::f32::consts::FRAC_1_SQRT_2;
    let at = |d: f32, side: f32| (tx + (d + side) * k, ty - (d - side) * k);
    outline_then_fill(&mut pixmap, |pm, paint, extra| {
        // The glass tube, thin at the tip.
        let mut pb = PathBuilder::new();
        let (a, b, c) = (at(0.0, 0.0), at(17.0, -2.2), at(17.0, 2.2));
        pb.move_to(a.0, a.1);
        pb.line_to(at(3.0, -1.4).0, at(3.0, -1.4).1);
        pb.line_to(b.0, b.1);
        pb.line_to(c.0, c.1);
        pb.line_to(at(3.0, 1.4).0, at(3.0, 1.4).1);
        pb.close();
        let tube = pb.finish().unwrap();
        let stroke = Stroke { width: 0.8 + extra, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
        pm.fill_path(&tube, paint, FillRule::Winding, t, None);
        pm.stroke_path(&tube, paint, &stroke, t, None);
        // The collar across it.
        let collar = Stroke { width: 3.0 + extra, line_cap: LineCap::Round, ..Default::default() };
        let mut pb = PathBuilder::new();
        let (p, q) = (at(17.5, -4.6), at(17.5, 4.6));
        pb.move_to(p.0, p.1);
        pb.line_to(q.0, q.1);
        pm.stroke_path(&pb.finish().unwrap(), paint, &collar, t, None);
        // The bulb.
        let (bx, by) = at(23.5, 0.0);
        let bulb = PathBuilder::from_circle(bx, by, 3.8 + extra / 2.0).unwrap();
        pm.fill_path(&bulb, paint, FillRule::Winding, t, None);
    });
    pixmap
}

/// A thin cross with an open middle (the pixel stays visible), and the
/// selection mode's sign at the bottom right.
fn cross(mark: Mark, scale: f32) -> Pixmap {
    let (mut pixmap, t) = canvas(scale);
    let c = SIZE as f32 / 2.0;
    outline_then_fill(&mut pixmap, |pm, paint, extra| {
        let stroke = Stroke { width: 1.2 + extra * 0.6, line_cap: LineCap::Butt, ..Default::default() };
        let (gap, arm) = (2.5, 8.0);
        let mut pb = PathBuilder::new();
        for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
            pb.move_to(c + dx * gap, c + dy * gap);
            pb.line_to(c + dx * arm, c + dy * arm);
        }
        pm.stroke_path(&pb.finish().unwrap(), paint, &stroke, t, None);
        // The sign.
        let sign = Stroke { width: 1.8 + extra * 0.7, line_cap: LineCap::Square, ..Default::default() };
        let (sx, sy, r) = (c + 9.5, c + 9.5, 3.0);
        let mut pb = PathBuilder::new();
        match mark {
            Mark::None => return,
            Mark::Add => { pb.move_to(sx - r, sy); pb.line_to(sx + r, sy); pb.move_to(sx, sy - r); pb.line_to(sx, sy + r); }
            Mark::Subtract => { pb.move_to(sx - r, sy); pb.line_to(sx + r, sy); }
            Mark::Intersect => { let q = r * 0.8; pb.move_to(sx - q, sy - q); pb.line_to(sx + q, sy + q); pb.move_to(sx + q, sy - q); pb.line_to(sx - q, sy + q); }
        }
        pm.stroke_path(&pb.finish().unwrap(), paint, &sign, t, None);
    });
    pixmap
}

/// A cursor bitmap for `scale`, and the transform from grid units to it.
fn canvas(scale: f32) -> (Pixmap, Transform) {
    let side = (SIZE as f32 * scale).round() as u32;
    (Pixmap::new(side, side).unwrap(), Transform::from_scale(side as f32 / SIZE as f32, side as f32 / SIZE as f32))
}

fn rounded_rect(r: tiny_skia::Rect, radius: f32) -> tiny_skia::Path {
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    let k = radius * 0.5523;
    let mut pb = PathBuilder::new();
    pb.move_to(l + radius, t);
    pb.line_to(rt - radius, t);
    pb.cubic_to(rt - radius + k, t, rt, t + radius - k, rt, t + radius);
    pb.line_to(rt, b - radius);
    pb.cubic_to(rt, b - radius + k, rt - radius + k, b, rt - radius, b);
    pb.line_to(l + radius, b);
    pb.cubic_to(l + radius - k, b, l, b - radius + k, l, b - radius);
    pb.line_to(l, t + radius);
    pb.cubic_to(l, t + radius - k, l + radius - k, t, l + radius, t);
    pb.close();
    pb.finish().unwrap()
}

/// tiny-skia keeps premultiplied pixels; cursors take straight RGBA.
fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_bitmaps_have_ink() {
        for pm in [hand(false, 1.0), hand(true, 1.0), rotate(1.0), magnifier(1.0), hand(false, 2.0), pipette(1.0), cross(Mark::Add, 1.0), cross(Mark::Intersect, 2.0)] {
            let rgba = straight_rgba(&pm);
            assert_eq!(rgba.len(), (pm.width() * pm.height() * 4) as usize);
            let opaque = rgba.chunks(4).filter(|p| p[3] > 128).count();
            assert!(opaque > 30, "cursor almost empty: {opaque}");
            if let Ok(dir) = std::env::var("CURSOR_DUMP") {
                let n = std::fs::read_dir(&dir).map_or(0, |d| d.count());
                std::fs::write(format!("{dir}/c{n}.rgba"), &rgba).unwrap();
            }
        }
    }
}
