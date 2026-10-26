mod lift;
mod mip;
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

/// Graphics APIs the window can be drawn with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend { Vulkan, DirectX12, OpenGl, Metal }

impl Backend {
    fn wgpu(self) -> Backends {
        match self { Backend::Vulkan => Backends::VULKAN, Backend::DirectX12 => Backends::DX12, Backend::OpenGl => Backends::GL, Backend::Metal => Backends::METAL }
    }

    /// Built for this system.
    fn available(self) -> bool {
        match self {
            Backend::Metal => cfg!(target_os = "macos"),
            Backend::DirectX12 => cfg!(windows),
            Backend::Vulkan => !cfg!(target_os = "macos"),
            Backend::OpenGl => true,
        }
    }

    pub fn name(self) -> &'static str {
        match self { Backend::Vulkan => "Vulkan", Backend::DirectX12 => "DirectX 12", Backend::OpenGl => "OpenGL", Backend::Metal => "Metal" }
    }
}

pub struct GpuState {
    backend:         Backend,
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
    /// Keeps the canvas texture's mipmaps up to date.
    mip:             mip::MipGen,
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
    /// Textures egui dropped in the frame `last_ui` comes from: that frame
    /// may still draw them (a dialog closing draws its pictures one last
    /// time), and so may its cached redraws. Freed once a newer UI frame
    /// has been submitted.
    textures_to_free: Vec<egui::TextureId>,
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
    /// A transform's live preview (lift.rs), and its pipelines.
    lift:            Option<lift::Lift>,
    lift_pipes:      Option<lift::Pipes>,
}

pub use lift::GRID as LIFT_GRID;

/// What the graphics side holds, itemised for the memory report (bytes).
#[derive(Clone, Debug, Default)]
pub struct GpuMemory {
    /// The graphics API in use.
    pub backend:     String,
    /// Resources whose size is known from what they were made as; "~" in
    /// the name: an estimate.
    pub known:       Vec<(String, u64)>,
    /// egui's textures (fonts, icons, thumbnails): name, bytes.
    pub egui:        Vec<(String, u64)>,
    /// The driver memory allocator's own count (Vulkan, DirectX 12): bytes
    /// given to resources, and reserved from the device. None: the API
    /// does not say.
    pub allocated:   Option<u64>,
    pub reserved:    Option<u64>,
    /// The allocations it lists, biggest first.
    pub allocations: Vec<(String, u64)>,
}

/// Bytes of a 2D RGBA8 texture of `w` x `h` with all its mip levels.
fn mipped_bytes(w: u32, h: u32) -> u64 {
    let (mut w, mut h, mut total) = (w.max(1) as u64, h.max(1) as u64, 0u64);
    loop {
        total += w * h * 4;
        if w == 1 && h == 1 { return total; }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

impl GpuState {
    /// Itemise what the GPU holds (for Window > Memory report).
    pub fn memory(&self) -> GpuMemory {
        let mut m = GpuMemory { backend: format!("{:?}", self.backend), ..Default::default() };
        let (w, h) = (self.canvas.width, self.canvas.height);
        m.known.push((format!("canvas texture {w}x{h} with mipmaps"), mipped_bytes(w, h)));
        let lut = self.lut.size as u64;
        m.known.push((format!("display lookup table {lut}^3"), lut * lut * lut * 8));
        m.known.push((format!("~swapchain {}x{} (3 images)", self.config.width, self.config.height), self.config.width as u64 * self.config.height as u64 * 4 * 3));
        let tex = self.egui_ctx.tex_manager();
        let tex = tex.read();
        for (_, meta) in tex.allocated() { m.egui.push((meta.name.clone(), meta.bytes_used() as u64)); }
        m.egui.sort_by_key(|x| std::cmp::Reverse(x.1));
        if let Some(r) = self.device.generate_allocator_report() {
            m.allocated = Some(r.total_allocated_bytes);
            m.reserved = Some(r.total_reserved_bytes);
            m.allocations = r.allocations.into_iter().map(|a| (a.name, a.size)).collect();
            m.allocations.sort_by_key(|x| std::cmp::Reverse(x.1));
        }
        m
    }
}

#[cfg(test)]
mod memory_tests {
    use super::mipped_bytes;

    #[test]
    fn mip_chain_is_a_third_bigger() {
        assert_eq!(mipped_bytes(1, 1), 4);
        assert_eq!(mipped_bytes(2, 2), 16 + 4);
        assert_eq!(mipped_bytes(4, 2), 32 + 8 + 4);
        let full = 4096u64 * 4096 * 4;
        let all = mipped_bytes(4096, 4096);
        assert!(all > full && all < full * 4 / 3 + 64, "{all}");
    }
}

impl GpuState {
    /// `canvas_size`: initial canvas width and height in pixels. The
    /// graphics API `prefer`, else the next one that runs (Vulkan, DirectX
    /// 12, OpenGL); AURASTUDIO_BACKEND overrides. Err: none runs.
    pub async fn new(window: Arc<Window>, canvas_size: (u32, u32), prefer: Backend) -> Result<Self, String> {
        let size = window.inner_size();

        let forced = match std::env::var("AURASTUDIO_BACKEND").ok().as_deref() {
            Some("dx12")   => Some(Backend::DirectX12),
            Some("vulkan") => Some(Backend::Vulkan),
            Some("metal")  => Some(Backend::Metal),
            Some("gl")     => Some(Backend::OpenGl),
            _ => None,
        };
        let mut order = vec![forced.unwrap_or(prefer)];
        for b in [Backend::Metal, Backend::Vulkan, Backend::DirectX12, Backend::OpenGl] {
            if !order.contains(&b) && b.available() { order.push(b); }
        }
        let mut errors = Vec::new();
        let mut found = None;
        for b in order {
            let instance = Instance::new(InstanceDescriptor { backends: b.wgpu(), ..Default::default() });
            let surface = match instance.create_surface(Arc::clone(&window)) {
                Ok(s) => s,
                Err(e) => { errors.push(format!("{}: {e}", b.name())); continue; }
            };
            let adapter = instance.request_adapter(&RequestAdapterOptions {
                power_preference:       PowerPreference::HighPerformance,
                compatible_surface:     Some(&surface),
                force_fallback_adapter: false,
            }).await;
            let Some(adapter) = adapter else { errors.push(format!("{}: no adapter", b.name())); continue };
            let device = adapter.request_device(
                &DeviceDescriptor {
                    label:             None,
                    required_features: Features::empty(),
                    // The biggest texture the GPU takes (it caps the canvas size).
                    // What this GPU has (an older one, or OpenGL, has less
                    // than wgpu's defaults); its biggest texture caps the
                    // canvas size.
                    required_limits:   adapter.limits(),
                    // Small memory blocks (the default reserves 128 MB chunks).
                    memory_hints:      MemoryHints::MemoryUsage,
                },
                None,
            ).await;
            match device {
                Ok((device, queue)) => {
                    if b != prefer { log::warn!("graphics: {} unavailable, using {}", prefer.name(), b.name()); }
                    found = Some((b, surface, adapter, device, queue));
                    break;
                }
                Err(e) => errors.push(format!("{}: {e}", b.name())),
            }
        }
        let Some((backend, surface, adapter, device, queue)) = found else { return Err(errors.join("; ")) };

        let caps   = surface.get_capabilities(&adapter);
        // Non-sRGB surface: the pixels are sRGB already (else encoded twice).
        let format = caps.formats.iter().copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        // Lowest latency first: Mailbox, Immediate, then Fifo (~14 ms more);
        // AURASTUDIO_PRESENT overrides.
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
        let mip = mip::MipGen::new(&device, &canvas);
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

        Ok(Self {
            backend,
            surface, device, queue, config,
            canvas_pipeline, view_buffer, view_bind_group, canvas_bgl,
            nearest_sampler, nearest_bind_group,
            view: CanvasView::default(),
            last_ui: None,
            textures_to_free: Vec::new(),
            cursor_pipeline, cursor_buffer, cursor_bind,
            brush_cursor: None,
            egui_cursor: egui::CursorIcon::Default,
            lut,
            color_managed: false,
            canvas, window, mip,
            egui_ctx, egui_state, egui_renderer,
            viewport: [0.0, 0.0, size.width.max(1) as f32, size.height.max(1) as f32],
            text_input_active: false,
            swapchain_rebuilt: false,
            repaint_delay: None,
            clear_color: [0.4, 0.4, 0.4],
            dpi_scale,
            pixel_exact: false,
            ui_scale: 1.0,
            lift: None,
            lift_pipes: None,
        })
    }

    /// The graphics API drawing the window.
    pub fn backend(&self) -> Backend { self.backend }

    /// Biggest canvas side the GPU accepts (max 2D texture size).
    pub fn max_canvas_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Replace the canvas texture with a new size. The caller then uploads
    /// the new composite with upload_rect.
    /// A transform began: its live preview's pieces (see lift.rs). False
    /// if they don't fit the canvas.
    pub fn lift_begin(&mut self, src: aurastudio_canvas::Rect, below: &[u8], above: Option<&[u8]>, layers: &[(&[u8], f32)]) -> bool {
        let aniso = if self.backend == Backend::OpenGl { 1 } else { 16 };
        let pipes = self.lift_pipes.get_or_insert_with(|| lift::Pipes::new(&self.device, aniso));
        self.lift = lift::Lift::new(&self.device, &self.queue, pipes, &self.canvas, lift::Pieces { src, below, above, layers });
        self.lift.is_some()
    }

    /// The live preview for the transform of `src`, ready.
    pub fn lift_ready(&self, src: aurastudio_canvas::Rect) -> bool {
        self.lift.as_ref().is_some_and(|l| l.src == src)
    }

    /// Show the mesh `verts` (lift.rs); `live`: kept over what is
    /// uploaded until `lift_hold(false)`.
    pub fn lift_show(&mut self, verts: &[[f32; 4]], nearest: bool, live: bool) {
        if let Some(l) = &mut self.lift { l.show(&self.queue, verts, nearest, live); }
    }

    /// Stop drawing the preview over uploads (the exact result is in).
    pub fn lift_hold(&mut self, live: bool) {
        if let Some(l) = &mut self.lift { l.live = live; }
    }

    pub fn lift_end(&mut self) {
        self.lift = None;
    }

    pub fn resize_canvas(&mut self, width: u32, height: u32) {
        self.lift = None;
        self.canvas = Canvas::new(&self.device, width, height);
        self.mip.attach(&self.device, &self.canvas);
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

    /// Canvas pixel -> window position (physical pixels).
    pub fn canvas_to_screen(&self, p: (f32, f32)) -> (f32, f32) {
        self.view.canvas_to_screen(p, self.canvas_size(), self.viewport)
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

    /// A text field is being edited (not wants_keyboard_input: a clicked
    /// button has focus too, which disabled Delete).
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

    /// A press at `pos` starts a stroke: over the canvas, under no panel
    /// (egui's own flag is a frame late here).
    pub fn is_canvas_at(&self, pos: (f32, f32)) -> bool {
        let [vx, vy, vw, vh] = self.viewport;
        let inside = pos.0 >= vx && pos.0 < vx + vw && pos.1 >= vy && pos.1 < vy + vh;
        let ppp = self.egui_ctx.pixels_per_point();
        // Floating windows over the canvas (not egui's background layer).
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
        self.mip.mark(rect);
        if let Some(l) = &mut self.lift { l.uploaded(); }
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
        // Minimized: nothing to show (frames are drawn on demand, not only
        // on the window system's request).
        if self.window.is_minimized() == Some(true) { return; }
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
                // When egui wants its next frame (the app schedules it).
                self.repaint_delay = full_output.viewport_output.get(&egui::ViewportId::ROOT)
                    .map(|v| v.repaint_delay);

                // egui asks for IME (text input) only while a text field is focused.
                self.text_input_active = full_output.platform_output.ime.is_some();
                // The app picks the cursor: egui's wish is kept for it.
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
                // Free what the previous frame dropped (this frame no longer
                // draws it); keep what this one dropped for later.
                textures_free = std::mem::replace(&mut self.textures_to_free, full_output.textures_delta.free);
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
                // No frame this time: what the previous frame dropped can go
                // all the same (this one's waits).
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
        // A transform's live preview, into the canvas.
        if let (Some(l), Some(p)) = (&mut self.lift, &self.lift_pipes) {
            if let Some(r) = l.draw(&mut enc, p) { self.mip.mark(r); }
        }
        // Zoomed-out views sample the mipmaps: bring them up to date.
        self.mip.rebuild(&mut enc, &self.canvas);

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

        self.last_ui = Some((tris, ppp));

        self.queue.submit(std::iter::once(enc.finish()));
        // (After the submit: wgpu refuses a submit using a texture already
        // destroyed.)
        for id in &textures_free {
            self.egui_renderer.free_texture(id);
        }
        let suboptimal = output.suboptimal;
        output.present();

        // Suboptimal swapchain (resize, DPI change): rebuilt once per episode
        // (the taskbar thumbnail keeps it suboptimal).
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
