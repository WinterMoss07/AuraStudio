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

            WindowEvent::MouseInput { button: MouseButton::Left, state, .. } => {
                self.is_drawing = state.is_pressed();
                self.pressure   = 1.0;
                if self.is_drawing {
                    self.begin_stroke();
                } else {
                    self.last_pos = None;
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if self.is_drawing {
                    self.draw_at(position.x as f32, position.y as f32);
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
                    gpu.render();
                }
            }

            _ => {}
        }
    }
}
