//! The window's drawing: the canvas (aurastudio-gpu) with the interface
//! (egui) over it. egui and winit meet the GPU only here.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use aurastudio_gpu::{over_pass, Backend, CanvasQuality, GpuMemory, GpuState, TextureTraffic, ViewSnapshot};
use winit::window::Window;

pub struct Screen {
    gpu:          GpuState,
    window:       Arc<Window>,
    egui_ctx:     egui::Context,
    egui_state:   egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    /// Interface size chosen in the View menu (1.0 = the system's).
    ui_scale:     f32,
    /// egui's wish for the next frame, from the last render().
    repaint_delay: Option<std::time::Duration>,
    /// Cursor egui wants over its widgets, from the last UI frame.
    egui_cursor:  egui::CursorIcon,
    text_input_active: bool,
    /// Last UI primitives, redrawn as-is by render_cached_ui.
    last_ui:      Option<(Vec<egui::ClippedPrimitive>, f32)>,
    /// Textures egui dropped in the frame `last_ui` comes from: that frame
    /// may still draw them (a dialog closing draws its pictures one last
    /// time), and so may its cached redraws. Freed once a newer UI frame
    /// has been submitted.
    textures_to_free: Vec<egui::TextureId>,
    /// Bytes of each interface texture alive, and the traffic so far.
    egui_bytes:   HashMap<egui::TextureId, u64>,
    traffic:      TextureTraffic,
    /// Interface textures sent whole: name, size, private bytes the
    /// process gained meanwhile (the first 40; for the memory report).
    texture_costs: Vec<(String, u64, u64)>,
}

impl Deref for Screen {
    type Target = GpuState;
    fn deref(&self) -> &GpuState { &self.gpu }
}

impl DerefMut for Screen {
    fn deref_mut(&mut self) -> &mut GpuState { &mut self.gpu }
}

fn size_of(window: &Window) -> (u32, u32) {
    let s = window.inner_size();
    (s.width, s.height)
}

impl Screen {
    /// See `GpuState::new`.
    pub async fn new(window: Arc<Window>, canvas_size: (u32, u32), prefer: Backend, quality: CanvasQuality, vsync: bool) -> Result<Self, String> {
        let scale = window.scale_factor() as f32;
        let gpu = GpuState::new(Arc::clone(&window), size_of(&window), scale, canvas_size, prefer, quality, vsync).await?;
        let egui_ctx = egui::Context::default();
        // One pass per frame: the app's UI closure runs once (a second pass
        // egui asks for, to settle a size, would draw an empty UI); what a
        // discarded pass changes shows on the next frame.
        egui_ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN);
        let egui_state = egui_winit::State::new(egui_ctx.clone(), egui::ViewportId::ROOT, &window, Some(scale), None, None);
        let egui_renderer = egui_wgpu::Renderer::new(gpu.device(), gpu.format(), egui_wgpu::RendererOptions { msaa_samples: 1, dithering: false, ..Default::default() });
        crate::memory::mark("interface renderer");
        Ok(Self {
            gpu, window, egui_ctx, egui_state, egui_renderer,
            ui_scale: 1.0,
            repaint_delay: None,
            egui_cursor: egui::CursorIcon::Default,
            text_input_active: false,
            last_ui: None,
            textures_to_free: Vec::new(),
            egui_bytes: HashMap::new(),
            traffic: TextureTraffic::default(),
            texture_costs: Vec::new(),
        })
    }

    /// What the GPU holds, with the interface's textures.
    pub fn memory(&self) -> GpuMemory {
        let mut m = self.gpu.memory();
        m.traffic = self.traffic;
        let tex = self.egui_ctx.tex_manager();
        let tex = tex.read();
        for (_, meta) in tex.allocated() { m.egui.push((meta.name.clone(), meta.bytes_used() as u64)); }
        m.egui.sort_by_key(|x| std::cmp::Reverse(x.1));
        m.uploads = self.texture_costs.clone();
        m
    }

    /// An interface texture sent whole (new, or replacing one of the same id).
    fn note_made(&mut self, id: egui::TextureId, bytes: u64) {
        let t = &mut self.traffic;
        if let Some(old) = self.egui_bytes.insert(id, bytes) {
            t.freed += 1;
            t.freed_bytes += old;
            t.live_bytes = t.live_bytes.saturating_sub(old);
        }
        t.made += 1;
        t.made_bytes += bytes;
        t.live_bytes += bytes;
        t.peak_bytes = t.peak_bytes.max(t.live_bytes);
    }

    /// Interface textures egui dropped, freed (and counted).
    fn free_textures(&mut self, ids: &[egui::TextureId]) {
        for id in ids {
            self.egui_renderer.free_texture(id);
            if let Some(b) = self.egui_bytes.remove(id) {
                let t = &mut self.traffic;
                t.freed += 1;
                t.freed_bytes += b;
                t.live_bytes = t.live_bytes.saturating_sub(b);
            }
        }
    }

    /// Cursor egui wants over its widgets (from the last UI frame).
    pub fn egui_cursor(&self) -> egui::CursorIcon { self.egui_cursor }

    /// Size of the interface relative to the system's scaling.
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale;
        self.apply_ui_zoom();
    }

    /// The display scale the window is really at (see
    /// aurastudio_input::window_dpi; None: trust winit). When it differs
    /// from winit's, the interface is corrected by that ratio. The picture
    /// keeps its size on screen. Returns the new scale if it changed.
    pub fn set_dpi_scale(&mut self, real: Option<f32>) -> Option<f32> {
        let new = real.unwrap_or(self.winit_scale());
        if !self.gpu.set_dpi_scale(new) { return None; }
        self.apply_ui_zoom();
        Some(new)
    }

    /// Scale winit reports for the window (egui's base scale).
    pub fn winit_scale(&self) -> f32 { self.window.scale_factor() as f32 }

    /// egui's zoom: the chosen interface size, times the correction when
    /// winit's scale is stale.
    fn apply_ui_zoom(&self) {
        let fix = self.gpu.dpi_scale() / self.winit_scale();
        self.egui_ctx.set_zoom_factor(self.ui_scale * fix);
    }

    /// The current view, to place UI drawn over the canvas (selection
    /// outline...) in egui points.
    pub fn view_snapshot(&self) -> ViewSnapshot { self.gpu.view_snapshot(self.egui_ctx.pixels_per_point()) }

    /// True while egui holds the pointer (dragging a dock tab, a slider,
    /// a splitter...). It then needs a frame on every pointer move, even
    /// above the canvas, or the drag never completes.
    pub fn ui_is_dragging(&self) -> bool {
        self.egui_ctx.egui_is_using_pointer() || self.egui_ctx.dragged_id().is_some()
    }

    /// A text field is being edited (not wants_keyboard_input: a clicked
    /// button has focus too, which disabled Delete).
    pub fn ui_wants_keyboard(&self) -> bool { self.text_input_active }

    /// When egui wants the next frame (zero = right away), consumed once.
    pub fn take_repaint_delay(&mut self) -> Option<std::time::Duration> { self.repaint_delay.take() }

    /// Feed a window event to egui. Returns true when egui needs a new
    /// frame for it (hover change, click, typing...). Whether a press
    /// starts a stroke is decided by is_canvas_at, not by egui.
    pub fn egui_handle_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).repaint
    }

    /// A press at `pos` starts a stroke: over the canvas, under no panel
    /// (egui's own flag is a frame late here).
    pub fn is_canvas_at(&self, pos: (f32, f32)) -> bool {
        let ppp = self.egui_ctx.pixels_per_point();
        // Floating windows over the canvas (not egui's background layer).
        let over_ui = self.egui_ctx
            .layer_id_at(egui::pos2(pos.0 / ppp, pos.1 / ppp))
            .is_some_and(|layer| layer.order != egui::Order::Background);
        self.gpu.in_viewport(pos) && !over_ui
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.gpu.resize((size.width, size.height));
    }

    /// Draw a frame. egui runs first to know how much room the docks take;
    /// the canvas is then drawn in the remaining area, and the UI on top.
    pub fn render(&mut self, ui_fn: impl FnOnce(&mut egui::Ui)) {
        self.render_impl(Some(ui_fn));
    }

    /// Draw a frame reusing the last UI as it was, without running it.
    /// Used mid-stroke when the document is busy being painted: the canvas
    /// still updates, and the UI thread never waits for the brush.
    pub fn render_cached_ui(&mut self) {
        self.render_impl(None::<fn(&mut egui::Ui)>);
    }

    /// A UI frame exists to reuse (see render_cached_ui).
    pub fn has_cached_ui(&self) -> bool { self.last_ui.is_some() }

    fn render_impl(&mut self, ui_fn: Option<impl FnOnce(&mut egui::Ui)>) {
        // Minimized: nothing to show (frames are drawn on demand, not only
        // on the window system's request).
        if self.window.is_minimized() == Some(true) { return; }
        let mut textures_free = Vec::new();
        let (tris, ppp) = match ui_fn {
            Some(f) => {
                let raw_input = self.egui_state.take_egui_input(&self.window);
                let mut ui_fn = Some(f);
                let mut free_rect = None;
                let mut full_output = self.egui_ctx.run_ui(raw_input, |ui| {
                    if let Some(f) = ui_fn.take() { f(ui); }
                    // The canvas card the UI laid out, or what's left of
                    // the window.
                    free_rect = Some(ui.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("aurastudio_canvas_rect")))
                        .unwrap_or_else(|| ui.available_rect_before_wrap()));
                });
                // When egui wants its next frame (the app schedules it).
                self.repaint_delay = full_output.viewport_output.get(&egui::ViewportId::ROOT)
                    .map(|v| v.repaint_delay);

                // egui asks for IME (text input) only while a text field is focused.
                self.text_input_active = full_output.platform_output.ime.is_some();
                // The app picks the cursor: egui's wish is kept for it.
                let mut platform_output = std::mem::take(&mut full_output.platform_output);
                self.egui_cursor = std::mem::replace(&mut platform_output.cursor_icon, egui::CursorIcon::Default);
                self.egui_state.handle_platform_output(&self.window, platform_output);

                let ppp = full_output.pixels_per_point;
                // The canvas area goes right up to the docks (a zoomed-in
                // canvas fills it); the fit margin is applied by the view.
                if let Some(r) = free_rect {
                    let (ww, wh) = self.gpu.surface_size();
                    let (ww, wh) = (ww as f32, wh as f32);
                    let x0 = (r.min.x * ppp).round().clamp(0.0, ww);
                    let y0 = (r.min.y * ppp).round().clamp(0.0, wh);
                    let x1 = (r.max.x * ppp).round().clamp(x0, ww);
                    let y1 = (r.max.y * ppp).round().clamp(y0, wh);
                    self.gpu.set_viewport([x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)]);
                }

                let tris = self.egui_ctx.tessellate(std::mem::take(&mut full_output.shapes), ppp);
                let set = std::mem::take(&mut full_output.textures_delta.set);
                let freed = std::mem::take(&mut full_output.textures_delta.free);
                for (id, delta) in set.iter().flat_map(|(id, ds)| ds.iter().map(move |d| (id, d))) {
                    // A texture sent whole: what it costs the process (the
                    // driver may take its share later, at the submit).
                    let probe = aurastudio_gpu::MEMORY_PROBE.get().filter(|_| delta.pos.is_none() && self.texture_costs.len() < 40);
                    let before = probe.map(|f| f());
                    self.egui_renderer.update_texture(self.gpu.device(), self.gpu.queue(), *id, delta);
                    if delta.pos.is_none() {
                        let [w, h] = delta.image.size();
                        self.note_made(*id, w as u64 * h as u64 * 4);
                    }
                    if let (Some(f), Some(before)) = (probe, before) {
                        let name = self.egui_ctx.tex_manager().read().meta(*id).map_or_else(|| format!("{id:?}"), |m| m.name.clone());
                        let [w, h] = delta.image.size();
                        self.texture_costs.push((format!("{name} {w}x{h}"), w as u64 * h as u64 * 4, f().saturating_sub(before)));
                    }
                }
                // Free what the previous frame dropped (this frame no longer
                // draws it); keep what this one dropped for later.
                textures_free = std::mem::replace(&mut self.textures_to_free, freed.into_iter().collect());
                (tris, ppp)
            }
            None => match self.last_ui.take() {
                Some(ui) => ui,
                None => return,
            },
        };

        let Some(mut frame) = self.gpu.begin_frame() else {
            // No frame this time: what the previous frame dropped can go
            // all the same (this one's waits).
            self.free_textures(&textures_free);
            self.last_ui = Some((tris, ppp));
            return;
        };

        // UI (docks) over the canvas.
        let (w, h) = self.gpu.surface_size();
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: ppp };
        self.egui_renderer.update_buffers(self.gpu.device(), self.gpu.queue(), &mut frame.encoder, &tris, &screen);
        {
            let pass = over_pass(&mut frame.encoder, &frame.view, "egui_pass");
            self.egui_renderer.render(&mut pass.forget_lifetime(), &tris, &screen);
        }
        self.last_ui = Some((tris, ppp));

        self.gpu.end_frame(frame, size_of(&self.window));
        // (After the submit: wgpu refuses a submit using a texture already
        // destroyed.)
        self.free_textures(&textures_free);
    }
}
