//! Everything that points, as aurastudio-input hands it over: strokes, view
//! drags, the Alt + right drag on the brush, the tools' presses; the
//! cursor and the brush outline that follow.

use std::sync::Arc;

use aurastudio_core::Point;
use aurastudio_forge::{Brush, PenSample};
use aurastudio_input::{Device, Event, Kind};
use aurastudio_layers::FillParams;
use aurastudio_ui::Tool;
use winit::window::{CursorIcon, ResizeDirection};

use super::{App, Gesture, Nav, NavButton, ZOOM_DRAG};
use crate::paint_thread::Cmd;
use crate::window::Screen;

/// Window edge that resizes the window (it has no system frame), physical
/// pixels.
const EDGE: f32 = 5.0;

impl App {
    /// Window pixels -> canvas pixels, through the view (zoom, rotation,
    /// pan): the exact inverse of what the GPU draws.
    pub(super) fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        Some(self.gpu.as_ref()?.screen_to_canvas((wx, wy)))
    }

    /// The canvas point under the cursor.
    pub(super) fn canvas_at(&self) -> Option<Point> {
        let (x, y) = self.window_to_canvas(self.cursor.0, self.cursor.1)?;
        Some(Point::new(x as f64, y as f64))
    }

    /// Everything the pointing devices did since the last call.
    #[profiling::function]
    pub(super) fn poll_input(&mut self) {
        // Where the canvas is and whether the interface holds the pointer:
        // the strokelog keeps only the canvas's strokes.
        if let Some(gpu) = &self.gpu {
            let [x, y, w, h] = gpu.viewport();
            let (x, y, w, h) = (x as f64, y as f64, w as f64, h as f64);
            self.input.set_frame(aurastudio_input::FrameInfo { canvas: (x, y, x + w, y + h), ui_wants_pointer: gpu.ui_is_dragging() });
        }
        for e in self.input.poll() { self.on_pointer(e); }
    }

    fn on_pointer(&mut self, e: Event) {
        // (The pen's samples come here, not as window events.)
        self.wakes.touched = true;
        let pos = (e.x as f32, e.y as f32);
        // Pen ⇄ mouse: the system shows its own cursor again, ours must be
        // set anew (else the arrow stays inside the brush outline). Only on
        // a real switch: the pen arriving, or the mouse used once the pen
        // is away (Windows sends mouse events for the pen too: reset on
        // each, the cursor flickered while painting).
        match e.kind {
            Kind::ProximityIn => self.pen_near = true,
            Kind::ProximityOut => self.pen_near = false,
            _ => {}
        }
        let for_mouse = match e.device {
            Device::Mouse if !self.pen_near => true,
            Device::Pen | Device::Eraser => false,
            _ => self.cursor_for_mouse,
        };
        if for_mouse != self.cursor_for_mouse || matches!(e.kind, Kind::ProximityIn) {
            self.cursor_for_mouse = for_mouse;
            if let Some(c) = &mut self.cursors { c.invalidate(); }
            self.cursor = pos;
            self.update_cursor();
        }
        match e.kind {
            Kind::Down => {
                self.cursor = pos;
                self.over_canvas = self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos));
                self.press(e);
            }
            Kind::Move | Kind::Hover => {
                self.cursor = pos;
                if !self.is_drawing && self.nav.is_none() {
                    self.over_canvas = self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos));
                    // Hovering, the cursor follows the pen's tilt and turn.
                    if matches!(e.device, Device::Pen | Device::Eraser) { self.pen = (e.tilt, e.rotation); }
                }
                self.follow_direction(pos);
                self.cursor_in_window = true;
                self.moved(e);
            }
            Kind::Up { .. } => {
                self.cursor = pos;
                self.release(e);
            }
            Kind::ButtonDown(b) => self.button(b, true),
            Kind::ButtonUp(b) => self.button(b, false),
            Kind::Wheel { dy, .. } => {
                if self.is_drawing || !self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos)) { return; }
                if let Some(gpu) = &mut self.gpu {
                    gpu.view_zoom_at(1.2f32.powf(dy), pos);
                    self.request_redraw();
                }
            }
            Kind::ProximityOut => { self.update_cursor(); self.request_canvas_frame(); }
            _ => {}
        }
    }

    /// The tip touched (the left button).
    fn press(&mut self, e: Event) {
        if self.nav.is_some() || self.is_drawing || self.size_drag.is_some() {
            // Already dragging the view (middle button) or drawing: a
            // second press starts nothing.
            return;
        }
        if e.device == Device::Mouse {
            if let Some(dir) = self.resize_edge(self.cursor) {
                if let Some(w) = &self.window { if let Err(err) = w.drag_resize_window(dir) { log::warn!("window resize: {err}"); } }
                // The system's resize loop took the release.
                self.input.mouse_button(aurastudio_input::Button::Left, false);
                return;
            }
        }
        // A question is waiting: the canvas doesn't paint.
        if self.ui.has_dialog() || !self.over_canvas { return; }
        if self.space_down {
            // Space + drag: pan; Shift + Space + drag: rotate; Ctrl +
            // Space + drag: zoom (Krita / CSP).
            self.begin_nav(NavButton::Left);
        } else if !self.home && self.gesture.is_none() {
            self.pressure = e.pressure;
            self.pen = (e.tilt, e.rotation);
            self.stroke_device = Some(e.device);
            self.tool_press(e);
        }
    }

    fn moved(&mut self, e: Event) {
        // (The window edge under the cursor, asked once per move.)
        let edge = self.resize_edge(self.cursor);
        let edge_changed = edge.is_some() != self.edge_hover;
        self.edge_hover = edge.is_some();
        // The outline moves with the cursor: a frame is needed.
        if self.over_canvas || self.outline_shown {
            self.update_outline();
            self.apply_cursor_at(edge);
            self.request_canvas_frame();
        } else if edge_changed {
            self.apply_cursor_at(edge);
        }
        // The loupe is UI: a UI frame (also when it just left).
        if self.loupe_on().is_some() || self.loupe_shown { self.request_redraw(); }
        if let Some(d) = &mut self.size_drag {
            let scale = self.gpu.as_ref().map_or(1.0, Screen::winit_scale).max(0.1);
            d.moved((self.cursor.0 / scale, self.cursor.1 / scale));
            self.brush.resize_with_dual_brush(d.size().max(1.0));
            self.brush.hardness = d.hardness();
            self.update_outline();
            // The slider and the readout follow.
            self.request_redraw();
        } else if self.nav.is_some() {
            self.drag_nav();
        } else if self.xf.as_ref().is_some_and(|x| x.drag.is_some()) {
            self.xform_drag();
        } else if self.gesture.is_some() {
            self.tool_drag();
        } else if self.poly.is_some() {
            self.request_redraw();
        } else if self.arc.active() {
            if let Some(at) = self.canvas_at() { self.arc_move(at); }
        } else if self.is_drawing && self.stroke_device == Some(e.device) && e.kind == Kind::Move {
            self.pressure = e.pressure;
            self.pen = (e.tilt, e.rotation);
            self.draw_at(self.cursor, e.time.0 as f64 / 1e6);
            self.request_canvas_frame();
        }
    }

    /// The tip lifted: a release always ends what the press began, even
    /// over a docker.
    fn release(&mut self, e: Event) {
        if self.nav.is_some() {
            if self.nav_button == NavButton::Left { self.end_nav(); }
        } else if self.is_drawing {
            if self.stroke_device == Some(e.device) { self.end_stroke(e.time.0 as f64 / 1e6); }
        } else if self.xf.as_ref().is_some_and(|x| x.drag.is_some()) {
            self.xform_release();
        } else if self.gesture.is_some() {
            self.tool_release();
        } else if self.arc.wants_release() {
            if let Some(at) = self.canvas_at() { self.arc_release(at); }
        }
    }

    /// Mouse buttons other than the left one: 0 right, 1 middle.
    fn button(&mut self, b: u8, down: bool) {
        match (b, down) {
            // Alt + right drag over the canvas (Photoshop): horizontal sets
            // the brush size, vertical its hardness (size_drag.rs).
            (0, true) => {
                let can = self.over_canvas && self.modifiers.alt_key() && !self.home && self.tool.paints() && !self.is_drawing
                    && self.nav.is_none() && self.gesture.is_none() && self.xf.is_none() && !self.ui.has_dialog() && self.size_drag.is_none();
                if !can { return; }
                let scale = self.gpu.as_ref().map_or(1.0, Screen::winit_scale).max(0.1);
                self.size_drag = Some(crate::size_drag::SizeDrag::new(
                    self.cursor,
                    (self.cursor.0 / scale, self.cursor.1 / scale),
                    (self.brush.size).round(),
                    self.brush.hardness,
                    aurastudio_ui::MAX_BRUSH_SIZE as f32,
                    true,
                ));
                self.update_cursor();
                self.request_redraw();
            }
            (0, false) if self.size_drag.take().is_some() => {
                self.update_cursor();
                self.request_redraw();
            }
            // Middle drag: pan, Shift: rotate, Ctrl: zoom.
            (1, true) if self.over_canvas && !self.is_drawing && self.nav.is_none() => self.begin_nav(NavButton::Middle),
            (1, false) if self.nav.is_some() && self.nav_button == NavButton::Middle => self.end_nav(),
            _ => {}
        }
    }

    /// The window edge under `p` when the window can be resized there (not
    /// maximized; the window has no system frame).
    pub(super) fn resize_edge(&self, p: (f32, f32)) -> Option<ResizeDirection> {
        let w = self.window.as_ref()?;
        if w.is_maximized() || self.is_drawing || self.nav.is_some() { return None; }
        let s = w.inner_size();
        let (w, h) = (s.width as f32, s.height as f32);
        let (l, r, t, b) = (p.0 < EDGE, p.0 >= w - EDGE, p.1 < EDGE, p.1 >= h - EDGE);
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }

    /// Which view drag the held keys ask for: Ctrl zooms, Shift rotates,
    /// nothing pans (Krita / CSP).
    pub(super) fn nav_mode(&self) -> Nav {
        if self.ctrl_held() { Nav::Zoom } else if self.shift_held() { Nav::Rotate } else { Nav::Pan }
    }

    /// Start panning / rotating / zooming the view with a drag from the
    /// cursor.
    fn begin_nav(&mut self, button: NavButton) {
        let mode = self.nav_mode();
        log::info!("view drag: {mode:?}");
        self.nav = Some((mode, self.cursor));
        self.nav_button = button;
        self.set_nav_pivot(mode);
        self.update_cursor();
    }

    /// Fixed point of a zoom drag (where it began, CSP) or rotation drag
    /// (the picture's center if all in view, else the area's).
    fn set_nav_pivot(&mut self, mode: Nav) {
        let Some(gpu) = &self.gpu else { return };
        self.nav_pivot = match mode {
            Nav::Zoom => self.cursor,
            _ => gpu.rotation_pivot(),
        };
    }

    fn end_nav(&mut self) {
        self.nav = None;
        self.update_cursor();
    }

    /// Shift is down, from the modifier state or the keys themselves (with
    /// the pen, the modifier state alone missed Shift).
    pub(super) fn shift_held(&self) -> bool { self.modifiers.shift_key() || self.shift_down }

    pub(super) fn ctrl_held(&self) -> bool { self.modifiers.control_key() || self.ctrl_down }

    /// Cursor moved during a navigation drag.
    fn drag_nav(&mut self) {
        let Some((prev_mode, last)) = self.nav else { return };
        // The mode follows Ctrl / Shift during the drag (Krita): pressing or
        // releasing one mid-drag switches.
        let mode = self.nav_mode();
        if mode != prev_mode {
            self.set_nav_pivot(mode);
            self.update_cursor();
        }
        let pivot = self.nav_pivot;
        let now = self.cursor;
        let Some(gpu) = &mut self.gpu else { return };
        match mode {
            Nav::Pan => gpu.view.pan_by(now.0 - last.0, now.1 - last.1),
            Nav::Rotate => {
                // Turn by the angle the cursor swept around the pivot.
                let a0 = (last.1 - pivot.1).atan2(last.0 - pivot.0);
                let a1 = (now.1 - pivot.1).atan2(now.0 - pivot.0);
                gpu.view_rotate_at(aurastudio_gpu::wrap_angle(a1 - a0), pivot);
            }
            // Horizontal drag (CSP): right zooms in, left zooms out; 100 px
            // is about x2.
            Nav::Zoom => gpu.view_zoom_at((ZOOM_DRAG * (now.0 - last.0)).exp(), pivot),
        }
        self.nav = Some((mode, now));
        self.request_redraw();
        self.render_if_due();
    }

    /// Brush outline under the cursor over the canvas, in place of the OS
    /// cursor (back over the docks and while navigating). None on the start
    /// screen (its 1 × 1 stand-in canvas shows at the highest zoom).
    pub(super) fn update_outline(&mut self) {
        // Alt + right drag: the brush stays where the drag started.
        if let Some(d) = &self.size_drag {
            self.outline_shown = true;
            let c = self.brush_cursor_at(d.anchor);
            if let Some(gpu) = &mut self.gpu { gpu.set_brush_cursor(c); }
            return;
        }
        let show = !self.home && self.over_canvas && self.cursor_in_window && !self.space_down && self.nav.is_none()
            && self.tool.paints() && !self.picking_with_alt() && self.gesture.is_none() && self.prefs.brush_outline;
        self.outline_shown = show;
        let c = if show { self.brush_cursor_at(self.cursor) } else { None };
        if let Some(gpu) = &mut self.gpu { gpu.set_brush_cursor(c); }
    }

    /// The brush cursor at window position `at`: the outline of the dab the
    /// brush would lay there (hovering: at full pressure; drawing: the
    /// pen's), turned and squashed as the dab would be on screen.
    fn brush_cursor_at(&mut self, at: (f32, f32)) -> Option<aurastudio_gpu::BrushCursor> {
        /// Longer side of a sampled tip's picture for its outline, pixels.
        const MASK: u32 = 128;
        let hover = !self.is_drawing;
        let ((tilt_x, tilt_y), rotation) = self.pen;
        let pen = aurastudio_forge::StrokePoint { x: 0.0, y: 0.0, pressure: self.pressure, tilt_x, tilt_y, rotation, wheel: 0.0, time: 0.0 };
        let d = self.brush.cursor_dab(pen, self.cursor_dir, hover);
        let gpu = self.gpu.as_mut()?;
        if d.sampled && self.brush.cursor_mask_id(MASK) != gpu.cursor_mask_id() {
            if let Some(m) = self.brush.cursor_mask(MASK) { gpu.set_cursor_mask(m.id, m.width, m.height, &m.alpha); }
        }
        // Window offset -> canvas offset -> the tip's coordinates.
        let w = gpu.window_to_canvas_linear();
        let t = d.to_tip;
        let to_tip = [t[0] * w[0] + t[1] * w[2], t[0] * w[1] + t[1] * w[3], t[2] * w[0] + t[3] * w[2], t[2] * w[1] + t[3] * w[3]];
        let zoom = gpu.zoom();
        let shape = if d.sampled { aurastudio_gpu::CursorShape::Mask } else { aurastudio_gpu::CursorShape::Round };
        Some(aurastudio_gpu::BrushCursor { center: at, to_tip, extent: d.reach * zoom, radius: d.radius * zoom, shape })
    }

    /// The way the pen goes (for a tip turned by the direction): from
    /// where it last was a few canvas pixels back.
    fn follow_direction(&mut self, pos: (f32, f32)) {
        let Some(gpu) = &self.gpu else { return };
        let p = gpu.screen_to_canvas(pos);
        let Some(from) = self.dir_from else { self.dir_from = Some(p); return };
        let (dx, dy) = (p.0 - from.0, p.1 - from.1);
        if dx.hypot(dy) < 3.0 { return; }
        // (Counter-clockwise, y up, as the brush engine reads it.)
        self.cursor_dir = (-dy).atan2(dx);
        self.dir_from = Some(p);
    }

    /// Pointer, keys or view drag changed: outline and cursor follow.
    pub(super) fn update_cursor(&mut self) {
        self.update_outline();
        self.apply_cursor();
    }

    /// The one place the window cursor is set (egui only suggests one).
    pub(super) fn apply_cursor(&mut self) { self.apply_cursor_at(self.resize_edge(self.cursor)); }

    /// The same, the window edge under the cursor known (`edge`).
    fn apply_cursor_at(&mut self, edge: Option<ResizeDirection>) {
        use crate::cursors::Wanted;
        let edge = edge.map(|d| Wanted::System(match d {
            ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
            ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
            ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
        }));
        let edge = edge.filter(|_| self.gesture.is_none() && self.size_drag.is_none());
        let wanted = match (self.nav, edge) {
            (Some((Nav::Pan, _)), _) => Wanted::HandClosed,
            (Some((Nav::Rotate, _)), _) => Wanted::Rotate,
            (Some((Nav::Zoom, _)), _) => Wanted::Zoom,
            (None, Some(e)) => e,
            (None, _) if self.space_down && self.over_canvas => match self.nav_mode() {
                Nav::Zoom => Wanted::Zoom,
                Nav::Rotate => Wanted::Rotate,
                Nav::Pan => Wanted::Hand,
            },
            (None, _) if self.is_drawing || self.size_drag.is_some() => Wanted::Hidden,
            (None, _) if self.over_canvas && !self.home => match self.tool {
                Tool::Transform => self.transform_cursor().unwrap_or(Wanted::System(CursorIcon::Default)),
                _ if matches!(self.gesture, Some(Gesture::MoveSel { .. })) => Wanted::System(CursorIcon::Move),
                // The loupe draws its own cross.
                _ if self.loupe_on().is_some() => Wanted::Hidden,
                _ if self.picking_with_alt() || matches!(self.gesture, Some(Gesture::Pick)) => Wanted::Pipette,
                t if t.selects() && (self.gesture.is_some() || self.poly.is_some()) => Wanted::Cross(self.select_mark()),
                _ if self.gesture.is_some() => Wanted::System(CursorIcon::Crosshair),
                Tool::RectSelect | Tool::EllipseSelect | Tool::Lasso
                    if self.canvas_at().is_some_and(|p| self.on_selection(p)) => Wanted::System(CursorIcon::Move),
                Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc if self.prefs.brush_outline => Wanted::Hidden,
                Tool::Brush | Tool::Eraser | Tool::Line | Tool::Rectangle | Tool::Ellipse | Tool::Arc | Tool::Fill => Wanted::System(CursorIcon::Crosshair),
                Tool::Picker => Wanted::Pipette,
                Tool::Lasso | Tool::RectSelect | Tool::EllipseSelect | Tool::PolySelect | Tool::MagicWand => Wanted::Cross(self.select_mark()),
            },
            (None, _) if self.over_canvas => Wanted::System(CursorIcon::Default),
            (None, _) => self.gpu.as_ref().map_or(Wanted::System(CursorIcon::Default), |g| crate::cursors::from_egui(g.egui_cursor())),
        };
        let (Some(window), Some(cursors)) = (&self.window, &mut self.cursors) else { return };
        cursors.apply(window, wanted);
    }

    /// Follow the scaling of the window's monitor, asked from Windows on
    /// every move / resize / focus (winit missed a 200 % -> 100 % move).
    pub(super) fn refresh_dpi(&mut self, why: &str) {
        // AURASTUDIO_DPI=96...: pretend Windows reports this DPI (testing;
        // read once, not at every move).
        static FORCED: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
        let forced = *FORCED.get_or_init(|| std::env::var("AURASTUDIO_DPI").ok().and_then(|v| v.parse::<u32>().ok()));
        let real = forced.or_else(|| self.hwnd().and_then(crate::system::window_dpi)).map(|d| d as f32 / 96.0);
        let Some(gpu) = &mut self.gpu else { return };
        if let Some(scale) = gpu.set_dpi_scale(real) {
            log::info!("display scaling: {:.0} % ({why}; winit: {:.0} %)", scale * 100.0, gpu.winit_scale() * 100.0);
            self.update_cursor();
            self.request_redraw();
        }
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    /// `time`: the press's, seconds on the input clock.
    pub(super) fn begin_stroke(&mut self, erase: bool, time: f64) {
        // (Drawing stops the playback, on the frame shown.)
        self.stop_playback();
        // Instant feedback when the layer can't be painted.
        if let Some(msg) = self.paint_refusal() {
            self.ui.notify(msg);
            self.is_drawing = false;
            self.request_redraw();
            return;
        }
        self.is_drawing = true;
        self.last_sent = None;
        self.send_begin(erase);
        self.draw_at(self.cursor, time);
    }

    /// Open a stroke on the paint thread with the current brush.
    pub(super) fn send_begin(&mut self, erase: bool) {
        // Each stroke its own jitter (a brush with a seed of its own keeps it).
        self.strokes += 1;
        crate::latency::begin_stroke();
        let seed = aurastudio_forge::stroke_seed(self.brush.seed, self.strokes);
        self.paint.send(Cmd::Begin {
            doc:   Arc::clone(&self.doc),
            brush: Box::new(Brush { erase: erase || self.brush.erase, seed, ..self.brush.clone() }),
            limit: self.stroke_limit(),
            zoom:  self.zoom(),
        });
    }

    /// "Stay inside the lines": the fill tool's tolerance and drafts
    /// setting, its expand (at least 1 px: under the lines' soft edge).
    fn stroke_limit(&self) -> Option<FillParams> {
        self.stay_inside.then_some(FillParams { expand: self.fill.expand.max(1), ..self.fill })
    }

    pub(super) fn end_stroke(&mut self, time: f64) {
        // The tip lifted where no sample went yet: the stroke ends there
        // (at the pressure last given; a press alone: one dot).
        if self.last_sent != Some(self.cursor) { self.draw_at(self.cursor, time); }
        self.is_drawing = false;
        self.stroke_device = None;
        self.paint.send(Cmd::End);
        crate::latency::end_stroke();
        self.update_cursor();
    }

    /// Forward one sample to the paint thread (never paints here). `time`:
    /// seconds on the input clock.
    fn draw_at(&mut self, at: (f32, f32), time: f64) {
        let Some((x, y)) = self.window_to_canvas(at.0, at.1) else { return };
        self.last_sent = Some(at);
        crate::latency::sent(time);
        self.paint.send(Cmd::Sample(PenSample::new(x, y, self.pressure, time).with_pen(self.pen.0, self.pen.1)));
    }

    /// The window lost focus (Alt+Tab...): key releases now go elsewhere,
    /// so drop held Space, any view drag and any stroke, instead of
    /// leaving them stuck.
    pub(super) fn reset_input(&mut self) {
        self.input.activated(false);
        self.poll_input();
        self.size_drag = None;
        self.space_down = false;
        self.shift_down = false;
        self.ctrl_down = false;
        self.nav = None;
        self.gesture = None;
        self.xform_release();
        if self.arc.active() { self.finish_arcs(); }
        self.arc.reset_release();
        if self.is_drawing { self.end_stroke(aurastudio_input::clock::now().0 as f64 / 1e6); }
        self.update_cursor();
        self.request_redraw();
    }
}

/// Canvas point → what the interface draws with.
pub(super) fn pt(p: Point) -> (f32, f32) { (p.x as f32, p.y as f32) }
