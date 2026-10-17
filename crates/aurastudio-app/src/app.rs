use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    keyboard::{Key, ModifiersState, NamedKey},
    window::{CursorIcon, Window, WindowId},
};

use aurastudio_brush::Brush;
use aurastudio_canvas::Rect;
use aurastudio_gpu::GpuState;
use aurastudio_input::Stylus;
use aurastudio_document::{SharedDoc, Workspace};
use aurastudio_ui::{Actions, Model, UiState, ViewCmd};

use crate::paint_thread::{self, Cmd, PaintThread, Timing};

/// Canvas size at startup; changed afterwards from the Canvas docker.
const DEFAULT_CANVAS: (u32, u32) = (2048, 2048);

/// View navigation drag in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Nav { Pan, Rotate }

/// Message posted to the event loop (by the app itself or the paint thread).
#[derive(Debug, Clone, Copy)]
pub enum AppEvent {
    /// Wake the loop so about_to_wait runs and requests the wanted frame.
    Wake,
    /// The paint thread changed some canvas area.
    Painted,
    /// A layer's pixels changed for good (stroke done, undo, clear).
    LayerChanged(u64),
    /// Message for the options bar, from the paint thread.
    Notice(&'static str),
}

pub struct App {
    proxy:  EventLoopProxy<AppEvent>,
    window: Option<Arc<Window>>,
    gpu:    Option<GpuState>,
    stylus: Option<Stylus>,

    brush:        Brush,
    /// true = hokusai engine (libmypaint, for textured brushes later),
    /// false = swept round pen (default).
    use_hokusai:  bool,
    /// Pressure smoothing distance, fraction of the brush radius.
    smoothing:    f32,

    /// Every open document (one for now; the crate handles several).
    #[allow(dead_code)]
    workspace: Workspace,
    /// The document shown, shared with the paint thread.
    doc:    SharedDoc,
    paint:  PaintThread,
    /// Width / height typed in the Canvas dock, applied by "Resize".
    canvas_edit: (u32, u32),
    ui:          UiState,

    is_drawing: bool,
    pressure:   f32,
    modifiers:  ModifiersState,
    /// Space held: a left drag moves the view instead of painting.
    space_down: bool,
    /// View drag in progress, with the last cursor position.
    nav:        Option<(Nav, (f32, f32))>,
    /// Set once the native pen sent samples during the current stroke:
    /// the CursorMoved events Windows synthesizes for the same pen are then
    /// ignored, otherwise the stroke gets two slightly offset position
    /// streams (wavy / zigzag edges).
    pen_active: bool,
    /// Last known cursor position, physical window pixels.
    cursor:     (f32, f32),

    /// Canvas area painted since the last frame, recomposited at render.
    pending:      Option<Rect>,
    /// A change outside strokes (visibility, opacity, add/remove/clear
    /// layer, resize) needs a full-canvas recomposite.
    layers_dirty: bool,
    last_frame:   Option<Instant>,
    /// A frame is needed. The actual window redraw request is made in
    /// about_to_wait: winit drops requests made while handling a redraw.
    redraw_wanted: bool,
    /// Delayed frame asked by egui (tooltip, notice fade-out).
    repaint_at:   Option<Instant>,
    /// Main-thread frame cost during the current stroke (logged at its end).
    frame_stats:  Timing,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let mut workspace = Workspace::new();
        let doc = workspace.new_document("Untitled", DEFAULT_CANVAS.0, DEFAULT_CANVAS.1);
        let worker_proxy = proxy.clone();
        let paint = PaintThread::spawn(move |e| {
            let _ = worker_proxy.send_event(match e {
                paint_thread::Event::Painted => AppEvent::Painted,
                paint_thread::Event::LayerChanged { layer_id } => AppEvent::LayerChanged(layer_id),
                paint_thread::Event::Notice(text) => AppEvent::Notice(text),
            });
        });
        Self {
            proxy,
            window: None,
            gpu:    None,
            stylus: None,

            brush:       Brush::new(),
            use_hokusai: false,
            smoothing:   0.5,

            workspace,
            doc,
            paint,
            canvas_edit: DEFAULT_CANVAS,
            ui:          UiState::new(),

            is_drawing: false,
            pressure:   1.0,
            modifiers:  ModifiersState::empty(),
            space_down: false,
            nav:        None,
            pen_active: false,
            cursor:     (0.0, 0.0),

            pending:      None,
            layers_dirty: false,
            last_frame:   None,
            redraw_wanted: false,
            repaint_at:   None,
            frame_stats:  Timing::default(),
        }
    }

    /// Window pixels -> canvas pixels, through the view (zoom, rotation,
    /// pan): the exact inverse of what the GPU draws.
    fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        Some(self.gpu.as_ref()?.screen_to_canvas((wx, wy)))
    }

    /// Start panning / rotating the view with a drag from the cursor.
    fn begin_nav(&mut self, rotate: bool) {
        self.nav = Some((if rotate { Nav::Rotate } else { Nav::Pan }, self.cursor));
        self.update_cursor_icon();
    }

    fn end_nav(&mut self) {
        self.nav = None;
        self.update_cursor_icon();
    }

    /// Cursor moved during a navigation drag.
    fn drag_nav(&mut self) {
        let Some((mode, last)) = self.nav else { return };
        let Some(gpu) = &mut self.gpu else { return };
        let now = self.cursor;
        match mode {
            Nav::Pan => gpu.view.pan_by(now.0 - last.0, now.1 - last.1),
            Nav::Rotate => {
                // Turn by the angle the cursor swept around the view center.
                let c = gpu.view_center();
                let a0 = (last.1 - c.1).atan2(last.0 - c.0);
                let a1 = (now.1 - c.1).atan2(now.0 - c.0);
                gpu.view_rotate_at(aurastudio_gpu::wrap_angle(a1 - a0), c);
            }
        }
        self.nav = Some((mode, now));
        self.request_redraw();
    }

    /// Hand cursor while Space is held / the view is being dragged.
    fn update_cursor_icon(&self) {
        let Some(w) = &self.window else { return };
        let icon = match (self.nav, self.space_down) {
            (Some(_), _) => CursorIcon::Grabbing,
            (None, true) => CursorIcon::Grab,
            _ => CursorIcon::Default,
        };
        w.set_cursor(icon);
    }

    /// Keyboard / menu view commands.
    fn view_cmd(&mut self, cmd: ViewCmd) {
        let Some(gpu) = &mut self.gpu else { return };
        let c = gpu.view_center();
        let step = 15f32.to_radians();
        match cmd {
            ViewCmd::ZoomIn        => gpu.view_zoom_at(1.25, c),
            ViewCmd::ZoomOut       => gpu.view_zoom_at(1.0 / 1.25, c),
            ViewCmd::ActualSize    => gpu.view_set_zoom(1.0, c),
            ViewCmd::Fit           => gpu.view.fit_to_area(),
            ViewCmd::RotateLeft    => gpu.view_rotate_at(-step, c),
            ViewCmd::RotateRight   => gpu.view_rotate_at(step, c),
            ViewCmd::ResetRotation => gpu.view_reset_rotation(),
        }
        self.request_redraw();
    }

    /// Ask for a frame. The window redraw request itself is made in
    /// about_to_wait: one made while handling a redraw is dropped (Windows
    /// validates the window after WM_PAINT; X11 has already chosen how long
    /// to sleep). Posting Wake makes sure the loop gets there right away.
    fn request_redraw(&mut self) {
        if !self.redraw_wanted {
            self.redraw_wanted = true;
            let _ = self.proxy.send_event(AppEvent::Wake);
        }
    }

    /// Sleep until the next OS event, or until egui's delayed repaint.
    fn schedule(&self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(match self.repaint_at {
            Some(t) => ControlFlow::WaitUntil(t),
            None    => ControlFlow::Wait,
        });
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    fn begin_stroke(&mut self) {
        let refusal = {
            let doc = self.doc.lock().unwrap();
            if doc.layers.can_paint() {
                None
            } else if doc.layers.active_node().is_folder() {
                Some("Select a layer to paint (a folder is selected)")
            } else {
                Some("Layer is locked")
            }
        };
        if let Some(msg) = refusal {
            self.ui.notify(msg);
            self.is_drawing = false;
            self.request_redraw();
            return;
        }
        self.is_drawing = true;
        self.pen_active = false;
        self.frame_stats = Timing::default();
        self.paint.send(Cmd::Begin {
            doc:       Arc::clone(&self.doc),
            brush:     self.brush.clone(),
            hokusai:   self.use_hokusai,
            smoothing: self.smoothing,
        });
    }

    fn end_stroke(&mut self) {
        self.paint.send(Cmd::End);
        self.pen_active = false;
    }

    /// Forward one pen sample to the paint thread (never paints here).
    fn draw_at(&mut self, wx: f32, wy: f32) {
        let Some((x, y)) = self.window_to_canvas(wx, wy) else { return };
        self.paint.send(Cmd::Sample { x, y, pressure: self.pressure });
    }

    /// Native pen samples (WinTab or Windows Ink). Press / release still
    /// comes from MouseInput, which Windows always synthesizes for the pen;
    /// only position and pressure come from here.
    fn drain_stylus(&mut self) {
        let Some(stylus) = &self.stylus else { return };
        for s in stylus.drain() {
            if self.is_drawing {
                self.pen_active = true;
                self.pressure   = s.pressure;
                self.draw_at(s.x, s.y);
            }
        }
    }

    /// The paint thread changed some area: show it. The layers are only
    /// recomposited once per displayed frame (in render_frame), not once
    /// per pen sample: at 200 px that recomposite was most of the cost.
    fn on_painted(&mut self) {
        if let Some(r) = self.paint.take_dirty() {
            self.pending = Some(match self.pending {
                None    => r,
                Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
            });
        }
        self.request_redraw();
        self.render_if_due();
    }

    /// Render now if a frame is pending and the last one is old enough.
    ///
    /// We can't rely on RedrawRequested alone while drawing: on Windows it
    /// comes from WM_PAINT, which the OS only sends once the message queue
    /// is empty. With big brushes each pen packet takes a few ms, packets
    /// keep arriving, the queue never empties and nothing was displayed
    /// until the pen was lifted. Krita likewise refreshes the canvas on its
    /// own timer rather than waiting for the window system.
    fn render_if_due(&mut self) {
        const MIN_FRAME: Duration = Duration::from_millis(4);
        if self.pending.is_none() { return; }
        if self.last_frame.is_some_and(|t| t.elapsed() < MIN_FRAME) { return; }
        self.render_frame();
    }

    fn render_frame(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        let t0 = Instant::now();

        if let Some(rect) = self.pending.take() {
            self.doc.lock().unwrap().layers.composite_rect_into(&mut gpu.canvas.pixels, rect);
            gpu.canvas.mark_dirty(rect);
        }

        let max_canvas = gpu.max_canvas_size();
        let (view_zoom, view_angle) = (gpu.view.zoom, gpu.view.angle.to_degrees());
        let ui_state   = &mut self.ui;
        let doc        = &self.doc;
        let brush      = &mut self.brush;
        let use_hokusai = &mut self.use_hokusai;
        let smoothing  = &mut self.smoothing;
        let canvas_edit = &mut self.canvas_edit;
        let mut actions = Actions::default();
        gpu.render(|ctx| {
            // Locked only while the UI is built, not during the GPU submit,
            // so the paint thread is held up as little as possible.
            let mut doc = doc.lock().unwrap();
            let (can_undo, can_redo) = (doc.history().undo_len() > 0, doc.history().redo_len() > 0);
            let mut model = Model {
                brush,
                use_hokusai,
                pressure_smoothing: smoothing,
                layers: &mut doc.layers,
                can_undo,
                can_redo,
                view_zoom,
                view_angle,
                canvas_edit,
                max_canvas,
            };
            aurastudio_ui::show(ctx, ui_state, &mut model, &mut actions);
        });
        let mut want_frame = false;
        match gpu.take_repaint_delay() {
            Some(d) if d.is_zero() => want_frame = true,
            // egui says "never" with a huge delay.
            Some(d) if d < Duration::from_secs(3600) => {
                let at = Instant::now() + d;
                self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            }
            _ => {}
        }
        if self.is_drawing {
            self.frame_stats.add(t0.elapsed());
        }
        self.last_frame = Some(Instant::now());

        if let Some(cmd) = actions.view { self.view_cmd(cmd); }
        if actions.undo { self.undo(); }
        if actions.redo { self.redo(); }
        if actions.clear_layer { self.clear_active_layer(); }
        let Some(gpu) = &mut self.gpu else { return };
        if actions.resize_to.is_some() || actions.recomposite || self.layers_dirty {
            let mut doc = self.doc.lock().unwrap();
            if let Some((w, h)) = actions.resize_to {
                log::info!("canvas resize: {}x{} -> {w}x{h} (undo history cleared)", doc.layers.width, doc.layers.height);
                doc.resize(w, h);
                gpu.resize_canvas(w, h);
                self.ui.invalidate_all_thumbnails();
            }
            let (w, h) = (doc.layers.width, doc.layers.height);
            doc.layers.composite_into(&mut gpu.canvas.pixels);
            gpu.canvas.mark_dirty((0, 0, w, h));
            self.layers_dirty = false;
            // Uploaded with the next frame.
            want_frame = true;
        }
        if want_frame {
            self.request_redraw();
        }
    }

    /// Delete key / Layer > Clear: erase the active layer (white for the
    /// background, transparent for the others), undoable. Done by the paint
    /// thread, after any stroke still being painted.
    fn clear_active_layer(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Clear(Arc::clone(&self.doc)));
    }

    fn undo(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Undo(Arc::clone(&self.doc)));
    }

    fn redo(&mut self) {
        if self.is_drawing { return; }
        self.paint.send(Cmd::Redo(Arc::clone(&self.doc)));
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            // Only there to wake the loop; about_to_wait does the work.
            AppEvent::Wake => {}
            AppEvent::Painted => self.on_painted(),
            AppEvent::LayerChanged(layer_id) => {
                if self.frame_stats.count > 0 {
                    log::info!("stroke frames (composite+render, main thread): {}", self.frame_stats);
                    self.frame_stats = Timing::default();
                }
                self.on_painted();
                self.ui.invalidate_thumbnail(layer_id);
                self.request_redraw();
            }
            AppEvent::Notice(text) => {
                self.ui.notify(text);
                self.request_redraw();
            }
        }
    }

    /// Called once winit has dispatched every pending OS message, right
    /// before it waits for new ones. WinTab packets arrive as their own
    /// window messages, which produce no winit event: draining here paints
    /// them immediately instead of waiting for the next mouse event.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_stylus();

        if self.repaint_at.is_some_and(|t| Instant::now() >= t) {
            self.repaint_at = None;
            self.redraw_wanted = true;
        }
        if std::mem::take(&mut self.redraw_wanted) {
            if let Some(w) = &self.window { w.request_redraw(); }
        }
        self.schedule(event_loop);
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("AuraStudio v0.1")
            .with_inner_size(winit::dpi::LogicalSize::new(1440u32, 900u32));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size   = {
            let d = self.doc.lock().unwrap();
            (d.layers.width, d.layers.height)
        };
        let mut gpu = pollster::block_on(GpuState::new(Arc::clone(&window), size));
        gpu.clear_color = aurastudio_ui::theme::backdrop_rgb();
        log::info!("max canvas size: {} px", gpu.max_canvas_size());

        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                let stylus = Stylus::install(h.hwnd.get());
                log::info!("pen input backend: {}", stylus.backend());
                self.stylus = Some(stylus);
            }
        }

        window.request_redraw();
        self.window = Some(window);
        self.gpu    = Some(gpu);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id:        WindowId,
        event:      WindowEvent,
    ) {
        // egui sees every event and says when it needs a new frame (hover,
        // slider drag...). Whether a press starts a stroke is decided by
        // GpuState::is_canvas_at, see there for why egui can't decide it.
        //
        // RedrawRequested itself must never ask for another redraw here:
        // that made an endless loop (1.6 CPU cores while idle with the
        // cursor over a docker).
        let ui_needs_frame = self.gpu.as_mut()
            .map(|gpu| gpu.egui_handle_event(&event))
            .unwrap_or(false);
        let over_canvas = self.gpu.as_ref()
            .map(|gpu| gpu.is_canvas_at(self.cursor))
            .unwrap_or(false);
        let ui_dragging = self.gpu.as_ref().is_some_and(|g| g.ui_is_dragging());
        let is_redraw = matches!(event, WindowEvent::RedrawRequested);
        if !is_redraw && (ui_needs_frame || ui_dragging) {
            self.request_redraw();
        }

        self.drain_stylus();

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                self.request_redraw();
            }

            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),

            WindowEvent::KeyboardInput { event: key, .. } => {
                // Space is tracked on release too (and even while a field
                // has focus, so it can't get stuck down).
                if key.logical_key == Key::Named(NamedKey::Space) {
                    let down = key.state == ElementState::Pressed;
                    if down != self.space_down {
                        self.space_down = down;
                        self.update_cursor_icon();
                    }
                }
                // Typing in a docker field (e.g. canvas width) must not
                // erase the layer, undo or move the view.
                let ui_typing = self.gpu.as_ref().is_some_and(|g| g.ui_wants_keyboard());
                if key.state != ElementState::Pressed || ui_typing { return; }
                let ctrl  = self.modifiers.control_key() || self.modifiers.super_key();
                let shift = self.modifiers.shift_key();
                match key.logical_key.as_ref() {
                    Key::Named(NamedKey::Delete) if !key.repeat => self.clear_active_layer(),
                    // Ctrl+Z undo, Ctrl+Shift+Z / Ctrl+Y redo (Krita, CSP,
                    // Photoshop). Holding the keys repeats, like everywhere.
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("z") => {
                        if shift { self.redo() } else { self.undo() }
                    }
                    Key::Character(c) if ctrl && c.eq_ignore_ascii_case("y") => self.redo(),
                    // View keys (Krita): 1 = 100 %, 2 = fit, 4 / 6 = rotate
                    // by 15 degrees, 5 = reset rotation, + / - = zoom.
                    Key::Character(c) if !ctrl => match c {
                        "1" => self.view_cmd(ViewCmd::ActualSize),
                        "2" => self.view_cmd(ViewCmd::Fit),
                        "4" => self.view_cmd(ViewCmd::RotateLeft),
                        "6" => self.view_cmd(ViewCmd::RotateRight),
                        "5" => self.view_cmd(ViewCmd::ResetRotation),
                        "+" | "=" => self.view_cmd(ViewCmd::ZoomIn),
                        "-" => self.view_cmd(ViewCmd::ZoomOut),
                        _ => {}
                    },
                    _ => {}
                }
            }

            // A release always ends the stroke, even over a dock; otherwise
            // releasing above a panel left the app stuck in drawing mode.
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                if state.is_pressed() {
                    if over_canvas && self.space_down {
                        // Space + drag: pan; Shift + Space + drag: rotate (Krita).
                        self.begin_nav(self.modifiers.shift_key());
                    } else if over_canvas {
                        self.pressure   = 1.0;
                        self.begin_stroke();
                    }
                } else if self.nav.is_some() {
                    self.end_nav();
                } else if self.is_drawing {
                    self.is_drawing = false;
                    self.end_stroke();
                }
            }

            // Middle drag: pan, Shift + middle drag: rotate.
            WindowEvent::MouseInput { button: MouseButton::Middle, state, .. } => {
                if state.is_pressed() && over_canvas && !self.is_drawing {
                    self.begin_nav(self.modifiers.shift_key());
                } else if !state.is_pressed() && self.nav.is_some() {
                    self.end_nav();
                }
            }

            // Wheel: zoom toward the cursor.
            WindowEvent::MouseWheel { delta, .. } if over_canvas && !self.is_drawing => {
                use winit::event::MouseScrollDelta;
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
                };
                if let Some(gpu) = &mut self.gpu {
                    gpu.view_zoom_at(1.2f32.powf(notches), self.cursor);
                    self.request_redraw();
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.nav.is_some() {
                    self.drag_nav();
                } else if self.is_drawing && !self.pen_active {
                    self.draw_at(self.cursor.0, self.cursor.1);
                }
            }

            // macOS / Linux fallback only: on Windows, drain_stylus already
            // handles the pen, so Touch is ignored there to avoid drawing twice.
            WindowEvent::Touch(touch) if cfg!(not(target_os = "windows")) => {
                use winit::event::{Force, TouchPhase};
                self.pressure = match touch.force {
                    Some(Force::Normalized(f)) => f as f32,
                    Some(Force::Calibrated { force, max_possible_force, .. }) => {
                        (force / max_possible_force) as f32
                    }
                    None => 1.0,
                };
                let pos = (touch.location.x as f32, touch.location.y as f32);
                match touch.phase {
                    TouchPhase::Started => {
                        if !self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos)) {
                            return;
                        }
                        self.begin_stroke();
                        if self.is_drawing {
                            self.draw_at(pos.0, pos.1);
                        }
                    }
                    TouchPhase::Moved => {
                        if self.is_drawing {
                            self.draw_at(pos.0, pos.1);
                        }
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.is_drawing = false;
                        self.end_stroke();
                    }
                }
            }

            WindowEvent::RedrawRequested => self.render_frame(),

            _ => {}
        }
    }
}
