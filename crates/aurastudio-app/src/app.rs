use std::sync::Arc;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::{
    application::ApplicationHandler,
    event::{MouseButton, WindowEvent},
    event_loop::ActiveEventLoop,
    window::{Window, WindowId},
};
use aurastudio_brush::{stroke_line, Brush};
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
    last_pos:     Option<(f32, f32)>,
    pressure:     f32,
    stylus:       Option<Stylus>,
    /// true quand un changement hors-trait (visibilité, opacité, ajout/
    /// suppression de calque) demande une recomposition pleine image,
    /// plutôt que le composite_rect_into local fait pendant un trait.
    layers_dirty: bool,
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
            pressure:     1.0,
            stylus:       None,
            layers_dirty: false,
        }
    }
}

impl App {
    fn window_to_canvas(&self, wx: f32, wy: f32) -> Option<(f32, f32)> {
        let gpu = self.gpu.as_ref()?;
        let (ww, wh) = gpu.window_size();
        let cw = gpu.canvas.width  as f32;
        let ch = gpu.canvas.height as f32;

        let scale    = (ww / cw).min(wh / ch);
        let offset_x = (ww - cw * scale) / 2.0;
        let offset_y = (wh - ch * scale) / 2.0;

        Some(((wx - offset_x) / scale, (wy - offset_y) / scale))
    }

    fn begin_stroke(&mut self) {
        self.layers.active_layer_mut().begin_stroke();
        self.last_pos = None;
    }

    /// Peint sur le calque actif, puis recompose uniquement le
    /// rectangle touché (pas tout le canvas) dans le buffer GPU avant
    /// de marquer la zone dirty pour upload — garde le même coût que
    /// l'ancien chemin direct-sur-Canvas malgré l'ajout des calques.
    fn draw_at(&mut self, wx: f32, wy: f32) {
        if let Some(canvas_pos) = self.window_to_canvas(wx, wy) {
            let pressure = self.pressure;
            let from     = self.last_pos.unwrap_or(canvas_pos);
            let layer    = self.layers.active_layer_mut();

            if let Some(rect) = stroke_line(from, canvas_pos, &self.brush, layer, pressure) {
                let gpu = self.gpu.as_mut().unwrap();
                self.layers.composite_rect_into(&mut gpu.canvas.pixels, rect);
                gpu.canvas.mark_dirty(rect);
            }
            self.last_pos = Some(canvas_pos);

            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
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
                self.pressure = s.pressure;
                self.draw_at(s.x, s.y);
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("AuraStudio v0.1")
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 800u32));

        let window = Arc::new(event_loop.create_window(attrs).unwrap());
        let gpu    = pollster::block_on(GpuState::new(Arc::clone(&window)));

        if let Ok(handle) = window.window_handle() {
            if let RawWindowHandle::Win32(h) = handle.as_raw() {
                self.stylus = Some(Stylus::install(h.hwnd.get()));
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
        // egui a priorité : si l'event concerne un widget (clic sur un
        // slider du color picker, etc.), on ne dessine pas sur le canvas.
        let consumed_by_ui = self.gpu.as_mut()
            .map(|gpu| gpu.egui_handle_event(&event))
            .unwrap_or(false);

        // egui a besoin d'un redraw à chaque interaction avec un widget
        // (hover, drag de slider...) — sans ça l'UI resterait figée sous
        // ControlFlow::Wait tant qu'aucun autre event ne force une frame.
        if consumed_by_ui {
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

            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } if !consumed_by_ui => {
                self.is_drawing = state.is_pressed();
                self.pressure   = 1.0;
                if self.is_drawing {
                    self.begin_stroke();
                } else {
                    self.last_pos = None;
                }
            }

            WindowEvent::CursorMoved { position, .. } if !consumed_by_ui => {
                if self.is_drawing {
                    self.draw_at(position.x as f32, position.y as f32);
                }
            }

            // Fallback macOS/Linux uniquement : sous Windows, drain_stylus
            // (efude-input) gère déjà tout, donc on ignore Touch ici pour
            // ne pas dessiner en double.
            WindowEvent::Touch(touch) if cfg!(not(target_os = "windows")) && !consumed_by_ui => {
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
                        self.last_pos   = None;
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                if let Some(gpu) = &mut self.gpu {
                    let brush  = &mut self.brush;
                    let layers = &mut self.layers;
                    let mut changed = false;
                    gpu.render(|ctx| {
                        color_picker_panel(ctx, brush);
                        changed = layers_panel(ctx, layers);
                    });

                    if changed || self.layers_dirty {
                        self.layers.composite_into(&mut gpu.canvas.pixels);
                        gpu.canvas.mark_dirty((0, 0, CANVAS_SIZE, CANVAS_SIZE));
                        self.layers_dirty = false;
                    }
                }
            }

            _ => {}
        }
    }
}

/// Panneau egui : carré de preview + sliders RGB et HSV, synchronisés
/// avec la couleur active du brush. Volontairement minimal (pas encore
/// de calques/palette) — brique color picker v1.
fn color_picker_panel(ctx: &egui::Context, brush: &mut Brush) {
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
            ui.add(egui::Slider::new(&mut brush.opacity, 0.0..=1.0).text("Opacité"));
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
