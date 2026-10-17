use std::sync::Arc;
use std::time::{Duration, Instant};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{Key, NamedKey},
    window::{Window, WindowId},
};

use aurastudio_brush::{stroke_line, Brush, HokusaiEngine, PressureSmoother};
use aurastudio_canvas::Rect;
use aurastudio_gpu::GpuState;
use aurastudio_input::Stylus;
use aurastudio_layers::LayerStack;
use aurastudio_ui::{Actions, Model, UiState};

/// Canvas size at startup; changed afterwards from the Canvas docker.
const DEFAULT_CANVAS: (u32, u32) = (2048, 2048);

pub struct App {
    window: Option<Arc<Window>>,
    gpu:    Option<GpuState>,
    stylus: Option<Stylus>,

    brush:        Brush,
    hokusai:      HokusaiEngine,
    /// true = hokusai engine (libmypaint, for textured brushes later),
    /// false = swept round pen (default).
    use_hokusai:  bool,
    pen_smoother: PressureSmoother,

    layers: LayerStack,
    /// Width / height typed in the Canvas dock, applied by "Resize".
    canvas_edit: (u32, u32),
    ui:          UiState,

    is_drawing: bool,
    /// Last sample of the current stroke: canvas x, y and smoothed pressure.
    last_pos:   Option<(f32, f32, f32)>,
    pressure:   f32,
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
    stats:        StrokeStats,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window: None,
            gpu:    None,
            stylus: None,

            brush:        Brush::new(),
            hokusai:      HokusaiEngine::new(),
            use_hokusai:  false,
            pen_smoother: PressureSmoother::new(0.5),

            layers:      LayerStack::new(DEFAULT_CANVAS.0, DEFAULT_CANVAS.1),
            canvas_edit: DEFAULT_CANVAS,
            ui:          UiState::new(),

            is_drawing: false,
            last_pos:   None,
            pressure:   1.0,
            pen_active: false,
            cursor:     (0.0, 0.0),

            pending:      None,
            layers_dirty: false,
            last_frame:   None,
            stats:        StrokeStats::default(),
        }
    }
}

impl App {
    /// Window pixels -> canvas pixels. The canvas is letterboxed inside the
    /// free area left by the docks (the GPU viewport), not the whole window,
    /// so both the viewport origin and its size matter here.
    fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        let gpu = self.gpu.as_ref()?;
        let [vx, vy, vw, vh] = gpu.canvas_viewport();
        let cw = gpu.canvas.width  as f32;
        let ch = gpu.canvas.height as f32;

        let scale    = (vw / cw).min(vh / ch);
        let offset_x = vx + (vw - cw * scale) / 2.0;
        let offset_y = vy + (vh - ch * scale) / 2.0;

        Some(((wx - offset_x) / scale, (wy - offset_y) / scale))
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window { w.request_redraw(); }
    }

    /// Start a stroke on the active layer. Refused (with a notice in the
    /// options bar) when it is locked or a folder is selected.
    fn begin_stroke(&mut self) {
        if !self.layers.can_paint() {
            let msg = if self.layers.active_node().is_folder() {
                "Select a layer to paint (a folder is selected)"
            } else {
                "Layer is locked"
            };
            self.ui.notify(msg);
            self.is_drawing = false;
            self.request_redraw();
            return;
        }
        self.is_drawing = true;
        self.layers.begin_stroke();
        if self.use_hokusai {
            self.hokusai.smoother.span = self.pen_smoother.span;
            self.hokusai.sync_from(&self.brush);
            self.hokusai.begin_stroke();
        }
        self.last_pos   = None;
        self.pen_active = false;
        self.pen_smoother.reset();
    }

    fn end_stroke(&mut self) {
        if self.stats.paint.count > 0 {
            log::info!("stroke: {} samples | paint {} | frame (composite+render) {}",
                self.stats.paint.count, self.stats.paint, self.stats.frame);
        }
        self.stats = StrokeStats::default();
        if self.use_hokusai && self.last_pos.is_some() {
            if let Some(mut surface) = self.layers.active_surface() {
                let rect = self.hokusai.end_stroke(&mut surface);
                self.present_rect(rect);
            }
        }
        if self.last_pos.is_some() {
            self.ui.invalidate_thumbnail(self.layers.active_id());
            self.request_redraw();
        }
        self.last_pos   = None;
        self.pen_active = false;
    }

    fn draw_at(&mut self, wx: f32, wy: f32) {
        let Some(pos) = self.window_to_canvas(wx, wy) else { return };
        let t0 = Instant::now();
        let pressure = self.pressure;
        let Some(mut surface) = self.layers.active_surface() else { return };

        let rect = if self.use_hokusai {
            self.last_pos = Some((pos.0, pos.1, pressure));
            self.hokusai.stroke_to(&mut surface, pos.0, pos.1, pressure)
        } else {
            let p = self.pen_smoother.apply(pos.0, pos.1, pressure, self.brush.radius as f32);
            let (fx, fy, fp) = self.last_pos.unwrap_or((pos.0, pos.1, p));
            self.last_pos = Some((pos.0, pos.1, p));
            stroke_line((fx, fy), pos, &self.brush, &mut surface, fp, p)
        };
        self.stats.paint.add(t0.elapsed());
        self.present_rect(rect);
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

    /// Record that `rect` of the active layer changed. The layers are only
    /// recomposited once per displayed frame (in render_frame), not once
    /// per pen sample: at 200 px that recomposite was most of the cost.
    fn present_rect(&mut self, rect: Option<Rect>) {
        let Some(r) = rect else { return };
        self.pending = Some(match self.pending {
            None    => r,
            Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
        });
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
            self.layers.composite_rect_into(&mut gpu.canvas.pixels, rect);
            gpu.canvas.mark_dirty(rect);
        }

        let max_canvas = gpu.max_canvas_size();
        let ui_state   = &mut self.ui;
        let mut model = Model {
            brush:              &mut self.brush,
            use_hokusai:        &mut self.use_hokusai,
            pressure_smoothing: &mut self.pen_smoother.span,
            layers:             &mut self.layers,
            canvas_edit:        &mut self.canvas_edit,
            max_canvas,
        };
        let mut actions = Actions::default();
        gpu.render(|ctx| aurastudio_ui::show(ctx, ui_state, &mut model, &mut actions));
        if self.is_drawing {
            self.stats.frame.add(t0.elapsed());
        }
        self.last_frame = Some(Instant::now());

        if let Some((w, h)) = actions.resize_to {
            log::info!("canvas resize: {}x{} -> {w}x{h}", self.layers.width, self.layers.height);
            self.layers.resize(w, h);
            gpu.resize_canvas(w, h);
            self.ui.invalidate_all_thumbnails();
            self.layers_dirty = true;
        }
        if actions.recomposite || self.layers_dirty {
            let (w, h) = (self.layers.width, self.layers.height);
            self.layers.composite_into(&mut gpu.canvas.pixels);
            gpu.canvas.mark_dirty((0, 0, w, h));
            self.layers_dirty = false;
            // Uploaded with the next frame.
            self.request_redraw();
        }
    }

    /// Delete key: erase the active layer (white for the background,
    /// transparent for the others), like Krita / CSP.
    fn clear_active_layer(&mut self) {
        if self.is_drawing { return; }
        if self.layers.clear_active() {
            self.ui.invalidate_thumbnail(self.layers.active_id());
            self.layers_dirty = true;
        } else {
            self.ui.notify("Layer is locked or is a folder");
        }
        self.request_redraw();
    }
}

impl ApplicationHandler for App {
    /// Called once winit has dispatched every pending OS message, right
    /// before it waits for new ones. WinTab packets arrive as their own
    /// window messages, which produce no winit event: draining here paints
    /// them immediately instead of waiting for the next mouse event.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.drain_stylus();
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("AuraStudio v0.1")
            .with_inner_size(winit::dpi::LogicalSize::new(1440u32, 900u32));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size   = (self.layers.width, self.layers.height);
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
        // egui sees every event. Its "consumed" flag is only used to know
        // when the UI needs a new frame (hover, slider drag...); whether a
        // press starts a stroke is decided by GpuState::is_canvas_at, see
        // there for why egui's flag can't be trusted for that.
        let consumed_by_ui = self.gpu.as_mut()
            .map(|gpu| gpu.egui_handle_event(&event))
            .unwrap_or(false);
        let over_canvas = self.gpu.as_ref()
            .map(|gpu| gpu.is_canvas_at(self.cursor))
            .unwrap_or(false);
        let ui_dragging = self.gpu.as_ref().is_some_and(|g| g.ui_is_dragging());
        if consumed_by_ui || ui_dragging || (!self.is_drawing && !over_canvas) {
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

            WindowEvent::KeyboardInput { event: key, .. } => {
                // Typing in a docker field (e.g. canvas width) must not
                // erase the layer.
                let ui_typing = self.gpu.as_ref().is_some_and(|g| g.ui_wants_keyboard());
                if key.state == ElementState::Pressed
                    && !key.repeat
                    && !ui_typing
                    && key.logical_key == Key::Named(NamedKey::Delete)
                {
                    self.clear_active_layer();
                }
            }

            // A release always ends the stroke, even over a dock; otherwise
            // releasing above a panel left the app stuck in drawing mode.
            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                if state.is_pressed() {
                    if over_canvas {
                        self.pressure   = 1.0;
                        self.begin_stroke();
                    }
                } else if self.is_drawing {
                    self.is_drawing = false;
                    self.end_stroke();
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.is_drawing && !self.pen_active {
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

/// CPU time spent per stroke, logged when the stroke ends so latency can be
/// measured on the real machine instead of guessed.
#[derive(Default)]
struct StrokeStats {
    paint: Timing,
    frame: Timing,
}

#[derive(Default)]
struct Timing {
    count: u32,
    total: Duration,
    max:   Duration,
}

impl Timing {
    fn add(&mut self, d: Duration) {
        self.count += 1;
        self.total += d;
        self.max = self.max.max(d);
    }
}

impl std::fmt::Display for Timing {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.count == 0 { return write!(f, "-"); }
        let avg = self.total.as_secs_f64() * 1000.0 / self.count as f64;
        write!(f, "avg {avg:.2} ms, max {:.2} ms", self.max.as_secs_f64() * 1000.0)
    }
}
