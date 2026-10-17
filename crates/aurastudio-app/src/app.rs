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

pub struct App {
    window:     Option<Arc<Window>>,
    gpu:        Option<GpuState>,
    brush:      Brush,
    is_drawing: bool,
    last_pos:   Option<(f32, f32)>,
    pressure:   f32,
    stylus:     Option<Stylus>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            window:     None,
            gpu:        None,
            brush:      Brush::new(),
            is_drawing: false,
            last_pos:   None,
            pressure:   1.0,
            stylus:     None,
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
        if let Some(gpu) = &mut self.gpu {
            gpu.canvas.begin_stroke();
        }
        self.last_pos = None;
    }

    fn draw_at(&mut self, wx: f32, wy: f32) {
        if let Some(canvas_pos) = self.window_to_canvas(wx, wy) {
            let gpu      = self.gpu.as_mut().unwrap();
            let pressure = self.pressure;
            let from     = self.last_pos.unwrap_or(canvas_pos);

            if let Some(rect) = stroke_line(from, canvas_pos, &self.brush, &mut gpu.canvas, pressure) {
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
                    let brush = &mut self.brush;
                    gpu.render(|ctx| color_picker_panel(ctx, brush));
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
