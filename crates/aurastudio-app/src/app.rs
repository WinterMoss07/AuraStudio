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

/// Canvas size at startup; changed afterwards from the Canvas docker.
const DEFAULT_CANVAS: (u32, u32) = (2048, 2048);

/// Brush size range shown in the UI, as a diameter in pixels.
const MAX_BRUSH_SIZE: u32 = 1000;

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
    /// Width / height typed in the Canvas docker, applied by "Resize".
    canvas_edit: (u32, u32),

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

/// What the docks asked for during a frame, applied once the UI is done.
#[derive(Default)]
struct UiActions {
    recomposite: bool,
    resize_to:   Option<(u32, u32)>,
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

    fn begin_stroke(&mut self) {
        self.layers.begin_stroke();
        if self.use_hokusai {
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
            let rect = self.hokusai.end_stroke(&mut self.layers.active_surface());
            self.present_rect(rect);
        }
        self.last_pos   = None;
        self.pen_active = false;
    }

    fn draw_at(&mut self, wx: f32, wy: f32) {
        let Some(pos) = self.window_to_canvas(wx, wy) else { return };
        let t0 = Instant::now();
        let pressure = self.pressure;
        let mut surface = self.layers.active_surface();

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

        let max_canvas  = gpu.max_canvas_size();
        let brush       = &mut self.brush;
        let use_hokusai = &mut self.use_hokusai;
        let layers      = &mut self.layers;
        let canvas_edit = &mut self.canvas_edit;
        let mut actions = UiActions::default();
        gpu.render(|ctx| {
            color_panel(ctx, brush, use_hokusai);
            layers_panel(ctx, layers, &mut actions);
            canvas_panel(ctx, layers, canvas_edit, max_canvas, &mut actions);
        });
        if self.is_drawing {
            self.stats.frame.add(t0.elapsed());
        }
        self.last_frame = Some(Instant::now());

        if let Some((w, h)) = actions.resize_to {
            log::info!("canvas resize: {}x{} -> {w}x{h}", self.layers.width, self.layers.height);
            self.layers.resize(w, h);
            gpu.resize_canvas(w, h);
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
        self.layers.clear_active();
        self.layers_dirty = true;
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 800u32));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let size   = (self.layers.width, self.layers.height);
        let gpu    = pollster::block_on(GpuState::new(Arc::clone(&window), size));
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
        if consumed_by_ui || (!self.is_drawing && !over_canvas) {
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
                        self.is_drawing = true;
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
                        self.is_drawing = true;
                        self.begin_stroke();
                        self.draw_at(pos.0, pos.1);
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

/// Color docker v1: preview swatch, RGB and HSV sliders kept in sync with
/// the brush color, brush size / opacity. Krita / CSP-style docks are next.
fn color_panel(ctx: &egui::Context, brush: &mut Brush, use_hokusai: &mut bool) {
    egui::Window::new("Color")
        .resizable(false)
        .default_pos([16.0, 16.0])
        .show(ctx, |ui| {
            let [r, g, b] = brush.color;
            let mut hsva = egui::ecolor::Hsva::from_srgb([r, g, b]);

            let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, egui::Color32::from(hsva));

            ui.separator();
            ui.label("RGB");
            let mut r = brush.color[0];
            let mut g = brush.color[1];
            let mut b = brush.color[2];
            ui.add(egui::Slider::new(&mut r, 0..=255).text("R"));
            ui.add(egui::Slider::new(&mut g, 0..=255).text("G"));
            ui.add(egui::Slider::new(&mut b, 0..=255).text("B"));
            brush.color = [r, g, b];

            ui.separator();
            ui.label("HSV");
            let changed_h = ui.add(egui::Slider::new(&mut hsva.h, 0.0..=1.0).text("H")).changed();
            let changed_s = ui.add(egui::Slider::new(&mut hsva.s, 0.0..=1.0).text("S")).changed();
            let changed_v = ui.add(egui::Slider::new(&mut hsva.v, 0.0..=1.0).text("V")).changed();
            if changed_h || changed_s || changed_v {
                brush.color = hsva.to_srgb();
            }

            ui.separator();
            ui.label("Brush");
            // Shown as a diameter (like Krita / CSP), stored as a radius.
            // Logarithmic so small sizes stay precise on a 1..1000 range.
            let mut size = brush.radius * 2;
            if ui.add(egui::Slider::new(&mut size, 1..=MAX_BRUSH_SIZE).logarithmic(true).text("Size (px)")).changed() {
                brush.radius = (size / 2).max(1);
            }
            ui.add(egui::Slider::new(&mut brush.opacity, 0.0..=1.0).text("Opacity"));
            ui.checkbox(use_hokusai, "hokusai engine (libmypaint, textured brushes)");
        });
}

/// Layers docker v1: list top to bottom (like Krita / CSP), visibility,
/// opacity, active layer, add / delete.
fn layers_panel(ctx: &egui::Context, layers: &mut LayerStack, actions: &mut UiActions) {
    egui::Window::new("Layers")
        .resizable(false)
        .default_pos([16.0, 380.0])
        .show(ctx, |ui| {
            let mut remove = None;

            ui.horizontal(|ui| {
                if ui.button("+ New layer").clicked() {
                    let n = layers.layers.len();
                    layers.add_layer(&format!("Layer {n}"));
                    actions.recomposite = true;
                }
                ui.label("Del: clear active layer");
            });
            ui.separator();

            // Top of the list = end of the Vec.
            for i in (0..layers.layers.len()).rev() {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut layers.layers[i].visible, "").changed() {
                        actions.recomposite = true;
                    }
                    let active = layers.active == i;
                    if ui.selectable_label(active, &layers.layers[i].name).clicked() {
                        layers.set_active(i);
                    }
                    if layers.layers.len() > 1 && ui.small_button("🗑").clicked() {
                        // Removed after the loop: removing here made the
                        // opacity slider below read a layer that no longer
                        // existed (index out of bounds -> crash).
                        remove = Some(i);
                    }
                });
                let opacity = egui::Slider::new(&mut layers.layers[i].opacity, 0.0..=1.0).text("opacity");
                if ui.add(opacity).changed() {
                    actions.recomposite = true;
                }
            }

            if let Some(i) = remove {
                layers.remove_layer(i);
                actions.recomposite = true;
            }
        });
}

/// Canvas docker: type a width and height, then Resize. The content stays
/// centered (margins added or borders cropped, nothing is scaled).
fn canvas_panel(
    ctx: &egui::Context,
    layers: &LayerStack,
    edit: &mut (u32, u32),
    max_size: u32,
    actions: &mut UiActions,
) {
    // Top-right corner until the docks land (the left column is full).
    let right = ctx.screen_rect().right();
    egui::Window::new("Canvas")
        .resizable(false)
        .default_pos([right - 220.0, 16.0])
        .show(ctx, |ui| {
            ui.label(format!("Current: {} x {} px", layers.width, layers.height));
            egui::Grid::new("canvas_size").num_columns(2).show(ui, |ui| {
                ui.label("Width");
                ui.add(egui::DragValue::new(&mut edit.0).range(1..=max_size).suffix(" px"));
                ui.end_row();
                ui.label("Height");
                ui.add(egui::DragValue::new(&mut edit.1).range(1..=max_size).suffix(" px"));
                ui.end_row();
            });

            // RAM estimate: 4 bytes/px per layer, plus the composite (4),
            // the stroke snapshot (4) and the stroke mask (1).
            let px = edit.0 as f64 * edit.1 as f64;
            let mb = px * (4.0 * layers.layers.len() as f64 + 9.0) / (1024.0 * 1024.0);
            ui.label(format!("≈ {mb:.0} MB RAM with {} layer(s)", layers.layers.len()));

            let unchanged = *edit == (layers.width, layers.height);
            if ui.add_enabled(!unchanged, egui::Button::new("Resize")).clicked() {
                actions.resize_to = Some(*edit);
            }
        });
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
