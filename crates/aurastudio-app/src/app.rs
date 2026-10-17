use std::sync::Arc;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};
use aurastudio_brush::{stroke_line, Brush, HokusaiEngine, PressureSmoother};
use aurastudio_gpu::GpuState;
use aurastudio_input::Stylus;
use aurastudio_layers::LayerStack;

/// Taille fixe du canvas pour l'instant (doit matcher Canvas::new dans
/// aurastudio-gpu, 2048x2048) — passera en paramètre config plus tard.
const CANVAS_SIZE: u32 = 2048;

pub struct App {
    window:     Option<Arc<Window>>,
    gpu:        Option<GpuState>,
    brush:      Brush,
    layers:     LayerStack,
    is_drawing:   bool,
    /// Last sample of the current stroke: canvas x, y and smoothed pressure.
    last_pos:     Option<(f32, f32, f32)>,
    pen_smoother: PressureSmoother,
    pressure:     f32,
    stylus:       Option<Stylus>,
    /// true quand un changement hors-trait (visibilité, opacité, ajout/
    /// suppression de calque) demande une recomposition pleine image,
    /// plutôt que le composite_rect_into local fait pendant un trait.
    layers_dirty: bool,
    stats:        StrokeStats,
    /// Canvas area painted since the last frame, recomposited at render.
    pending:      Option<aurastudio_canvas::Rect>,
    last_frame:   Option<std::time::Instant>,
    hokusai:      HokusaiEngine,
    /// true = moteur hokusai (libmypaint), false = brush rond simple.
    use_hokusai:  bool,
    /// true dès que le stylet natif a envoyé des samples pendant le trait
    /// en cours : on ignore alors les CursorMoved que Windows synthétise
    /// pour le même stylet, sinon le trait reçoit deux flux de positions
    /// légèrement décalées (bords ondulés / zigzag).
    pen_active:   bool,
    /// Last known cursor position, physical window pixels.
    cursor:       (f32, f32),
}

impl Default for App {
    fn default() -> Self {
        Self {
            window:       None,
            gpu:          None,
            brush:        Brush::new(),
            layers:       LayerStack::new(CANVAS_SIZE, CANVAS_SIZE),
            is_drawing:   false,
            last_pos:     None,
            pen_smoother: PressureSmoother::new(0.5),
            pressure:     1.0,
            stylus:       None,
            layers_dirty: false,
            stats:        StrokeStats::default(),
            pending:      None,
            last_frame:   None,
            hokusai:      HokusaiEngine::new(),
            use_hokusai:  false,
            pen_active:   false,
            cursor:       (0.0, 0.0),
        }
    }
}

impl App {
    fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        // Window pixels -> canvas pixels. The canvas is letterboxed inside
        // the free area left by the docks (the GPU viewport), not the whole
        // window, so both the viewport origin and its size matter here.
        let gpu = self.gpu.as_ref()?;
        let [vx, vy, vw, vh] = gpu.canvas_viewport();
        let cw = gpu.canvas.width  as f32;
        let ch = gpu.canvas.height as f32;

        let scale    = (vw / cw).min(vh / ch);
        let offset_x = vx + (vw - cw * scale) / 2.0;
        let offset_y = vy + (vh - ch * scale) / 2.0;

        Some(((wx - offset_x) / scale, (wy - offset_y) / scale))
    }

    fn begin_stroke(&mut self) {
        self.layers.active_layer_mut().begin_stroke();
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
            let rect = self.hokusai.end_stroke(self.layers.active_layer_mut());
            self.present_rect(rect);
        }
        self.last_pos   = None;
        self.pen_active = false;
    }

    /// Recompose uniquement le rectangle touché (pas tout le canvas)
    /// dans le buffer GPU, puis marque la zone dirty pour upload.
    /// Record that `rect` of the active layer changed. The layers are only
    /// recomposited once per displayed frame (in render_frame), not once
    /// per pen sample: at 200 px that recomposite was most of the cost.
    fn present_rect(&mut self, rect: Option<aurastudio_canvas::Rect>) {
        let Some(r) = rect else { return };
        self.pending = Some(match self.pending {
            None    => r,
            Some(d) => (d.0.min(r.0), d.1.min(r.1), d.2.max(r.2), d.3.max(r.3)),
        });
        if let Some(w) = &self.window {
            w.request_redraw();
        }
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
        const MIN_FRAME: std::time::Duration = std::time::Duration::from_millis(4);
        if self.pending.is_none() { return; }
        if self.last_frame.is_some_and(|t| t.elapsed() < MIN_FRAME) { return; }
        self.render_frame();
    }

    fn render_frame(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        let t0 = std::time::Instant::now();

        if let Some(rect) = self.pending.take() {
            self.layers.composite_rect_into(&mut gpu.canvas.pixels, rect);
            gpu.canvas.mark_dirty(rect);
        }

        let brush       = &mut self.brush;
        let layers      = &mut self.layers;
        let use_hokusai = &mut self.use_hokusai;
        let mut changed = false;
        gpu.render(|ctx| {
            color_picker_panel(ctx, brush, use_hokusai);
            changed = layers_panel(ctx, layers);
        });
        if self.is_drawing {
            self.stats.frame.add(t0.elapsed());
        }
        self.last_frame = Some(std::time::Instant::now());

        if changed || self.layers_dirty {
            self.layers.composite_into(&mut gpu.canvas.pixels);
            gpu.canvas.mark_dirty((0, 0, CANVAS_SIZE, CANVAS_SIZE));
            self.layers_dirty = false;
            // Uploaded with the next frame.
            if let Some(w) = &self.window { w.request_redraw(); }
        }
    }

    fn draw_at(&mut self, wx: f32, wy: f32) {
        let Some(pos) = self.window_to_canvas(wx, wy) else { return };
        let t0 = std::time::Instant::now();
        let pressure = self.pressure;
        let layer    = self.layers.active_layer_mut();

        let rect = if self.use_hokusai {
            self.last_pos = Some((pos.0, pos.1, pressure));
            self.hokusai.stroke_to(layer, pos.0, pos.1, pressure)
        } else {
            let p = self.pen_smoother.apply(pos.0, pos.1, pressure, self.brush.radius as f32);
            let (fx, fy, fp) = self.last_pos.unwrap_or((pos.0, pos.1, p));
            self.last_pos = Some((pos.0, pos.1, p));
            stroke_line((fx, fy), pos, &self.brush, layer, fp, p)
        };
        self.stats.paint.add(t0.elapsed());
        self.present_rect(rect);
    }

    /// Draine les échantillons stylet natifs (WinTab ou Windows Ink via
    /// efude-input). Le press/release reste géré par MouseInput, car
    /// Windows synthétise toujours des events souris compatibles pour
    /// le stylet — c'est uniquement la position/pression qui vient d'ici.
    fn drain_stylus(&mut self) {
        let Some(stylus) = &self.stylus else { return; };
        let samples = stylus.drain();
        for s in samples {
            if self.is_drawing {
                self.pen_active = true;
                self.pressure   = s.pressure;
                self.draw_at(s.x, s.y);
            }
        }
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
        let gpu    = pollster::block_on(GpuState::new(Arc::clone(&window)));

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
            if let Some(w) = &self.window { w.request_redraw(); }
        }

        self.drain_stylus();

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
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

            // Fallback macOS/Linux uniquement : sous Windows, drain_stylus
            // (efude-input) gère déjà tout, donc on ignore Touch ici pour
            // ne pas dessiner en double.
            WindowEvent::Touch(touch) if cfg!(not(target_os = "windows")) => {
                use winit::event::{Force, TouchPhase};
                self.pressure = match touch.force {
                    Some(Force::Normalized(f)) => f as f32,
                    Some(Force::Calibrated { force, max_possible_force, .. }) => {
                        (force / max_possible_force) as f32
                    }
                    None => 1.0,
                };
                match touch.phase {
                    TouchPhase::Started => {
                        let pos = (touch.location.x as f32, touch.location.y as f32);
                        if !self.gpu.as_ref().is_some_and(|g| g.is_canvas_at(pos)) {
                            return;
                        }
                        self.is_drawing = true;
                        self.begin_stroke();
                        self.draw_at(touch.location.x as f32, touch.location.y as f32);
                    }
                    TouchPhase::Moved => {
                        if self.is_drawing {
                            self.draw_at(touch.location.x as f32, touch.location.y as f32);
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

/// Panneau egui : carré de preview + sliders RGB et HSV, synchronisés
/// avec la couleur active du brush. Volontairement minimal (pas encore
/// de calques/palette) — brique color picker v1.
fn color_picker_panel(ctx: &egui::Context, brush: &mut Brush, use_hokusai: &mut bool) {
    egui::Window::new("Couleur")
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
            ui.label("Pinceau");
            ui.add(egui::Slider::new(&mut brush.radius, 1..=200).text("Taille"));
            ui.add(egui::Slider::new(&mut brush.opacity, 0.0..=1.0).text("Opacité"));
            ui.checkbox(use_hokusai, "Moteur hokusai (libmypaint, textured brushes)");
        });
}

/// Panneau egui : liste des calques (du haut vers le bas, comme Krita/
/// CSP), visibilité, opacité, sélection du calque actif, ajout/suppr.
/// Retourne true si un changement nécessite une recomposition pleine
/// image (visibilité/opacité/ajout/suppression — pas un trait de pinceau,
/// déjà géré en local par composite_rect_into).
fn layers_panel(ctx: &egui::Context, layers: &mut LayerStack) -> bool {
    let mut changed = false;

    egui::Window::new("Calques")
        .resizable(false)
        .default_pos([16.0, 320.0])
        .show(ctx, |ui| {
            if ui.button("+ Nouveau calque").clicked() {
                let n = layers.layers.len() + 1;
                layers.add_layer(&format!("Calque {n}"));
                changed = true;
            }

            ui.separator();

            // Du haut vers le bas dans l'UI = de la fin vers le début du Vec.
            for i in (0..layers.layers.len()).rev() {
                ui.horizontal(|ui| {
                    let active = layers.active == i;
                    if ui.selectable_label(active, &layers.layers[i].name).clicked() {
                        layers.set_active(i);
                    }
                    if ui.checkbox(&mut layers.layers[i].visible, "").changed() {
                        changed = true;
                    }
                    if layers.layers.len() > 1 && ui.small_button("🗑").clicked() {
                        layers.remove_layer(i);
                        changed = true;
                    }
                });
                if ui.add(egui::Slider::new(&mut layers.layers[i].opacity, 0.0..=1.0).text("opacité")).changed() {
                    changed = true;
                }
            }
        });

    changed
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
    total: std::time::Duration,
    max:   std::time::Duration,
}

impl Timing {
    fn add(&mut self, d: std::time::Duration) {
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
