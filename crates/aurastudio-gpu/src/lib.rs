mod view;
pub use view::{wrap_angle, CanvasView};

use std::sync::Arc;
use wgpu::*;
use wgpu::util::DeviceExt;
use winit::window::Window;
use aurastudio_canvas::Canvas;

/// From this zoom up, canvas pixels are shown as sharp squares (nearest
/// sampling) instead of being blurred, like Krita / CSP close up.
const NEAREST_FROM_ZOOM: f32 = 2.0;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    m0:          [f32; 4],
    m1:          [f32; 4],
    canvas_size: [f32; 2],
    _pad:        [f32; 2],
    /// x: display color management on (1) / off (0), y: LUT size.
    cm:          [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CursorUniform {
    center: [f32; 2],
    radius: f32,
    cross:  f32,
    screen: [f32; 2],
    _pad:   [f32; 2],
}

/// Below this outline radius (screen pixels) a crosshair is shown instead.
const MIN_OUTLINE_RADIUS: f32 = 3.0;

pub struct GpuState {
    surface:         Surface<'static>,
    device:          Device,
    queue:           Queue,
    config:          SurfaceConfiguration,
    /// Set after rebuilding a suboptimal swapchain, cleared once presents
    /// are optimal again.
    swapchain_rebuilt: bool,
    /// egui's wish for the next frame, from the last render().
    repaint_delay:   Option<std::time::Duration>,
    /// Scale of the monitor the window is on (Windows display scaling:
    /// 2.0 at 200 %), as of the last check.
    dpi_scale:       f32,
    /// 100 % zoom = one canvas pixel per physical screen pixel (XnView's
    /// "1 image pixel = 1 screen pixel"); off: per point, so a picture
    /// keeps the same size on a 4K and an HD screen.
    pixel_exact:     bool,
    /// Interface size chosen in the View menu (1.0 = the system's).
    ui_scale:        f32,
    canvas_pipeline: RenderPipeline,
    view_buffer:     Buffer,
    nearest_sampler: Sampler,
    /// Same as view_bind_group with nearest sampling, for strong zoom.
    nearest_bind_group: BindGroup,
    /// Zoom / rotation / pan of the canvas.
    pub view:        CanvasView,
    /// sRGB -> monitor lookup table, applied to the canvas when
    /// color_managed is on.
    lut:             DisplayLut,
    color_managed:   bool,
    /// Brush outline: pipeline, uniform, and where to draw it (center in
    /// physical pixels, radius in screen pixels); None = hidden.
    cursor_pipeline: RenderPipeline,
    cursor_buffer:   Buffer,
    cursor_bind:     BindGroup,
    brush_cursor:    Option<((f32, f32), f32)>,
    /// Cursor egui wants over its widgets, from the last UI frame.
    egui_cursor:     egui::CursorIcon,
    /// Last UI primitives, redrawn as-is by render_cached_ui.
    last_ui:         Option<(Vec<egui::ClippedPrimitive>, f32)>,
    view_bind_group: BindGroup,
    canvas_bgl:      BindGroupLayout,
    pub canvas:      Canvas,
    window:          Arc<Window>,
    egui_ctx:        egui::Context,
    egui_state:      egui_winit::State,
    egui_renderer:   egui_wgpu::Renderer,
    /// Window area (physical pixels) where the canvas is drawn: all the
    /// space not covered by docks. (x, y, width, height)
    viewport:        [f32; 4],
    text_input_active: bool,
    /// Backdrop color around the canvas (sRGB, 0..1).
    pub clear_color: [f64; 3],
}

impl GpuState {
    /// `canvas_size`: initial canvas width and height in pixels.
    pub async fn new(window: Arc<Window>, canvas_size: (u32, u32)) -> Self {
        let size = window.inner_size();

        // Vulkan by default (lowest RAM in our tests), Metal on macOS.
        // AURASTUDIO_BACKEND=dx12|vulkan|metal|gl overrides it, to compare
        // latency per GPU driver without rebuilding.
        let backend = match std::env::var("AURASTUDIO_BACKEND").ok().as_deref() {
            Some("dx12")   => Backends::DX12,
            Some("vulkan") => Backends::VULKAN,
            Some("metal")  => Backends::METAL,
            Some("gl")     => Backends::GL,
            _ if cfg!(target_os = "macos") => Backends::METAL,
            _ => Backends::VULKAN,
        };

        let instance = Instance::new(InstanceDescriptor {
            backends: backend,
            ..Default::default()
        });

        let surface = instance.create_surface(Arc::clone(&window)).unwrap();

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference:       PowerPreference::HighPerformance,
                compatible_surface:     Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("No GPU adapter found");

        let (device, queue) = adapter
            .request_device(
                &DeviceDescriptor {
                    label:             None,
                    required_features: Features::empty(),
                    // Ask for the biggest texture the GPU supports (often
                    // 16384) instead of wgpu's portable default (8192): it
                    // caps the canvas size.
                    required_limits:   Limits {
                        max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                        ..Limits::default()
                    },
                    // Small GPU memory blocks: the default (Performance)
                    // reserves 128 MB chunks at a time on Vulkan, for a few
                    // MB of staging and UI buffers.
                    memory_hints:      MemoryHints::MemoryUsage,
                },
                None,
            )
            .await
            .expect("Failed to create device");

        let caps   = surface.get_capabilities(&adapter);
        // Non-sRGB surface: canvas pixels are already sRGB-encoded bytes and
        // must reach the screen unchanged. With an sRGB surface the GPU
        // encoded them a second time: everything looked lighter, mid-tones
        // were compressed and gradients banded. egui also expects this.
        let format = caps.formats.iter().copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        // Latency, best first:
        // - Mailbox: newest frame shown at the next vsync, never queued, no
        //   tearing. Not offered by every driver (Intel Vulkan on Windows
        //   only has Immediate + Fifo).
        // - Immediate: presented right away, no wait for vsync. In a normal
        //   window the Windows compositor (DWM) still syncs to the display,
        //   so it doesn't tear there.
        // - Fifo: classic vsync. Rendering blocks until the next refresh, and
        //   pen samples arriving meanwhile wait one more frame: measured as
        //   ~14 ms per frame on an Intel Arc 140T, the main remaining lag.
        // Rendering only happens on demand (ControlFlow::Wait), so neither
        // Mailbox nor Immediate makes the GPU spin when idle.
        // AURASTUDIO_PRESENT=mailbox|immediate|fifo overrides the choice.
        let wanted = match std::env::var("AURASTUDIO_PRESENT").ok().as_deref() {
            Some("mailbox")   => vec![PresentMode::Mailbox],
            Some("immediate") => vec![PresentMode::Immediate],
            Some("fifo")      => vec![PresentMode::Fifo],
            _ => vec![PresentMode::Mailbox, PresentMode::Immediate],
        };
        let present_mode = wanted.into_iter()
            .find(|m| caps.present_modes.contains(m))
            .unwrap_or(PresentMode::Fifo);
        log::info!("surface: {format:?}, present mode {present_mode:?} (available: {:?}), backend {:?}",
            caps.present_modes, adapter.get_info().backend);

        let config = SurfaceConfiguration {
            usage:                         TextureUsages::RENDER_ATTACHMENT,
            format,
            width:                         size.width.max(1),
            height:                        size.height.max(1),
            present_mode,
            alpha_mode:                    caps.alpha_modes[0],
            view_formats:                  vec![],
            // At most 1 frame in flight: a new stroke sample is never stuck
            // behind an older frame waiting to be shown.
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let canvas_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("canvas_bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding:    0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled:   false,
                        view_dimension: TextureViewDimension::D2,
                        sample_type:    TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    2,
                    visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer {
                        ty:                 BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size:   None,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    3,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        multisampled:   false,
                        view_dimension: TextureViewDimension::D3,
                        sample_type:    TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding:    4,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // Display LUT: identity (2^3 corners) until the app sets the real
        // one, and switched off anyway.
        let lut = DisplayLut::new(&device, &queue, &IDENTITY_LUT, 2);

        let view_uniform = ViewUniform {
            m0: [0.0; 4],
            m1: [0.0; 4],
            canvas_size: [canvas_size.0 as f32, canvas_size.1 as f32],
            _pad: [0.0; 2],
            cm: [0.0; 4],
        };

        let view_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("view_buffer"),
            contents: bytemuck::cast_slice(&[view_uniform]),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("canvas_shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/canvas.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label:                Some("canvas_pipeline_layout"),
            bind_group_layouts:   &[&canvas_bgl],
            push_constant_ranges: &[],
        });

        let canvas_pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:  Some("canvas_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module:              &shader,
                entry_point:         Some("vs_main"),
                buffers:             &[],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(FragmentState {
                module:              &shader,
                entry_point:         Some("fs_main"),
                targets:             &[Some(ColorTargetState {
                    format,
                    blend:      Some(BlendState::REPLACE),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive:     PrimitiveState::default(),
            depth_stencil: None,
            multisample:   MultisampleState::default(),
            multiview:     None,
            cache:         None,
        });

        let (cursor_pipeline, cursor_buffer, cursor_bind) = cursor_pipeline(&device, format);
        let canvas = Canvas::new(&device, canvas_size.0, canvas_size.1);
        let nearest_sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("canvas_sampler_nearest"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Nearest,
            min_filter:     FilterMode::Linear,
            ..Default::default()
        });
        let view_bind_group = canvas_bind_group(&device, &canvas_bgl, &canvas, &canvas.sampler, &view_buffer, &lut);
        let nearest_bind_group = canvas_bind_group(&device, &canvas_bgl, &canvas, &nearest_sampler, &view_buffer, &lut);

        let dpi_scale = window.scale_factor() as f32;
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

        Self {
            surface, device, queue, config,
            canvas_pipeline, view_buffer, view_bind_group, canvas_bgl,
            nearest_sampler, nearest_bind_group,
            view: CanvasView::default(),
            last_ui: None,
            cursor_pipeline, cursor_buffer, cursor_bind,
            brush_cursor: None,
            egui_cursor: egui::CursorIcon::Default,
            lut,
            color_managed: false,
            canvas, window,
            egui_ctx, egui_state, egui_renderer,
            viewport: [0.0, 0.0, size.width.max(1) as f32, size.height.max(1) as f32],
            text_input_active: false,
            swapchain_rebuilt: false,
            repaint_delay: None,
            clear_color: [0.4, 0.4, 0.4],
            dpi_scale,
            pixel_exact: false,
            ui_scale: 1.0,
        }
    }

    /// Biggest canvas side the GPU accepts (max 2D texture size).
    pub fn max_canvas_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Replace the canvas texture with a new size. The caller then uploads
    /// the new composite with upload_rect.
    pub fn resize_canvas(&mut self, width: u32, height: u32) {
        self.canvas = Canvas::new(&self.device, width, height);
        self.rebuild_bind_groups();
        // New canvas: show it whole.
        self.view.fit_to_area();
    }

    fn rebuild_bind_groups(&mut self) {
        self.view_bind_group = canvas_bind_group(&self.device, &self.canvas_bgl, &self.canvas, &self.canvas.sampler, &self.view_buffer, &self.lut);
        self.nearest_bind_group = canvas_bind_group(&self.device, &self.canvas_bgl, &self.canvas, &self.nearest_sampler, &self.view_buffer, &self.lut);
    }

    /// Display color management of the canvas: `lut` is a size^3 RGBA
    /// table (red fastest) from aurastudio-color, None switches it off.
    pub fn set_display_lut(&mut self, lut: Option<(&[[f32; 4]], usize)>) {
        match lut {
            Some((data, size)) if size >= 2 && data.len() == size * size * size => {
                self.lut = DisplayLut::new(&self.device, &self.queue, data, size as u32);
                self.rebuild_bind_groups();
                self.color_managed = true;
            }
            _ => self.color_managed = false,
        }
    }

    /// Cursor egui wants over its widgets (from the last UI frame).
    pub fn egui_cursor(&self) -> egui::CursorIcon {
        self.egui_cursor
    }

    /// Show the brush outline at `center` (physical window pixels) with
    /// `radius` in screen pixels, or hide it.
    pub fn set_brush_cursor(&mut self, cursor: Option<((f32, f32), f32)>) {
        self.brush_cursor = cursor;
    }

    /// Physical screen pixels per canvas pixel at 100 % zoom.
    pub fn zoom_unit(&self) -> f32 {
        if self.pixel_exact { 1.0 } else { self.dpi_scale }
    }

    /// The zoom as shown to the user (1.0 = 100 %, see zoom_unit).
    pub fn display_zoom(&self) -> f32 {
        self.view.zoom / self.zoom_unit()
    }

    /// Switch "1 image pixel = 1 screen pixel"; the zoom percentage stays,
    /// so the picture changes size on a high-density screen.
    pub fn set_pixel_exact(&mut self, on: bool) {
        if on == self.pixel_exact { return; }
        let shown = self.display_zoom();
        self.pixel_exact = on;
        let (unit, c) = (self.zoom_unit(), self.view_center());
        if !self.view.fit { self.view_set_zoom(shown * unit, c); }
    }

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
        let new = real.unwrap_or(self.window.scale_factor() as f32);
        if (new - self.dpi_scale).abs() < 1e-3 { return None; }
        let k = new / self.dpi_scale;
        self.dpi_scale = new;
        if !self.pixel_exact && !self.view.fit {
            self.view.zoom = (self.view.zoom * k).clamp(view::MIN_ZOOM, view::MAX_ZOOM);
            self.view.pan = (self.view.pan.0 * k, self.view.pan.1 * k);
        }
        self.apply_ui_zoom();
        Some(new)
    }

    /// Scale winit reports for the window (egui's base scale).
    pub fn winit_scale(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    /// egui's zoom: the chosen interface size, times the correction when
    /// winit's scale is stale.
    fn apply_ui_zoom(&self) {
        let fix = self.dpi_scale / self.winit_scale();
        self.egui_ctx.set_zoom_factor(self.ui_scale * fix);
    }

    /// Current zoom (screen pixels per canvas pixel).
    pub fn zoom(&self) -> f32 {
        self.view.zoom
    }

    fn canvas_size(&self) -> (u32, u32) {
        (self.canvas.width, self.canvas.height)
    }

    /// Window position (physical pixels) -> canvas pixel, through the
    /// current zoom / rotation / pan. Not clamped: off-canvas is fine.
    pub fn screen_to_canvas(&self, pos: (f32, f32)) -> (f32, f32) {
        self.view.screen_to_canvas(pos, self.canvas_size(), self.viewport)
    }

    /// The current view, to place UI drawn over the canvas (selection
    /// outline...) in egui points.
    pub fn view_snapshot(&self) -> ViewSnapshot {
        ViewSnapshot { view: self.view, canvas: self.canvas_size(), area: self.viewport, ppp: self.egui_ctx.pixels_per_point() }
    }

    /// Center of the canvas area (pivot for keyboard zoom / rotation).
    pub fn view_center(&self) -> (f32, f32) {
        let [x, y, w, h] = self.viewport;
        (x + w / 2.0, y + h / 2.0)
    }

    /// Pivot for turning the view (see CanvasView::rotation_pivot).
    pub fn rotation_pivot(&self) -> (f32, f32) {
        self.view.rotation_pivot(self.canvas_size(), self.viewport)
    }

    pub fn view_zoom_at(&mut self, factor: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.zoom_at(factor, pivot, c, a);
    }

    pub fn view_set_zoom(&mut self, zoom: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.set_zoom(zoom, pivot, c, a);
    }

    pub fn view_rotate_at(&mut self, delta: f32, pivot: (f32, f32)) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.rotate_at(delta, pivot, c, a);
    }

    pub fn view_reset_rotation(&mut self) {
        let (c, a, pivot) = (self.canvas_size(), self.viewport, self.rotation_pivot());
        let back = -self.view.angle;
        self.view.rotate_at(back, pivot, c, a);
    }

    /// True while egui holds the pointer (dragging a dock tab, a slider,
    /// a splitter...). It then needs a frame on every pointer move, even
    /// above the canvas, or the drag never completes.
    pub fn ui_is_dragging(&self) -> bool {
        self.egui_ctx.is_using_pointer() || self.egui_ctx.dragged_id().is_some()
    }

    /// True while an egui text/number field is being edited: shortcuts like
    /// Delete must then go to the field, not the canvas.
    ///
    /// Not egui's wants_keyboard_input(): that is true whenever ANY widget
    /// has focus, including a button just clicked (e.g. the delete-layer
    /// button), which silently disabled the Delete shortcut.
    pub fn ui_wants_keyboard(&self) -> bool {
        self.text_input_active
    }

    /// When egui wants the next frame (zero = right away), consumed once.
    pub fn take_repaint_delay(&mut self) -> Option<std::time::Duration> {
        self.repaint_delay.take()
    }

    /// Feed a window event to egui. Returns true when egui needs a new
    /// frame for it (hover change, click, typing...). Whether a press
    /// starts a stroke is decided by is_canvas_at, not by egui.
    pub fn egui_handle_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.egui_state.on_window_event(&self.window, event).repaint
    }

    /// True if a press at `pos` (physical window pixels) should start a
    /// stroke: inside the canvas area and not under any egui panel.
    ///
    /// We don't rely on egui's own "consumed" flag for this: it is based on
    /// the pointer position of egui's *last frame*, which is stale because
    /// we only redraw on demand (ControlFlow::Wait), so it rejected clicks
    /// made on the canvas.
    pub fn is_canvas_at(&self, pos: (f32, f32)) -> bool {
        let [vx, vy, vw, vh] = self.viewport;
        let inside = pos.0 >= vx && pos.0 < vx + vw && pos.1 >= vy && pos.1 < vy + vh;
        let ppp = self.egui_ctx.pixels_per_point();
        // Docked panels are already outside the viewport. Here we only look
        // for floating windows/popups above the canvas: egui keeps a
        // full-window Background-order layer, which must not count.
        let over_ui = self.egui_ctx
            .layer_id_at(egui::pos2(pos.0 / ppp, pos.1 / ppp))
            .is_some_and(|layer| layer.order != egui::Order::Background);
        inside && !over_ui
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 { return; }
        self.config.width  = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.device, &self.config);
        // Recomputed from the egui layout on the next frame; until then,
        // the whole window.
        self.viewport = [0.0, 0.0, size.width as f32, size.height as f32];
    }

    /// Upload the view transform and color management switch for this
    /// frame (64 bytes).
    fn write_view_uniform(&mut self) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.update_fit(c, a);
        let [m0, m1] = self.view.ndc_matrix(c, a);
        let cm = [if self.color_managed { 1.0 } else { 0.0 }, self.lut.size as f32, 0.0, 0.0];
        let uniform = ViewUniform { m0, m1, canvas_size: [c.0 as f32, c.1 as f32], _pad: [0.0; 2], cm };
        self.queue.write_buffer(&self.view_buffer, 0, bytemuck::cast_slice(&[uniform]));
    }

    /// Copy merged pixels (`rgba`: the rows of `rect`, tightly packed) to
    /// the canvas texture.
    pub fn upload(&mut self, rect: aurastudio_canvas::Rect, rgba: &[u8]) {
        let (x0, y0, x1, y1) = rect;
        if x1 <= x0 || y1 <= y0 || x1 > self.canvas.width || y1 > self.canvas.height { return; }
        let w = x1 - x0;
        if rgba.len() != w as usize * (y1 - y0) as usize * 4 { return; }
        self.queue.write_texture(
            ImageCopyTexture {
                texture:   &self.canvas.texture,
                mip_level: 0,
                origin:    Origin3d { x: x0, y: y0, z: 0 },
                aspect:    TextureAspect::All,
            },
            rgba,
            ImageDataLayout {
                offset:         0,
                bytes_per_row:  Some(w * 4),
                rows_per_image: Some(y1 - y0),
            },
            Extent3d { width: x1 - x0, height: y1 - y0, depth_or_array_layers: 1 },
        );
    }

    /// Draw a frame. egui runs first to know how much room the docks take;
    /// the canvas is then drawn in the remaining area, and the UI on top.
    pub fn render(&mut self, ui_fn: impl FnOnce(&egui::Context)) {
        self.render_impl(Some(ui_fn));
    }

    /// Draw a frame reusing the last UI as it was, without running it.
    /// Used mid-stroke when the document is busy being painted: the canvas
    /// still updates, and the UI thread never waits for the brush.
    pub fn render_cached_ui(&mut self) {
        self.render_impl(None::<fn(&egui::Context)>);
    }

    /// A UI frame exists to reuse (see render_cached_ui).
    pub fn has_cached_ui(&self) -> bool {
        self.last_ui.is_some()
    }

    fn render_impl(&mut self, ui_fn: Option<impl FnOnce(&egui::Context)>) {
        let mut textures_free = Vec::new();
        let (tris, ppp) = match ui_fn {
            Some(f) => {
                let raw_input = self.egui_state.take_egui_input(&self.window);
                let mut ui_fn = Some(f);
                let mut free_rect = None;
                let full_output = self.egui_ctx.run(raw_input, |ctx| {
                    if let Some(f) = ui_fn.take() { f(ctx); }
                    // The canvas card the UI laid out, or what's left of
                    // the window.
                    free_rect = Some(ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("aurastudio_canvas_rect")))
                        .unwrap_or_else(|| ctx.available_rect()));
                });
                // When egui wants its next frame: right away (layout not
                // settled, a click changed something) or later (tooltip,
                // fading notice). The app schedules it; requesting a redraw
                // from inside a redraw is dropped by winit.
                self.repaint_delay = full_output.viewport_output.get(&egui::ViewportId::ROOT)
                    .map(|v| v.repaint_delay);

                // egui asks for IME (text input) only while a text field is focused.
                self.text_input_active = full_output.platform_output.ime.is_some();
                // The app picks the cursor (brush outline, hands): keep egui's
                // suggestion for it, and let egui itself always ask for the
                // same one, so it never touches the window cursor again.
                let mut platform_output = full_output.platform_output;
                self.egui_cursor = std::mem::replace(&mut platform_output.cursor_icon, egui::CursorIcon::Default);
                self.egui_state.handle_platform_output(&self.window, platform_output);

                let ppp = full_output.pixels_per_point;
                // The canvas area goes right up to the docks (a zoomed-in
                // canvas fills it); the fit margin is applied by the view.
                if let Some(r) = free_rect {
                    let (ww, wh) = (self.config.width as f32, self.config.height as f32);
                    let x0 = (r.min.x * ppp).round().clamp(0.0, ww);
                    let y0 = (r.min.y * ppp).round().clamp(0.0, wh);
                    let x1 = (r.max.x * ppp).round().clamp(x0, ww);
                    let y1 = (r.max.y * ppp).round().clamp(y0, wh);
                    self.viewport = [x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)];
                }

                let tris = self.egui_ctx.tessellate(full_output.shapes, ppp);
                for (id, delta) in &full_output.textures_delta.set {
                    self.egui_renderer.update_texture(&self.device, &self.queue, *id, delta);
                }
                textures_free = full_output.textures_delta.free;
                (tris, ppp)
            }
            None => match self.last_ui.take() {
                Some(ui) => ui,
                None => return,
            },
        };

        self.write_view_uniform();

        let output = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(e) => {
                if matches!(e, SurfaceError::Lost | SurfaceError::Outdated) {
                    self.surface.configure(&self.device, &self.config);
                } else {
                    log::error!("Surface error: {e:?}");
                }
                // No frame this time: textures egui dropped are still freed.
                for id in &textures_free {
                    self.egui_renderer.free_texture(id);
                }
                self.last_ui = Some((tris, ppp));
                return;
            }
        };

        let view = output.texture.create_view(&TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(
            &CommandEncoderDescriptor { label: Some("frame") }
        );

        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("canvas_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops: Operations {
                        // Backdrop around the canvas (set by the UI theme).
                        load:  LoadOp::Clear(Color {
                            r: self.clear_color[0], g: self.clear_color[1], b: self.clear_color[2], a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
            });

            let [vx, vy, vw, vh] = self.viewport;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            pass.set_pipeline(&self.canvas_pipeline);
            let bind = if self.view.zoom >= NEAREST_FROM_ZOOM { &self.nearest_bind_group } else { &self.view_bind_group };
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        }

        // UI (docks) over the canvas.
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels:    [self.config.width, self.config.height],
            pixels_per_point:  ppp,
        };
        self.egui_renderer.update_buffers(&self.device, &self.queue, &mut enc, &tris, &screen_descriptor);

        {
            let pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("egui_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
            });
            self.egui_renderer.render(&mut pass.forget_lifetime(), &tris, &screen_descriptor);
        }

        // Brush outline, over everything (it follows the pen also while
        // the UI is shown from cache mid-stroke).
        if let Some(((cx, cy), r)) = self.brush_cursor {
            let u = CursorUniform {
                center: [cx, cy],
                radius: r.max(MIN_OUTLINE_RADIUS),
                cross: if r < MIN_OUTLINE_RADIUS { 1.0 } else { 0.0 },
                screen: [self.config.width as f32, self.config.height as f32],
                _pad: [0.0; 2],
            };
            self.queue.write_buffer(&self.cursor_buffer, 0, bytemuck::cast_slice(&[u]));
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("cursor_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
            });
            pass.set_pipeline(&self.cursor_pipeline);
            pass.set_bind_group(0, &self.cursor_bind, &[]);
            pass.draw(0..6, 0..1);
        }

        for id in &textures_free {
            self.egui_renderer.free_texture(id);
        }
        self.last_ui = Some((tris, ppp));

        self.queue.submit(std::iter::once(enc.finish()));
        let suboptimal = output.suboptimal;
        output.present();

        // The swapchain still works but no longer matches the window
        // (resize in progress, maximize, DPI or monitor change): rebuild
        // it at the window's real size, otherwise Vulkan reports it on
        // every single frame.
        // Only once per suboptimal episode: while Windows shows the taskbar
        // thumbnail, presents can stay suboptimal whatever we do, and
        // rebuilding on every frame would only cost time.
        if suboptimal && !self.swapchain_rebuilt {
            let size = self.window.inner_size();
            if size.width > 0 && size.height > 0 {
                self.config.width  = size.width;
                self.config.height = size.height;
                self.surface.configure(&self.device, &self.config);
            }
        }
        self.swapchain_rebuilt = suboptimal;
    }
}

/// Pipeline drawing the brush outline (shaders/cursor.wgsl), premultiplied
/// alpha over the frame.
fn cursor_pipeline(device: &Device, format: TextureFormat) -> (RenderPipeline, Buffer, BindGroup) {
    let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label:   Some("cursor_bgl"),
        entries: &[BindGroupLayoutEntry {
            binding:    0,
            visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let buffer = device.create_buffer(&BufferDescriptor {
        label:              Some("cursor_buffer"),
        size:               std::mem::size_of::<CursorUniform>() as u64,
        usage:              BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&BindGroupDescriptor {
        label:   Some("cursor_bind"),
        layout:  &layout,
        entries: &[BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    });
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label:  Some("cursor_shader"),
        source: ShaderSource::Wgsl(include_str!("shaders/cursor.wgsl").into()),
    });
    let pl = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("cursor_pl"), bind_group_layouts: &[&layout], push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
        label:  Some("cursor_pipeline"),
        layout: Some(&pl),
        vertex: VertexState {
            module: &shader, entry_point: Some("vs_main"), buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: &shader, entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: MultisampleState::default(),
        multiview: None,
        cache: None,
    });
    (pipeline, buffer, bind)
}

/// 2x2x2 identity table: each corner maps to itself.
const IDENTITY_LUT: [[f32; 4]; 8] = [
    [0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [1.0, 1.0, 0.0, 1.0],
    [0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 1.0],
];

/// 3D lookup texture (half floats, filterable everywhere) + its sampler.
struct DisplayLut {
    _texture: Texture,
    view:     TextureView,
    sampler:  Sampler,
    size:     u32,
}

impl DisplayLut {
    fn new(device: &Device, queue: &Queue, data: &[[f32; 4]], size: u32) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label:           Some("display_lut"),
            size:            Extent3d { width: size, height: size, depth_or_array_layers: size },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       TextureDimension::D3,
            format:          TextureFormat::Rgba16Float,
            usage:           TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats:    &[],
        });
        let halfs: Vec<u16> = data.iter().flatten().map(|v| half::f16::from_f32(*v).to_bits()).collect();
        queue.write_texture(
            ImageCopyTexture { texture: &texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            bytemuck::cast_slice(&halfs),
            ImageDataLayout { offset: 0, bytes_per_row: Some(size * 8), rows_per_image: Some(size) },
            Extent3d { width: size, height: size, depth_or_array_layers: size },
        );
        let view = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("display_lut_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            ..Default::default()
        });
        Self { _texture: texture, view, sampler, size }
    }
}

fn canvas_bind_group(device: &Device, layout: &BindGroupLayout, canvas: &Canvas, sampler: &Sampler, view_buffer: &Buffer, lut: &DisplayLut) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label:   Some("view_bind_group"),
        layout,
        entries: &[
            BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&canvas.view) },
            BindGroupEntry { binding: 1, resource: BindingResource::Sampler(sampler) },
            BindGroupEntry { binding: 2, resource: view_buffer.as_entire_binding() },
            BindGroupEntry { binding: 3, resource: BindingResource::TextureView(&lut.view) },
            BindGroupEntry { binding: 4, resource: BindingResource::Sampler(&lut.sampler) },
        ],
    })
}

/// The view at one moment (see GpuState::view_snapshot).
#[derive(Clone, Copy, Debug)]
pub struct ViewSnapshot {
    view:   view::CanvasView,
    canvas: (u32, u32),
    area:   view::Area,
    ppp:    f32,
}

impl ViewSnapshot {
    /// Canvas pixel -> egui point.
    pub fn to_points(&self, p: (f32, f32)) -> (f32, f32) {
        let (x, y) = self.view.canvas_to_screen(p, self.canvas, self.area);
        (x / self.ppp, y / self.ppp)
    }
}
