mod canvas;
mod display;
mod lift;
mod mip;
mod playback;
mod upload;
mod view;
pub use display::CanvasQuality;
pub use view::{wrap_angle, CanvasView};
pub use playback::PlayKey;

use std::sync::Arc;

/// What a window must offer to be drawn into (winit's `Arc<Window>` does).
pub trait Target: WindowHandle + wgt::WgpuHasDisplayHandle + Clone {}
impl<T: WindowHandle + wgt::WgpuHasDisplayHandle + Clone> Target for T {}
use wgpu::*;
use wgpu::util::DeviceExt;
pub use canvas::Canvas;

/// Standard quality: from this zoom up, canvas pixels are shown as sharp
/// squares (nearest sampling) instead of being blurred, like Krita / CSP
/// close up (unless smooth zoom-in is chosen).
const NEAREST_FROM_ZOOM: f32 = 2.0;

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

/// Interface textures made and freed since the start (bytes), for the
/// memory report: a churn shows here even when the textures alive are few.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureTraffic {
    pub made:       u64,
    pub made_bytes: u64,
    pub freed:      u64,
    pub freed_bytes: u64,
    pub live_bytes: u64,
    pub peak_bytes: u64,
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
    /// Scale of the monitor the window is on (Windows display scaling:
    /// 2.0 at 200 %), as of the last check.
    dpi_scale:       f32,
    /// 100 % zoom = one canvas pixel per physical screen pixel (XnView's
    /// "1 image pixel = 1 screen pixel"); off: per point, so a picture
    /// keeps the same size on a 4K and an HD screen.
    pixel_exact:     bool,
    canvas_pipeline: RenderPipeline,
    /// The exact average (High quality), built in the background.
    canvas_high:     display::HighPipeline,
    /// How long building it took (ms), once known.
    high_ms:         Option<f32>,
    quality:         CanvasQuality,
    /// A quality chosen during a transform, applied when it ends (the
    /// transform draws into the canvas texture).
    quality_later:   Option<CanvasQuality>,
    /// Above 100 %: canvas pixels blended (bilinear) instead of sharp.
    smooth_zoom_in:  bool,
    /// Presentation in step with the screen (Fifo) instead of Immediate.
    vsync:           bool,
    surface_caps:    SurfaceCapabilities,
    /// Pixels sent to textures, with a ceiling on the staging memory.
    uploads:         upload::Uploads,
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
    view_bind_group: BindGroup,
    canvas_bgl:      BindGroupLayout,
    pub canvas:      Canvas,
    /// Window area (physical pixels) where the canvas is drawn: all the
    /// space not covered by docks. (x, y, width, height)
    viewport:        [f32; 4],
    /// Backdrop color around the canvas (sRGB, 0..1).
    pub clear_color: [f64; 3],
    /// A transform's live preview (lift.rs), and its pipelines.
    lift:            Option<lift::Lift>,
    lift_pipes:      Option<lift::Pipes>,
    /// Animation playback from tiles on the GPU (playback.rs), made when
    /// first played; the frame to draw at the next render.
    playback:        Option<playback::Playback>,
    play_frame:      Option<u32>,
}

pub use lift::GRID as LIFT_GRID;

/// The app lends a function giving the process's private bytes, so the
/// steps of the start can be measured here (the memory report shows them).
pub static MEMORY_PROBE: std::sync::OnceLock<fn() -> u64> = std::sync::OnceLock::new();

/// A step of the start: name, private bytes after it, milliseconds since
/// the start of `GpuState::new`.
static STEPS: std::sync::Mutex<Vec<(&'static str, u64, f32)>> = std::sync::Mutex::new(Vec::new());
static STEPS_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
static ADAPTER_INFO: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static SHADER_COMPILER: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// DirectX 12's shader compiler: FXC (built into Windows; enough for our
/// small shaders), or DXC when Microsoft's dxcompiler.dll sits next to the
/// program. (DXC built in, wgpu's `static-dxc`, adds ~25 MB to the .exe:
/// for when GPU filters and compositing effects bring big shaders.)
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

/// Note the memory now as step `name` of the start (once per name).
fn step(name: &'static str) {
    let t0 = STEPS_T0.get_or_init(std::time::Instant::now);
    let mut v = STEPS.lock().unwrap();
    if v.iter().any(|x| x.0 == name) { return; }
    v.push((name, MEMORY_PROBE.get().map_or(0, |f| f()), t0.elapsed().as_secs_f32() * 1000.0));
}

/// The steps of `GpuState::new` so far: name, private bytes after it,
/// milliseconds since it began.
pub fn startup_steps() -> Vec<(&'static str, u64, f32)> { STEPS.lock().unwrap().clone() }

/// What the graphics side holds, itemised for the memory report (bytes).
#[derive(Clone, Debug, Default)]
pub struct GpuMemory {
    /// The graphics API in use, and the adapter (name, driver).
    pub backend:     String,
    pub adapter:     String,
    /// Resources whose size is known from what they were made as; "~" in
    /// the name: an estimate.
    pub known:       Vec<(String, u64)>,
    /// egui's textures (fonts, icons, thumbnails): name, bytes.
    pub egui:        Vec<(String, u64)>,
    /// The interface textures sent whole: name and size, bytes, private
    /// bytes gained while sending.
    pub uploads:     Vec<(String, u64, u64)>,
    /// The driver memory allocator's own count (Vulkan, DirectX 12): bytes
    /// given to resources, and reserved from the device. None: the API
    /// does not say.
    pub allocated:   Option<u64>,
    pub reserved:    Option<u64>,
    /// The allocations it lists, biggest first.
    pub allocations: Vec<(String, u64)>,
    /// How frames are presented, and the canvas display quality.
    pub present:     String,
    pub quality:     String,
    /// Interface textures made and freed since the start.
    pub traffic:     TextureTraffic,
    /// Times uploads waited for the GPU to keep staging memory low.
    pub upload_waits: u64,
    /// The shader compiler, and how long the High canvas shader took.
    pub shaders:     String,
}

fn mb_text(b: u64) -> String { format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)) }

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
    /// Itemise what the GPU holds (for Window > Memory report); the
    /// interface's textures are added by whoever draws it.
    pub fn memory(&self) -> GpuMemory {
        let mut m = GpuMemory { backend: format!("{:?}", self.backend), adapter: ADAPTER_INFO.get().cloned().unwrap_or_default(), ..Default::default() };
        let (w, h) = (self.canvas.width, self.canvas.height);
        if self.canvas.texture.mip_level_count() > 1 {
            m.known.push((format!("canvas texture {w}x{h} with mipmaps"), mipped_bytes(w, h)));
        } else {
            m.known.push((format!("canvas texture {w}x{h}"), w as u64 * h as u64 * 4));
        }
        if let Some(p) = &self.playback {
            if p.page_bytes() > 0 { m.known.push((format!("playback tiles ({} of pixels)", mb_text(p.bytes)), p.page_bytes())); }
        }
        let lut = self.lut.size as u64;
        m.known.push((format!("display lookup table {lut}^3"), lut * lut * lut * 8));
        let images = swapchain_images(self.config.present_mode);
        m.known.push((format!("~swapchain {}x{} ({images} images)", self.config.width, self.config.height), self.config.width as u64 * self.config.height as u64 * 4 * images));
        m.present = format!("{:?}, swapchain built {} times", self.config.present_mode, self.swapchain_builds);
        m.quality = format!("{:?}", self.quality);
        m.shaders = format!(
            "{}; High canvas shader {}",
            SHADER_COMPILER.get().copied().unwrap_or("?"),
            self.high_ms.map_or("still building".to_string(), |ms| format!("built in {ms:.0} ms (in the background)")),
        );
        m.upload_waits = self.uploads.flushes;
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
    /// Draw into `window`, of `size` physical pixels at display scale
    /// `dpi_scale`. `canvas_size`: initial canvas width and height in pixels. The
    /// graphics API `prefer`, else the next one that runs (Vulkan, DirectX
    /// 12, OpenGL); AURASTUDIO_BACKEND overrides. Err: none runs.
    /// `quality`: how the canvas is drawn; `vsync`: presentation in step
    /// with the screen.
    pub async fn new<W: Target>(window: W, size: (u32, u32), dpi_scale: f32, canvas_size: (u32, u32), prefer: Backend, quality: CanvasQuality, vsync: bool) -> Result<Self, String> {

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
        step("before graphics");
        for b in order {
            let (shader_compiler, _) = shader_compiler(b);
            // (OpenGL needs the display connection, Wayland above all.)
            let mut desc = InstanceDescriptor { backends: b.wgpu(), ..InstanceDescriptor::new_with_display_handle(Box::new(window.clone())) };
            desc.backend_options.dx12.shader_compiler = shader_compiler;
            let instance = Instance::new(desc);
            step("graphics instance");
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
            step("adapter");
            let info = adapter.get_info();
            let _ = ADAPTER_INFO.set(format!("{} ({:?}, driver {} {})", info.name, info.device_type, info.driver, info.driver_info));
            // This GPU's limits, but not the million shader bindings it
            // offers: DirectX 12 makes a descriptor heap that size at once
            // (tens of MB); we use a few dozen.
            let mut limits = adapter.limits();
            limits.max_non_sampler_bindings = limits.max_non_sampler_bindings.min(65_536);
            let device = adapter.request_device(
                &DeviceDescriptor {
                    label:             None,
                    required_features: Features::empty(),
                    // What this GPU has (an older one, or OpenGL, has less
                    // than wgpu's defaults); its biggest texture caps the
                    // canvas size.
                    required_limits:   limits,
                    // Small memory blocks (the default reserves 128 MB chunks).
                    memory_hints:      MemoryHints::MemoryUsage,
                    ..Default::default()
                },
            ).await;
            match device {
                Ok((device, queue)) => {
                    let device = Arc::new(device);
                    if b != prefer { log::warn!("graphics: {} unavailable, using {}", prefer.name(), b.name()); }
                    found = Some((b, surface, adapter, device, queue));
                    break;
                }
                Err(e) => errors.push(format!("{}: {e}", b.name())),
            }
        }
        let Some((backend, surface, adapter, device, queue)) = found else { return Err(errors.join("; ")) };
        let _ = SHADER_COMPILER.set(shader_compiler(backend).1);
        step("device");

        let caps   = surface.get_capabilities(&adapter);
        // Non-sRGB surface: the pixels are sRGB already (else encoded twice).
        let format = caps.formats.iter().copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);

        let present_mode = present_mode(&caps, vsync);
        log::info!("surface: {format:?}, present mode {present_mode:?} (available: {:?}), backend {:?}",
            caps.present_modes, adapter.get_info().backend);

        let config = SurfaceConfiguration {
            usage:                         TextureUsages::RENDER_ATTACHMENT,
            format,
            width:                         size.0.max(1),
            height:                        size.1.max(1),
            present_mode,
            alpha_mode:                    caps.alpha_modes[0],
            view_formats:                  vec![],
            // At most 1 frame in flight: a new stroke sample is never stuck
            // behind an older frame waiting to be shown.
            desired_maximum_frame_latency: 1,
            // sRGB, as before (no wide gamut / HDR).
            color_space:                   SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);
        step("swapchain");

        let canvas_bgl = display::canvas_bgl(&device);

        // Display LUT: identity (2^3 corners) until the app sets the real
        // one, and switched off anyway.
        let lut = DisplayLut::new(&device, &queue, &IDENTITY_LUT, 2);

        let view_uniform = display::ViewUniform { canvas_size: [canvas_size.0 as f32, canvas_size.1 as f32], ..bytemuck::Zeroable::zeroed() };

        let view_buffer = device.create_buffer_init(&util::BufferInitDescriptor {
            label:    Some("view_buffer"),
            contents: bytemuck::cast_slice(&[view_uniform]),
            usage:    BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        });

        let (canvas_shader, canvas_layout) = display::canvas_program(&device, &canvas_bgl);
        let canvas_pipeline = display::canvas_pipeline(&device, &canvas_shader, &canvas_layout, format, "fs_main");
        let (canvas_high, high_ms) = display::HighPipeline::start(&device, canvas_shader, canvas_layout, format, backend == Backend::OpenGl);

        step("canvas shader");
        let (cursor_pipeline, cursor_buffer, cursor_bind) = cursor_pipeline(&device, format);
        let canvas = Canvas::new(&device, canvas_size.0, canvas_size.1, quality.mipmaps());
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

        step("canvas, mipmaps, display table");

        Ok(Self {
            backend,
            surface, device, queue, config,
            canvas_pipeline, canvas_high, high_ms, quality, quality_later: None, smooth_zoom_in: false, vsync, surface_caps: caps,
            uploads: upload::Uploads::default(),
            view_buffer, view_bind_group, canvas_bgl,
            nearest_sampler, nearest_bind_group,
            view: CanvasView::default(),
            cursor_pipeline, cursor_buffer, cursor_bind,
            brush_cursor: None,
            lut,
            color_managed: false,
            canvas, mip,
            viewport: [0.0, 0.0, size.0.max(1) as f32, size.1.max(1) as f32],
            swapchain_rebuilt: false,
            swapchain_builds: 1,
            size_pending: false,
            clear_color: [0.4, 0.4, 0.4],
            dpi_scale,
            pixel_exact: false,
            lift: None,
            lift_pipes: None,
            playback: None,
            play_frame: None,
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

    /// (Re)build the swapchain with `config`, counted for the report.
    fn configure_surface(&mut self) {
        self.surface.configure(&self.device, &self.config);
        self.swapchain_builds += 1;
    }

    /// Biggest canvas side the GPU accepts (max 2D texture size).
    pub fn max_canvas_size(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// A transform began: its live preview's pieces (see lift.rs). False
    /// if they don't fit the canvas.
    pub fn lift_begin(&mut self, src: aurastudio_core::Rect, below: &[u8], above: Option<&[u8]>, layers: &[(&[u8], f32)]) -> bool {
        let aniso = if self.backend == Backend::OpenGl { 1 } else { 16 };
        let pipes = self.lift_pipes.get_or_insert_with(|| lift::Pipes::new(&self.device, aniso));
        self.lift = lift::Lift::new(&self.device, &self.queue, pipes, &self.canvas, lift::Pieces { src, below, above, layers }, &mut self.uploads);
        self.lift.is_some()
    }

    /// The live preview for the transform of `src`, ready.
    pub fn lift_ready(&self, src: aurastudio_core::Rect) -> bool {
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
        if let Some(q) = self.quality_later.take() { self.set_quality(q); }
    }

    /// Playback from the GPU for `plan`: the tiles come with `play_add`.
    pub fn play_begin(&mut self, plan: aurastudio_animation::play::PlayPlan) {
        let device = &self.device;
        self.playback.get_or_insert_with(|| playback::Playback::new(device)).begin(plan);
        self.play_frame = None;
    }

    /// The plan being played from the GPU, if any.
    pub fn play_plan(&self) -> Option<&aurastudio_animation::play::PlayPlan> { self.playback.as_ref()?.plan() }

    /// The tiles of `key` are on the GPU.
    pub fn play_has(&self, key: PlayKey) -> bool { self.playback.as_ref().is_some_and(|p| p.has(key)) }

    pub fn play_add(&mut self, key: PlayKey, tiles: &aurastudio_animation::play::PlayTiles) {
        if let Some(p) = &mut self.playback { p.add(&self.device, &self.queue, key, tiles); }
    }

    /// Show frame `t` from the tiles at the next render. False when a part
    /// it needs isn't there yet (the merge shows it instead).
    pub fn play_show(&mut self, t: u32) -> bool {
        let Some(p) = &self.playback else { return false };
        let Some(plan) = p.plan() else { return false };
        if (plan.width, plan.height) != (self.canvas.width, self.canvas.height) || self.lift.is_some() { return false; }
        let mut k = 0;
        let ready = plan.parts.iter().all(|part| match part {
            aurastudio_animation::play::PlayPart::Static(i) => p.has(PlayKey::Static(*i)),
            aurastudio_animation::play::PlayPart::Anim { .. } => {
                let d = plan.shown_at(t)[k];
                k += 1;
                d.is_none_or(|d| p.has(PlayKey::Drawing(d)))
            }
        });
        if ready { self.play_frame = Some(t); }
        ready
    }

    /// Stop playing from the GPU and let its tiles go.
    pub fn play_end(&mut self) {
        if let Some(p) = &mut self.playback { p.end(); }
        self.play_frame = None;
    }

    /// How the canvas is drawn. The pixels are copied into a texture with
    /// or without mipmaps (nothing to send again); during a transform, once
    /// it ends.
    pub fn set_quality(&mut self, quality: CanvasQuality) {
        if self.lift.is_some() { self.quality_later = Some(quality); return; }
        self.quality_later = None;
        if quality == self.quality { return; }
        self.quality = quality;
        let (w, h) = (self.canvas.width, self.canvas.height);
        let canvas = Canvas::new(&self.device, w, h, quality.mipmaps());
        let mut enc = self.device.create_command_encoder(&CommandEncoderDescriptor { label: Some("quality") });
        enc.copy_texture_to_texture(
            TexelCopyTextureInfo { texture: &self.canvas.texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyTextureInfo { texture: &canvas.texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(std::iter::once(enc.finish()));
        self.canvas = canvas;
        self.mip.attach(&self.device, &self.canvas);
        self.rebuild_bind_groups();
    }

    pub fn quality(&self) -> CanvasQuality { self.quality }

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

    /// Replace the canvas texture with a new size. The caller then uploads
    /// the new composite with upload_rect.
    pub fn resize_canvas(&mut self, width: u32, height: u32) {
        self.lift = None;
        if let Some(q) = self.quality_later.take() { self.quality = q; }
        self.canvas = Canvas::new(&self.device, width, height, self.quality.mipmaps());
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

    /// Show the brush outline at `center` (physical window pixels) with
    /// `radius` in screen pixels, or hide it.
    pub fn set_brush_cursor(&mut self, cursor: Option<((f32, f32), f32)>) {
        self.brush_cursor = cursor;
    }

    /// The display scale the window is at (2.0 at 200 %).
    pub fn dpi_scale(&self) -> f32 { self.dpi_scale }

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

    /// The display scale the window is at (2.0 at 200 %). The picture
    /// keeps its size on screen. True if it changed.
    pub fn set_dpi_scale(&mut self, new: f32) -> bool {
        if (new - self.dpi_scale).abs() < 1e-3 { return false; }
        let k = new / self.dpi_scale;
        self.dpi_scale = new;
        if !self.pixel_exact && !self.view.fit {
            self.view.zoom = (self.view.zoom * k).clamp(view::MIN_ZOOM, view::MAX_ZOOM);
            self.view.pan = (self.view.pan.0 * k, self.view.pan.1 * k);
        }
        true
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

    /// The current view, to place what is drawn over the canvas (selection
    /// outline...) in points of `ppp` physical pixels.
    pub fn view_snapshot(&self, ppp: f32) -> ViewSnapshot {
        ViewSnapshot { view: self.view, canvas: self.canvas_size(), area: self.viewport, ppp }
    }

    /// Window area (physical pixels) where the canvas is drawn: x, y,
    /// width, height. Set from the interface's layout each frame.
    pub fn set_viewport(&mut self, area: [f32; 4]) { self.viewport = area; }

    /// `pos` (physical pixels) is in the canvas area.
    pub fn in_viewport(&self, pos: (f32, f32)) -> bool {
        let [vx, vy, vw, vh] = self.viewport;
        pos.0 >= vx && pos.0 < vx + vw && pos.1 >= vy && pos.1 < vy + vh
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

    /// The window is now `size` physical pixels.
    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 { return; }
        // Back from minimized at the same size: the swapchain is kept
        // (rebuilding it held old and new images at once, 2 × 23 MB, and
        // the driver kept a few MB each time).
        if size == (self.config.width, self.config.height) {
            self.size_pending = false;
            return;
        }
        // Built once, before the next frame: a monitor plugged in sends
        // several sizes in a row (0.16.1 built six swapchains for it).
        (self.config.width, self.config.height) = size;
        self.size_pending = true;
        // Set from the interface's layout on the next frame; until then,
        // the whole window.
        self.viewport = [0.0, 0.0, size.0 as f32, size.1 as f32];
    }

    /// Upload the view transform and color management switch for this
    /// frame (64 bytes).
    fn write_view_uniform(&mut self) {
        let (c, a) = (self.canvas_size(), self.viewport);
        self.view.update_fit(c, a);
        let cm = [if self.color_managed { 1.0 } else { 0.0 }, self.lut.size as f32, 0.0, 0.0];
        let uniform = display::view_uniform(&self.view, c, a, cm, self.smooth_zoom_in);
        self.queue.write_buffer(&self.view_buffer, 0, bytemuck::cast_slice(&[uniform]));
    }

    /// Copy merged pixels (`rgba`: the rows of `rect`, tightly packed) to
    /// the canvas texture.
    #[profiling::function]
    pub fn upload(&mut self, rect: aurastudio_core::Rect, rgba: &[u8]) {
        let (x0, y0, x1, y1) = rect;
        if x1 <= x0 || y1 <= y0 || x1 > self.canvas.width || y1 > self.canvas.height { return; }
        let w = x1 - x0;
        if rgba.len() != w as usize * (y1 - y0) as usize * 4 { return; }
        self.uploads.write(&self.device, &self.queue, &self.canvas.texture, (x0, y0), (w, y1 - y0), rgba);
        self.mip.mark(rect);
        if let Some(l) = &mut self.lift { l.uploaded(); }
    }

    /// Start a frame: the canvas (with a transform's preview or a frame
    /// of the animation) drawn in its area. The interface is drawn over
    /// `frame.view` with `frame.encoder`, then `end_frame` shows it. None:
    /// no image to draw into this time (the next frame tries again).
    #[profiling::function]
    pub fn begin_frame(&mut self) -> Option<Frame> {
        if std::mem::take(&mut self.size_pending) { self.configure_surface(); }
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
        let mut enc = self.device.create_command_encoder(
            &CommandEncoderDescriptor { label: Some("frame") }
        );
        // A transform's live preview, into the canvas.
        if let (Some(l), Some(p)) = (&mut self.lift, &self.lift_pipes) {
            if let Some(r) = l.draw(&mut enc, p) { self.mip.mark(r); }
        }
        // A frame of the animation, drawn from its tiles.
        if let (Some(t), Some(p)) = (self.play_frame.take(), &self.playback) {
            if p.draw(&self.queue, &mut enc, &self.canvas, t) { self.mip.mark((0, 0, self.canvas.width, self.canvas.height)); }
        }
        // Zoomed-out views sample the mipmaps: bring them up to date.
        self.mip.rebuild(&mut enc, &self.canvas);

        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("canvas_pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view:           &view,
                    depth_slice: None,
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
                multiview_mask: None,
            });

            let [vx, vy, vw, vh] = self.viewport;
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            let (exact, took) = self.canvas_high.ready();
            if let Some(ms) = took {
                self.high_ms = Some(ms);
                log::info!("High canvas shader built in {ms:.0} ms (in the background)");
            }
            // Until the High pipeline is ready, the Standard one draws (the
            // pixels alone, bilinear: no mipmaps in High).
            let high = self.quality == CanvasQuality::High && exact.is_some();
            pass.set_pipeline(match exact { Some(p) if high => p, _ => &self.canvas_pipeline });
            let sharp = !high && !self.smooth_zoom_in && self.view.zoom >= NEAREST_FROM_ZOOM;
            let bind = if sharp { &self.nearest_bind_group } else { &self.view_bind_group };
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        }
        Some(Frame { encoder: enc, view, output, suboptimal })
    }

    /// Finish the frame: the brush outline over everything, then shown.
    /// `window_size`: the window's size now (physical pixels), to rebuild a
    /// suboptimal swapchain.
    #[profiling::function]
    pub fn end_frame(&mut self, frame: Frame, window_size: (u32, u32)) {
        let Frame { encoder: mut enc, view, output, suboptimal } = frame;
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
            TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            bytemuck::cast_slice(&halfs),
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 8), rows_per_image: Some(size) },
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
