//! Mouse cursors: the app decides which one shows (egui only suggests one
//! over its widgets), and draws its own hand and rotate cursors (the
//! system has no open / closed hand on Windows: winit falls back to the
//! four-arrow "move" cross there).

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
    System(CursorIcon),
}

pub struct Cursors {
    hand:        CustomCursor,
    hand_closed: CustomCursor,
    rotate:      CustomCursor,
    /// What the window shows now (None: unknown, set it again).
    applied:     Option<Wanted>,
}

/// Cursor bitmaps are this many pixels wide and high.
const SIZE: u32 = 32;

impl Cursors {
    pub fn new(event_loop: &ActiveEventLoop) -> Self {
        let make = |pixmap: Pixmap| {
            let rgba = straight_rgba(&pixmap);
            let source = CustomCursor::from_rgba(rgba, SIZE as u16, SIZE as u16, (SIZE / 2) as u16, (SIZE / 2) as u16)
                .expect("cursor bitmap");
            event_loop.create_custom_cursor(source)
        };
        Self {
            hand: make(hand(false)),
            hand_closed: make(hand(true)),
            rotate: make(rotate()),
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
fn hand(closed: bool) -> Pixmap {
    let mut pixmap = Pixmap::new(SIZE, SIZE).unwrap();
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
            pm.stroke_path(&path, paint, &stroke, Transform::identity(), None);
        }
        let mut pb = PathBuilder::new();
        pb.move_to(thumb.0 .0, thumb.0 .1);
        pb.line_to(thumb.1 .0, thumb.1 .1);
        let path = pb.finish().unwrap();
        stroke.width = 3.6 + extra;
        pm.stroke_path(&path, paint, &stroke, Transform::identity(), None);
        // Palm: a rounded block under the fingers.
        let grow = extra / 2.0;
        let rect = tiny_skia::Rect::from_ltrb(8.6 - grow, 14.0 - grow, 24.4 + grow, 27.0 + grow).unwrap();
        let path = rounded_rect(rect, 4.5 + grow);
        pm.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
    });
    pixmap
}

/// A circle of two arrows (rotate the view).
fn rotate() -> Pixmap {
    let mut pixmap = Pixmap::new(SIZE, SIZE).unwrap();
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
            pm.stroke_path(&path, paint, &stroke, Transform::identity(), None);
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
            pm.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
        }
    });
    pixmap
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
        for pm in [hand(false), hand(true), rotate()] {
            let rgba = straight_rgba(&pm);
            assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);
            let opaque = rgba.chunks(4).filter(|p| p[3] > 128).count();
            assert!(opaque > 60, "cursor almost empty: {opaque}");
            if let Ok(dir) = std::env::var("CURSOR_DUMP") {
                let n = std::fs::read_dir(&dir).map_or(0, |d| d.count());
                std::fs::write(format!("{dir}/c{n}.rgba"), &rgba).unwrap();
            }
        }
    }
}
