//! wgpu: the device, the window's swapchain, the canvas (in pages) and how it
//! is shown (view, display table, quality), uploads through a staging
//! ring, the transform's live preview, the brush outline. Knows no
//! window system, document or interface: the app hands it pixels and a
//! window.
//!
//! `view`: zoom / rotation / pan and the methods using them; `display`:
//! the canvas shader, its pipelines, the display table; `canvas`: the
//! canvas's pages; `mip`: mipmaps on demand; `upload`: the staging ring; `lift`: the transform preview.

mod canvas;
mod display;
mod lift;
mod mip;
mod upload;
mod view;

use std::sync::Arc;
use std::time::{Duration, Instant};

use aurastudio_core::{DisplayLut, Rect, Rgba8};
use wgpu::util::DeviceExt;
use wgpu::*;

pub use canvas::{Canvas, PageCount, PAGE};
pub use display::CanvasQuality;
pub use lift::{Lift, Pieces, GRID as LIFT_GRID};
pub use view::{wrap_angle, CanvasView, ViewSnapshot};

/// What a window must offer to be drawn into (winit's `Arc<Window>` does).
pub trait Target: WindowHandle + wgt::WgpuHasDisplayHandle + Clone {}
impl<T: WindowHandle + wgt::WgpuHasDisplayHandle + Clone> Target for T {}

/// Standard quality: from this zoom up, canvas pixels are shown as sharp
/// squares (nearest sampling) instead of being blurred, like Krita / CSP
/// close up (unless smooth zoom-in is chosen).
const NEAREST_FROM_ZOOM: f32 = 2.0;

/// Mipmaps unused this long (zoomed in) are dropped at the next frame.
const MIPS_KEPT: Duration = Duration::from_secs(5);

/// How the window is presented. Immediate: a frame is shown as soon as it
/// is drawn (lowest pen latency, two images); Fifo: in step with the
/// screen (no tearing). Mailbox only when asked (AURASTUDIO_PRESENT): the
/// driver may keep a third image for it.
fn present_mode(caps: &SurfaceCapabilities, vsync: bool) -> PresentMode {
    let wanted = match std::env::var("AURASTUDIO_PRESENT").ok().as_deref() {
        Some("mailbox")   => PresentMode::Mailbox,
        Some("immediate") => PresentMode::Immediate,
        Some("fifo")      => PresentMode::Fifo,
        _ if vsync => PresentMode::Fifo,
        _ => PresentMode::Immediate,
    };
    // (Fifo is always there.)
    if caps.present_modes.contains(&wanted) { wanted } else { PresentMode::Fifo }
}

/// Images of the swapchain, about: one frame in flight plus the one shown,
/// a third for Mailbox.
fn swapchain_images(mode: PresentMode) -> u64 { if mode == PresentMode::Mailbox { 3 } else { 2 } }

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

/// DirectX 12's shader compiler: FXC (built into Windows; enough for our
/// small shaders), or DXC when Microsoft's dxcompiler.dll sits next to the
/// program. (DXC built in, wgpu's `static-dxc`, adds ~25 MB to the .exe.)
/// Other APIs: their own.
fn shader_compiler(b: Backend) -> (Dx12Compiler, &'static str) {
    match b {
        Backend::DirectX12 => {
            let dll = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("dxcompiler.dll"))).filter(|p| p.is_file());
            match dll {
                Some(p) => (Dx12Compiler::DynamicDxc { dxc_path: p.to_string_lossy().into_owned() }, "DXC (dxcompiler.dll next to the program)"),
                None => (Dx12Compiler::Fxc, "FXC (put dxcompiler.dll next to the program for DXC)"),
            }
        }
        Backend::Vulkan => (Dx12Compiler::Fxc, "SPIR-V (Vulkan driver)"),
        Backend::Metal => (Dx12Compiler::Fxc, "Metal"),
        Backend::OpenGl => (Dx12Compiler::Fxc, "GLSL (OpenGL driver)"),
    }
}

/// What the graphics side holds, itemised for the memory report (bytes).
#[derive(Clone, Debug, Default)]
pub struct GpuMemory {
    /// The graphics API in use, and the adapter (name, driver).
    pub backend:     String,
    pub adapter:     String,
    /// Resources whose size is known from what they were made as; "~" in
    /// the name: an estimate.
    pub known:       Vec<(String, u64)>,
    /// The driver memory allocator's own count (Vulkan, DirectX 12): bytes
    /// given to resources, and reserved from the device. `None`: the API
    /// does not say.
    pub allocated:   Option<u64>,
    pub reserved:    Option<u64>,
    /// The allocations it lists, biggest first.
    pub allocations: Vec<(String, u64)>,
    /// How frames are presented, and the canvas display quality.
    pub present:     String,
    pub quality:     String,
    /// Times uploads waited for the GPU to keep staging memory low.
    pub upload_waits: u64,
    /// The shader compiler, and how long the High canvas shader took.
    pub shaders:     String,
}

pub struct GpuState {
    backend:         Backend,
    adapter_info:    String,
    shader_compiler: &'static str,
    surface:         Surface<'static>,
    device:          Arc<Device>,
    queue:           Queue,
    config:          SurfaceConfiguration,
    /// Set after rebuilding a suboptimal swapchain, cleared once presents
    /// are optimal again.
    swapchain_rebuilt: bool,
    /// Swapchains built since the start (the first one included).
    swapchain_builds: u64,
    /// The window changed size: the swapchain is built again before the
    /// next frame.
    size_pending:    bool,
    /// Scale of the monitor the window is on (2.0 at 200 %).
    pub(crate) dpi_scale: f32,
    /// 100 % zoom = one canvas pixel per physical screen pixel; off: per
    /// point, so a picture keeps the same size on a 4K and an HD screen.
    pub(crate) pixel_exact: bool,
    canvas_pipeline: RenderPipeline,
    /// The exact average (High quality), built in the background.
    canvas_high:     display::HighPipeline,
    /// How long building it took (ms), once known.
    high_ms:         Option<f32>,
    quality:         CanvasQuality,
    /// Above 100 %: canvas pixels blended (bilinear) instead of sharp.
    smooth_zoom_in:  bool,
    /// Presentation in step with the screen (Fifo) instead of Immediate.
    vsync:           bool,
    surface_caps:    SurfaceCapabilities,
    /// Pixels sent to textures, with a ceiling on the staging memory.
    uploads:         upload::Uploads,
    /// Something was uploaded since the last frame (a live transform
    /// preview draws again over it).
    uploaded:        bool,
    view_buffer:     Buffer,
    view_bind_group: BindGroup,
    /// The canvas's textures the bind group was made with.
    bound:           u64,
    canvas_bgl:      BindGroupLayout,
    /// Zoom / rotation / pan of the canvas.
    pub view:        CanvasView,
    /// The display table, applied when `color_managed`.
    lut:             display::LutTexture,
    color_managed:   bool,
    /// Brush outline: pipeline, uniform, and where to draw it (center in
    /// physical pixels, radius in screen pixels); `None`: hidden.
    cursor_pipeline: RenderPipeline,
    cursor_buffer:   Buffer,
    cursor_bind:     BindGroup,
    brush_cursor:    Option<((f32, f32), f32)>,
    pub canvas:      Canvas,
    /// Window area (physical pixels) where the canvas is drawn: x, y,
    /// width, height.
    pub(crate) viewport: [f32; 4],
    /// Backdrop color around the canvas (sRGB, 0..1).
    pub clear_color: [f64; 3],
    /// The transform preview's pipelines, made when first needed.
    lift_pipes:      Option<lift::Pipes>,
    /// Mipmaps kept but not sampled (zoomed in) since then.
    mips_idle:       Option<Instant>,
}

impl GpuState {
    /// Draw into `window`, of `size` physical pixels at display scale
    /// `dpi_scale`. `canvas_size`: the canvas in pixels. The graphics API
    /// `prefer`, else the next one that runs (Vulkan, DirectX 12, OpenGL);
    /// AURASTUDIO_BACKEND overrides. Err: none runs.
    pub async fn new<W: Target>(window: W, size: (u32, u32), dpi_scale: f32, canvas_size: (u32, u32), prefer: Backend, quality: CanvasQuality, vsync: bool) -> Result<Self, String> {
        aurastudio_core::zone!("gpu start");
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
            aurastudio_core::zone!("gpu device");
            let (compiler, _) = shader_compiler(b);
            // (OpenGL needs the display connection, Wayland above all.)
            let mut desc = InstanceDescriptor { backends: b.wgpu(), ..InstanceDescriptor::new_with_display_handle(Box::new(window.clone())) };
            desc.backend_options.dx12.shader_compiler = compiler;
            let instance = Instance::new(desc);
            let surface = match instance.create_surface(window.clone()) {
                Ok(s) => s,
                Err(e) => { errors.push(format!("{}: {e}", b.name())); continue; }
            };
            let adapter = instance.request_adapter(&RequestAdapterOptions {
                power_preference:       PowerPreference::HighPerformance,
                compatible_surface:     Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets:    false,
            }).await;
            let adapter = match adapter {
                Ok(a) => a,
                Err(e) => { errors.push(format!("{}: {e}", b.name())); continue }
            };
            // This GPU's limits, but not the million shader bindings it
            // offers: DirectX 12 makes a descriptor heap that size at once
            // (tens of MB); we use a few dozen.
            let mut limits = adapter.limits();
            limits.max_non_sampler_bindings = limits.max_non_sampler_bindings.min(65_536);
            let device = adapter.request_device(&DeviceDescriptor {
                label:             None,
                required_features: Features::empty(),
                // What this GPU has; its biggest texture caps the canvas.
                required_limits:   limits,
                // Small memory blocks (the default reserves 128 MB chunks).
                memory_hints:      MemoryHints::MemoryUsage,
                ..Default::default()
            }).await;
            match device {
                Ok((device, queue)) => {
                    if b != prefer { log::warn!("graphics: {} unavailable, using {}", prefer.name(), b.name()); }
                    found = Some((b, surface, adapter, Arc::new(device), queue));
                    break;
                }
                Err(e) => errors.push(format!("{}: {e}", b.name())),
            }
        }
        let Some((backend, surface, adapter, device, queue)) = found else { return Err(errors.join("; ")) };
        let info = adapter.get_info();
        let adapter_info = format!("{} ({:?}, driver {} {})", info.name, info.device_type, info.driver, info.driver_info);

        let caps = surface.get_capabilities(&adapter);
        // Non-sRGB surface: the pixels are sRGB already (else encoded twice).
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).or(caps.formats.first().copied()).ok_or("the window offers no image format")?;
        let present_mode = present_mode(&caps, vsync);
        log::info!("surface: {format:?}, present mode {present_mode:?} (available: {:?}), backend {:?}", caps.present_modes, info.backend);
        let config = SurfaceConfiguration {
            usage:        TextureUsages::RENDER_ATTACHMENT,
            format,
            width:        size.0.max(1),
            height:       size.1.max(1),
            present_mode,
            alpha_mode:   caps.alpha_modes.first().copied().unwrap_or(CompositeAlphaMode::Auto),
            view_formats: vec![],
            // At most 1 frame in flight: a new stroke sample is never stuck
            // behind an older frame waiting to be shown.
            desired_maximum_frame_latency: 1,
            color_space:  SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let canvas_bgl = display::canvas_bgl(&device);
        // Identity (2³ corners) until color management is switched on.
        let lut = display::LutTexture::new(&device, &queue, &DisplayLut::identity(2));
        let view_uniform = display::ViewUniform { canvas_size: [canvas_size.0 as f32, canvas_size.1 as f32], ..bytemuck::Zeroable::zeroed() };
        let view_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("view_buffer"),
            contents: bytemuck::cast_slice(&[view_uniform]),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });
        let (canvas_shader, canvas_layout) = display::canvas_program(&device, &canvas_bgl);
        let canvas_pipeline = display::canvas_pipeline(&device, &canvas_shader, &canvas_layout, format, "fs_main");
        let (canvas_high, high_ms) = display::HighPipeline::start(&device, canvas_shader, canvas_layout, format, backend == Backend::OpenGl);
        let (cursor_pipeline, cursor_buffer, cursor_bind) = cursor_pipeline(&device, format);
        // No mipmaps yet: made the first time a zoomed-out view needs them.
        let canvas = Canvas::new(&device, canvas_size.0.max(1), canvas_size.1.max(1), false);
        let view_bind_group = display::bind_group(&device, &canvas_bgl, &canvas, &view_buffer, &lut);
        let bound = canvas.generation();
        Ok(Self {
            backend, adapter_info, shader_compiler: shader_compiler(backend).1,
            surface, device, queue, config,
            swapchain_rebuilt: false, swapchain_builds: 1, size_pending: false,
            dpi_scale, pixel_exact: false,
            canvas_pipeline, canvas_high, high_ms, quality, smooth_zoom_in: false, vsync, surface_caps: caps,
            uploads: upload::Uploads::default(), uploaded: false,
            view_buffer, view_bind_group, bound, canvas_bgl,
            view: CanvasView::default(), lut, color_managed: false,
            cursor_pipeline, cursor_buffer, cursor_bind, brush_cursor: None,
            canvas,
            viewport: [0.0, 0.0, size.0.max(1) as f32, size.1.max(1) as f32],
            clear_color: [0.4, 0.4, 0.4],
            lift_pipes: None,
            mips_idle: None,
        })
    }

    /// The graphics API drawing the window.
    pub fn backend(&self) -> Backend { self.backend }
    /// For whoever draws over the canvas (the interface).
    pub fn device(&self) -> &Device { &self.device }
    pub fn queue(&self) -> &Queue { &self.queue }
    /// Format of the window's images.
    pub fn format(&self) -> TextureFormat { self.config.format }
    /// Size of the window's images (physical pixels).
    pub fn surface_size(&self) -> (u32, u32) { (self.config.width, self.config.height) }
    /// Biggest canvas side the GPU accepts (max 2D texture size).
    pub fn max_canvas_size(&self) -> u32 { self.device.limits().max_texture_dimension_2d }

    /// (Re)build the swapchain, counted for the report. The frames in
    /// flight finish first, so the old images go before the new ones come.
    fn configure_surface(&mut self) {
        aurastudio_core::zone!("gpu swapchain");
        let _ = self.device.poll(PollType::Wait { submission_index: None, timeout: None });
        self.surface.configure(&self.device, &self.config);
        self.swapchain_builds += 1;
    }

    /// Itemise what the GPU holds (Window > Memory report); the interface's
    /// textures are added by whoever draws it.
    pub fn memory(&self) -> GpuMemory {
        let mut m = GpuMemory { backend: format!("{:?}", self.backend), adapter: self.adapter_info.clone(), ..Default::default() };
        let (w, h) = (self.canvas.width, self.canvas.height);
        let c = self.canvas.count();
        m.known.push((format!(
            "canvas {w}x{h} in pages of {PAGE}: {} one-color (no pixels), {} two-color (1 byte a pixel), {} full (4 bytes){}; a single texture would be {:.1} MB",
            c.uniform, c.mono, c.full, if self.canvas.mipmaps() { ", with mipmaps" } else { "" }, (w as u64 * h as u64 * 4) as f64 / 1048576.0,
        ), c.bytes));
        let lut = self.lut.size as u64;
        m.known.push((format!("display lookup table {lut}^3"), lut * lut * lut * 8));
        let images = swapchain_images(self.config.present_mode);
        m.known.push((format!("~swapchain {}x{} ({images} images)", self.config.width, self.config.height), self.config.width as u64 * self.config.height as u64 * 4 * images));
        m.present = format!("{:?}, swapchain built {} times", self.config.present_mode, self.swapchain_builds);
        m.quality = format!("{:?}", self.quality);
        m.shaders = format!("{}; High canvas shader {}", self.shader_compiler, self.high_ms.map_or("still building".to_string(), |ms| format!("built in {ms:.0} ms (in the background)")));
        m.upload_waits = self.uploads.flushes;
        if let Some(r) = self.device.generate_allocator_report() {
            m.allocated = Some(r.total_allocated_bytes);
            m.reserved = Some(r.total_reserved_bytes);
            m.allocations = r.allocations.into_iter().map(|a| (a.name, a.size)).collect();
            m.allocations.sort_by_key(|x| std::cmp::Reverse(x.1));
        }
        m
    }

    // --- The transform preview ----------------------------------------------

    /// A transform began: its live preview, from its pieces (`below`,
    /// `above`: canvas-sized; each layer: (source + 2)² premultiplied).
    /// `None` if they don't fit the canvas. The caller keeps it, lends it
    /// to each frame and drops it when the transform ends.
    pub fn lift_begin(&mut self, pieces: Pieces<'_>) -> Option<Lift> {
        let aniso = if self.backend == Backend::OpenGl { 1 } else { 16 };
        let size = self.canvas_size();
        let pipes = self.lift_pipes.get_or_insert_with(|| lift::Pipes::new(&self.device, aniso));
        Lift::new(&self.device, &self.queue, pipes, size, pieces, &mut self.uploads)
    }

    /// Show the mesh `verts` ((LIFT_GRID + 1)² points: canvas x, y, texture
    /// u, v); `live`: kept over what is uploaded until `Lift::live` is set
    /// back.
    pub fn lift_show(&self, lift: &mut Lift, verts: &[[f32; 4]], nearest: bool, live: bool) { lift.show(&self.queue, verts, nearest, live); }

    // --- Settings ----------------------------------------------------------

    /// How the canvas is drawn. The pages are copied into textures with
    /// or without mipmaps (nothing to send again).
    pub fn set_quality(&mut self, quality: CanvasQuality) {
        if quality == self.quality { return; }
        self.quality = quality;
        // High needs none; Standard makes them when zoomed out.
        if quality == CanvasQuality::High { self.set_mipmaps(false); }
    }

    pub fn quality(&self) -> CanvasQuality { self.quality }

    /// The canvas pages with or without mipmaps (level 0 copied over).
    fn set_mipmaps(&mut self, on: bool) {
        if self.canvas.mipmaps() == on { return; }
        aurastudio_core::zone!("gpu mipmaps on / off");
        self.canvas.set_mipmaps(&self.device, &self.queue, on);
    }

    /// Above 100 %: canvas pixels blended (true) or sharp squares.
    pub fn set_smooth_zoom_in(&mut self, on: bool) { self.smooth_zoom_in = on; }

    /// Presentation in step with the screen (no tearing) or Immediate.
    pub fn set_vsync(&mut self, on: bool) {
        if on == self.vsync { return; }
        self.vsync = on;
        let mode = present_mode(&self.surface_caps, on);
        if mode != self.config.present_mode {
            self.config.present_mode = mode;
            self.configure_surface();
            log::info!("present mode {mode:?}");
        }
    }

    /// A transform's preview is over: the canvas rects to send again whole
    /// (pages it made full, sorted again then).
    pub fn take_lifted(&mut self) -> Vec<aurastudio_core::Rect> { self.canvas.take_lifted() }

    /// A new canvas texture of another size; the caller then uploads the
    /// merge. The view fits it whole.
    pub fn resize_canvas(&mut self, width: u32, height: u32) {
        self.canvas = Canvas::new(&self.device, width.max(1), height.max(1), false);
        self.rebuild_bind_groups();
        self.view.fit_to_area();
    }

    /// The canvas about to be sent whole: room for about `full` RGBA and
    /// `mono` two-color pages made at once (`LayerStack::cell_estimate`
    /// with `PAGE`).
    pub fn expect_pages(&mut self, full: u32, mono: u32) {
        self.canvas.expect(&self.device, &self.queue, &mut self.uploads, full, mono);
        self.rebuild_bind_groups();
    }

    fn rebuild_bind_groups(&mut self) {
        self.view_bind_group = display::bind_group(&self.device, &self.canvas_bgl, &self.canvas, &self.view_buffer, &self.lut);
        self.bound = self.canvas.generation();
    }

    /// Display color management of the canvas: the table from `color`;
    /// `None` (or one that changes nothing) switches it off.
    pub fn set_display_lut(&mut self, lut: Option<&DisplayLut>) {
        match lut {
            Some(t) if t.size >= 2 && t.texels.len() == t.size * t.size * t.size && !t.is_identity() => {
                self.lut = display::LutTexture::new(&self.device, &self.queue, t);
                self.rebuild_bind_groups();
                self.color_managed = true;
            }
            _ => self.color_managed = false,
        }
    }

    /// Show the brush outline at `center` (physical window pixels) with
    /// `radius` in screen pixels, or hide it.
    pub fn set_brush_cursor(&mut self, cursor: Option<((f32, f32), f32)>) { self.brush_cursor = cursor; }

    /// The window is now `size` physical pixels.
    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 { return; }
        // Back from minimized at the same size: the swapchain is kept.
        if size == (self.config.width, self.config.height) { self.size_pending = false; return; }
        // Built once, before the next frame: a monitor plugged in sends
        // several sizes in a row.
        (self.config.width, self.config.height) = size;
        self.size_pending = true;
        // Set from the interface's layout on the next frame; until then,
        // the whole window.
        self.viewport = [0.0, 0.0, size.0 as f32, size.1 as f32];
    }

    // --- Pixels and frames ---------------------------------------------------

    /// Copy merged pixels (the rows of `rect`, straight RGBA) to the canvas
    /// pages. A rect past the canvas is cut to it.
    /// Upload merged pieces, all in one submission (each would close a
    /// staging chunk of its own).
    pub fn upload_all(&mut self, pieces: &[(Rect, Vec<Rgba8>)]) {
        self.uploads.hold();
        for (rect, px) in pieces {
            if rect.is_empty() || px.len() as u64 != rect.area() { continue; }
            self.canvas.upload(&self.device, &self.queue, &mut self.uploads, *rect, px);
            self.uploaded = true;
        }
        self.uploads.release(&self.queue);
    }

    /// Upload the view transform and color management switch for this
    /// frame (64 bytes).
    fn write_view_uniform(&mut self) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.update_fit(c, a);
        let cm = [if self.color_managed { 1.0 } else { 0.0 }, self.lut.size as f32, 0.0, 0.0];
        let high = self.quality == CanvasQuality::High && matches!(self.canvas_high, display::HighPipeline::Ready(_));
        let sharp = !high && !self.smooth_zoom_in && self.view.zoom >= NEAREST_FROM_ZOOM;
        let levels = if self.canvas.mipmaps() { canvas::PAGE_LEVELS } else { 1 };
        let uniform = display::view_uniform(&self.view, c, a, cm, self.smooth_zoom_in, levels, sharp);
        self.queue.write_buffer(&self.view_buffer, 0, bytemuck::cast_slice(&[uniform]));
    }

    /// Start a frame: the canvas (with a transform's preview) drawn in its
    /// area. The interface is drawn over `frame.view` with `frame.encoder`,
    /// then `end_frame` shows it. `None`: no image to draw into this time
    /// (the next frame tries again).
    pub fn begin_frame(&mut self, lift: Option<&mut Lift>) -> Option<Frame> {
        aurastudio_core::zone!("gpu frame");
        if std::mem::take(&mut self.size_pending) { self.configure_surface(); }
        self.uploads.trim();
        // Standard quality zoomed out samples mipmaps: made when needed,
        // dropped after a while zoomed in (a third more memory).
        let sampled = self.quality == CanvasQuality::Standard && self.view.zoom < 1.0;
        if sampled { self.set_mipmaps(true); }
        self.mips_idle = match (sampled || !self.canvas.mipmaps(), self.mips_idle) {
            (true, _) => None,
            (false, None) => Some(Instant::now()),
            (false, Some(t)) if t.elapsed() >= MIPS_KEPT => { self.set_mipmaps(false); None }
            (false, t) => t,
        };
        self.write_view_uniform();
        let (output, suboptimal) = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(t) => (t, false),
            CurrentSurfaceTexture::Suboptimal(t) => (t, true),
            e => {
                match e {
                    CurrentSurfaceTexture::Lost | CurrentSurfaceTexture::Outdated => self.configure_surface(),
                    CurrentSurfaceTexture::Validation => log::error!("surface: validation error"),
                    // (Timeout, hidden window: the next frame tries again.)
                    _ => {}
                }
                return None;
            }
        };
        let view = output.texture.create_view(&TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(&CommandEncoderDescriptor { label: Some("frame") });
        // A transform's live preview, into the canvas (again over uploads).
        let uploaded = std::mem::take(&mut self.uploaded);
        let lifting = lift.is_some();
        if let (Some(l), Some(p)) = (lift, &self.lift_pipes) {
            if uploaded { l.uploaded(); }
            if let Some(r) = l.wanted() {
                let pages = self.canvas.full_pages(&self.device, &self.queue, &mut self.uploads, r);
                let size = self.canvas_size();
                for (slot, part) in l.draw(&self.device, &self.queue, &mut enc, p, size, (self.canvas.rgba_texture(), &pages)) { self.canvas.mark_full(slot, part); }
            }
        }
        self.canvas.prepare(&self.device, &self.queue, &mut enc, !lifting);
        if self.canvas.generation() != self.bound { self.rebuild_bind_groups(); }
        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("canvas_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view, depth_slice: None, resolve_target: None,
                    ops: Operations {
                        // Backdrop around the canvas (set by the theme).
                        load:  LoadOp::Clear(Color { r: self.clear_color[0], g: self.clear_color[1], b: self.clear_color[2], a: 1.0 }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes:         None,
                occlusion_query_set:      None,
                multiview_mask:           None,
            });
            let [vx, vy, vw, vh] = self.viewport;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            let (exact, took) = self.canvas_high.ready();
            if let Some(ms) = took {
                self.high_ms = Some(ms);
                log::info!("High canvas shader built in {ms:.0} ms (in the background)");
            }
            // Until the High pipeline is ready, the Standard one draws.
            let high = self.quality == CanvasQuality::High && exact.is_some();
            pass.set_pipeline(match exact { Some(p) if high => p, _ => &self.canvas_pipeline });
            pass.set_bind_group(0, &self.view_bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
        Some(Frame { encoder: enc, view, output, suboptimal })
    }

    /// Finish the frame: the brush outline over everything, then shown.
    /// `window_size`: the window's size now, to rebuild a suboptimal
    /// swapchain.
    pub fn end_frame(&mut self, frame: Frame, window_size: (u32, u32)) {
        aurastudio_core::zone!("gpu present");
        let Frame { encoder: mut enc, view, output, suboptimal } = frame;
        if let Some(((cx, cy), r)) = self.brush_cursor {
            let u = CursorUniform {
                center: [cx, cy],
                radius: r.max(MIN_OUTLINE_RADIUS),
                cross: if r < MIN_OUTLINE_RADIUS { 1.0 } else { 0.0 },
                screen: [self.config.width as f32, self.config.height as f32],
                _pad: [0.0; 2],
            };
            self.queue.write_buffer(&self.cursor_buffer, 0, bytemuck::cast_slice(&[u]));
            let mut pass = over_pass(&mut enc, &view, "cursor_pass");
            pass.set_pipeline(&self.cursor_pipeline);
            pass.set_bind_group(0, &self.cursor_bind, &[]);
            pass.draw(0..6, 0..1);
        }
        self.queue.submit(std::iter::once(enc.finish()));
        self.queue.present(output);
        // Suboptimal swapchain (resize, DPI change): rebuilt once per episode
        // (the taskbar thumbnail keeps it suboptimal).
        if suboptimal && !self.swapchain_rebuilt && window_size.0 > 0 && window_size.1 > 0 {
            (self.config.width, self.config.height) = window_size;
            self.configure_surface();
        }
        self.swapchain_rebuilt = suboptimal;
    }
}

/// A frame being drawn (see `GpuState::begin_frame`).
pub struct Frame {
    pub encoder: CommandEncoder,
    pub view:    TextureView,
    output:      SurfaceTexture,
    suboptimal:  bool,
}

/// A render pass drawing over what `view` already shows.
pub fn over_pass<'e>(enc: &'e mut CommandEncoder, view: &TextureView, label: &str) -> RenderPass<'e> {
    enc.begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes:         None,
        occlusion_query_set:      None,
        multiview_mask: None,
    })
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
        label: Some("cursor_pl"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
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
        multiview_mask: None,
        cache: None,
    });
    (pipeline, buffer, bind)
}
