//! How the canvas reaches the screen (shaders/canvas.wgsl).
//!
//! - **High**: each screen pixel shows the exact average of the canvas
//!   pixels under it, each weighted by the part of it covered (rotation
//!   included). No mipmaps (a third less canvas memory), no blur between
//!   levels, nothing rebuilt after a stroke; GPU time instead.
//! - **Standard**: the mipmaps, sampled trilinearly (old or weak GPUs).
//!
//! Close up (above 100 %), High shows canvas pixels as sharp squares with
//! smoothed edges, or blends them (smooth zoom-in).

use wgpu::*;

use crate::view::{Area, CanvasView};

/// How the canvas is drawn (Preferences > Canvas & display).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CanvasQuality {
    #[default]
    High,
    Standard,
}

impl CanvasQuality {
    /// Whether the canvas texture has mipmaps.
    pub fn mipmaps(self) -> bool { self == CanvasQuality::Standard }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ViewUniform {
    pub m0:          [f32; 4],
    pub m1:          [f32; 4],
    pub canvas_size: [f32; 2],
    pub _pad:        [f32; 2],
    /// x: display color management on (1) / off (0), y: LUT size.
    pub cm:          [f32; 4],
    /// Screen pixel -> canvas pixel (rows, xyz used).
    pub inv0:        [f32; 4],
    pub inv1:        [f32; 4],
    /// x: rotated (not by a multiple of 90°), w: smooth zoom-in.
    pub q:           [f32; 4],
}

/// Whether the view is tilted (a screen pixel covers a tilted square of
/// the canvas): not a multiple of 90°.
fn tilted(angle: f32) -> bool { (2.0 * angle).sin().abs() > 1e-5 }

/// The screen pixel -> canvas pixel matrix of `view` (rows: x, y), in the
/// window's pixels.
pub(crate) fn screen_to_canvas(view: &CanvasView, canvas: (u32, u32), area: Area) -> [[f64; 3]; 2] {
    let (s, c) = (view.angle as f64).sin_cos();
    let z = view.zoom as f64;
    let cx = area[0] as f64 + area[2] as f64 / 2.0 + view.pan.0 as f64;
    let cy = area[1] as f64 + area[3] as f64 / 2.0 + view.pan.1 as f64;
    let (a, b, d, e) = (c / z, s / z, -s / z, c / z);
    [
        [a, b, -(a * cx + b * cy) + canvas.0 as f64 / 2.0],
        [d, e, -(d * cx + e * cy) + canvas.1 as f64 / 2.0],
    ]
}

/// What the canvas shader needs this frame.
pub(crate) fn view_uniform(view: &CanvasView, canvas: (u32, u32), area: Area, cm: [f32; 4], smooth_zoom_in: bool) -> ViewUniform {
    let [m0, m1] = view.ndc_matrix(canvas, area);
    let inv = screen_to_canvas(view, canvas, area);
    let row = |r: [f64; 3]| [r[0] as f32, r[1] as f32, r[2] as f32, 0.0];
    let smooth = smooth_zoom_in && view.zoom > 1.0;
    ViewUniform {
        m0, m1, canvas_size: [canvas.0 as f32, canvas.1 as f32], _pad: [0.0; 2], cm,
        inv0: row(inv[0]), inv1: row(inv[1]),
        q: [if tilted(view.angle) { 1.0 } else { 0.0 }, 0.0, 0.0, if smooth { 1.0 } else { 0.0 }],
    }
}

pub(crate) fn canvas_bgl(device: &Device) -> BindGroupLayout {
    let texture = |binding, dim| BindGroupLayoutEntry {
        binding, visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture { multisampled: false, view_dimension: dim, sample_type: TextureSampleType::Float { filterable: true } },
        count: None,
    };
    let sampler = |binding| BindGroupLayoutEntry {
        binding, visibility: ShaderStages::FRAGMENT, ty: BindingType::Sampler(SamplerBindingType::Filtering), count: None,
    };
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label:   Some("canvas_bgl"),
        entries: &[
            texture(0, TextureViewDimension::D2),
            sampler(1),
            BindGroupLayoutEntry {
                binding:    2,
                visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            texture(3, TextureViewDimension::D3),
            sampler(4),
        ],
    })
}

/// The canvas shader and its pipeline layout.
pub(crate) fn canvas_program(device: &Device, bgl: &BindGroupLayout) -> (ShaderModule, PipelineLayout) {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label:  Some("canvas_shader"),
        source: ShaderSource::Wgsl(include_str!("shaders/canvas.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("canvas_pipeline_layout"), bind_group_layouts: &[Some(bgl)], immediate_size: 0,
    });
    (shader, layout)
}

/// The canvas pipeline drawing with fragment entry `entry`: `fs_main`
/// (Standard) or `fs_high` (High).
pub(crate) fn canvas_pipeline(device: &Device, shader: &ShaderModule, layout: &PipelineLayout, format: TextureFormat, entry: &str) -> RenderPipeline {
    device.create_render_pipeline(&RenderPipelineDescriptor {
        label:  Some(entry),
        layout: Some(layout),
        vertex: VertexState { module: shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: PipelineCompilationOptions::default() },
        fragment: Some(FragmentState {
            module: shader, entry_point: Some(entry),
            targets: &[Some(ColorTargetState { format, blend: Some(BlendState::REPLACE), write_mask: ColorWrites::ALL })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive:     PrimitiveState::default(),
        depth_stencil: None,
        multisample:   MultisampleState::default(),
        multiview_mask:     None,
        cache:         None,
    })
}

/// The High pipeline, being built on another thread (the shader compiler
/// of DirectX 12 takes a few hundred milliseconds on its loops): the
/// canvas is drawn with the Standard one meanwhile.
pub(crate) enum HighPipeline {
    Building(std::thread::JoinHandle<(RenderPipeline, f32)>),
    Ready(RenderPipeline),
    /// The thread could not start or failed: built here when first needed.
    Missing,
}

impl HighPipeline {
    /// Start building it (on this thread for OpenGL, whose context is
    /// tied to a thread).
    pub fn start(device: &std::sync::Arc<Device>, shader: ShaderModule, layout: PipelineLayout, format: TextureFormat, here: bool) -> (Self, Option<f32>) {
        let build = move |device: &Device| {
            let t = std::time::Instant::now();
            let p = canvas_pipeline(device, &shader, &layout, format, "fs_high");
            (p, t.elapsed().as_secs_f32() * 1000.0)
        };
        if here {
            let (p, ms) = build(device);
            return (HighPipeline::Ready(p), Some(ms));
        }
        let d = std::sync::Arc::clone(device);
        match std::thread::Builder::new().name("canvas shader".into()).spawn(move || build(&d)) {
            Ok(h) => (HighPipeline::Building(h), None),
            Err(e) => { log::warn!("canvas shader thread: {e}"); (HighPipeline::Missing, None) }
        }
    }

    /// The pipeline if it is ready (collected once the thread is done),
    /// and the time it took when it just became ready.
    pub fn ready(&mut self) -> (Option<&RenderPipeline>, Option<f32>) {
        let mut took = None;
        if matches!(self, HighPipeline::Building(h) if h.is_finished()) {
            let HighPipeline::Building(h) = std::mem::replace(self, HighPipeline::Missing) else { unreachable!() };
            match h.join() {
                Ok((p, ms)) => { *self = HighPipeline::Ready(p); took = Some(ms); }
                Err(_) => log::error!("canvas shader thread failed"),
            }
        }
        match self { HighPipeline::Ready(p) => (Some(p), took), _ => (None, took) }
    }
}

/// The two canvas pipelines: Standard (`fs_main`) and High (`fs_high`).
#[cfg(test)]
pub(crate) fn canvas_pipelines(device: &Device, bgl: &BindGroupLayout, format: TextureFormat) -> (RenderPipeline, RenderPipeline) {
    let (shader, layout) = canvas_program(device, bgl);
    (canvas_pipeline(device, &shader, &layout, format, "fs_main"), canvas_pipeline(device, &shader, &layout, format, "fs_high"))
}

#[cfg(test)]
mod tests {
    //! The judge: a slow, exact CPU renderer in double precision, written
    //! apart from the shader (it clips the screen pixel by the canvas, then
    //! by each canvas pixel). The shader must give the same pictures. These
    //! tests run the real shader when a GPU is there (a software one does:
    //! Mesa's lavapipe); without one they say so and only check the judge.

    use super::*;

    type Pt = (f64, f64);

    /// `poly` clipped by the half-plane on the left of a -> b (convex
    /// polygons, counter-clockwise in a y-down frame: any consistent order).
    fn clip(poly: &[Pt], a: Pt, b: Pt) -> Vec<Pt> {
        let side = |p: Pt| (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        let mut out = Vec::new();
        for i in 0..poly.len() {
            let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 { out.push(p); }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                out.push((p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t));
            }
        }
        out
    }

    fn area(poly: &[Pt]) -> f64 {
        let n = poly.len();
        (0..n).map(|i| poly[i].0 * poly[(i + 1) % n].1 - poly[(i + 1) % n].0 * poly[i].1).sum::<f64>().abs() / 2.0
    }

    /// `poly` (any orientation) clipped by the box [x0, x1] × [y0, y1].
    fn clip_box(poly: &[Pt], x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<Pt> {
        // Orient the polygon so that "left of each box side" is inside.
        let mut p = poly.to_vec();
        let signed: f64 = (0..p.len()).map(|i| p[i].0 * p[(i + 1) % p.len()].1 - p[(i + 1) % p.len()].0 * p[i].1).sum();
        if signed < 0.0 { p.reverse(); }
        // With the polygon positive (x right, y down), the box corners in
        // the same turning order keep the inside on the left.
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)];
        for k in 0..4 {
            if p.is_empty() { break; }
            p = clip(&p, corners[k], corners[(k + 1) % 4]);
        }
        p
    }

    /// The exact picture of `img` (w × h straight RGBA) seen through `inv`
    /// (screen pixel -> canvas pixel), at the screen pixel (x, y): the
    /// average color, 0..255, or None outside the canvas.
    fn judge(img: &[u8], w: u32, h: u32, inv: [[f64; 3]; 2], x: u32, y: u32) -> Option<[f64; 4]> {
        let at = |px: f64, py: f64| (inv[0][0] * px + inv[0][1] * py + inv[0][2], inv[1][0] * px + inv[1][1] * py + inv[1][2]);
        let (fx, fy) = (x as f64, y as f64);
        let quad = [at(fx, fy), at(fx + 1.0, fy), at(fx + 1.0, fy + 1.0), at(fx, fy + 1.0)];
        let on = clip_box(&quad, 0.0, 0.0, w as f64, h as f64);
        let total = area(&on);
        if on.len() < 3 || total <= 1e-12 { return None; }
        let (lo_x, hi_x) = on.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (lo_y, hi_y) = on.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        let mut sum = [0.0f64; 4];
        for j in lo_y.floor() as u32..(hi_y.ceil() as u32).min(h) {
            for i in lo_x.floor() as u32..(hi_x.ceil() as u32).min(w) {
                let part = area(&clip_box(&on, i as f64, j as f64, i as f64 + 1.0, j as f64 + 1.0));
                if part <= 0.0 { continue; }
                let k = ((j * w + i) * 4) as usize;
                for c in 0..4 { sum[c] += img[k + c] as f64 * part; }
            }
        }
        Some(sum.map(|s| s / total))
    }

    /// A test picture: noise, one-pixel lines, a hard edge, transparency.
    fn picture(w: u32, h: u32) -> Vec<u8> {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                let k = ((y * w + x) * 4) as usize;
                let v = if x % 17 == 3 || y % 23 == 5 { [0, 0, 0, 255] }
                    else if x > w / 2 && y > h / 2 { [250, 40, 30, 255] }
                    else { [(seed & 255) as u8, (seed >> 8 & 255) as u8, (seed >> 16 & 255) as u8, if x < 20 { 128 } else { 255 }] };
                px[k..k + 4].copy_from_slice(&v);
            }
        }
        px
    }

    /// The judge itself: an upright view at 50 % averages 2 × 2 blocks; at
    /// 100 % it returns the pixels; a tilted view keeps a flat color flat.
    #[test]
    fn the_judge_knows_simple_cases() {
        let (w, h) = (8u32, 6u32);
        let img = picture(w, h);
        let half = [[2.0, 0.0, 0.0], [0.0, 2.0, 0.0]];
        let got = judge(&img, w, h, half, 1, 1).unwrap();
        for c in 0..4 {
            let k = |x: u32, y: u32| img[((y * w + x) * 4) as usize + c] as f64;
            let want = (k(2, 2) + k(3, 2) + k(2, 3) + k(3, 3)) / 4.0;
            assert!((got[c] - want).abs() < 1e-9, "{c}: {} {want}", got[c]);
        }
        let one = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(judge(&img, w, h, one, 4, 2).unwrap()[0], img[((2 * w + 4) * 4) as usize] as f64);
        let flat = vec![77u8; (w * h * 4) as usize];
        let (s, c) = 0.5f64.sin_cos();
        let tilt = [[c * 0.7, s * 0.7, 3.0], [-s * 0.7, c * 0.7, 3.0]];
        assert!((judge(&flat, w, h, tilt, 1, 1).unwrap()[1] - 77.0).abs() < 1e-9);
        assert!(judge(&flat, w, h, tilt, 40, 40).is_none());
    }

    struct Gpu { device: Device, queue: Queue }

    fn gpu() -> Option<Gpu> {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: None, required_features: Features::empty(), required_limits: adapter.limits(), memory_hints: MemoryHints::MemoryUsage, ..Default::default()
        })).ok()?;
        Some(Gpu { device, queue })
    }

    /// The real shader's picture of `img` in a `sw` × `sh` frame.
    fn shader_picture(g: &Gpu, img: &[u8], (w, h): (u32, u32), view: &CanvasView, (sw, sh): (u32, u32), high: bool) -> Vec<u8> {
        let (device, queue) = (&g.device, &g.queue);
        let canvas = aurastudio_canvas::Canvas::new(device, w, h, !high);
        queue.write_texture(
            TexelCopyTextureInfo { texture: &canvas.texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            img,
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let format = TextureFormat::Rgba8Unorm;
        let bgl = canvas_bgl(device);
        let (standard, exact) = canvas_pipelines(device, &bgl, format);
        let area = [0.0, 0.0, sw as f32, sh as f32];
        let u = view_uniform(view, (w, h), area, [0.0, 2.0, 0.0, 0.0], false);
        let buffer = util::DeviceExt::create_buffer_init(device, &util::BufferInitDescriptor {
            label: None, contents: bytemuck::cast_slice(&[u]), usage: BufferUsages::UNIFORM,
        });
        let lut = crate::DisplayLut::new(device, queue, &crate::IDENTITY_LUT, 2);
        let bind = crate::canvas_bind_group(device, &bgl, &canvas, &canvas.sampler, &buffer, &lut);
        let target = device.create_texture(&TextureDescriptor {
            label: None, size: Extent3d { width: sw, height: sh, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: TextureDimension::D2, format, usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC, view_formats: &[],
        });
        let tv = target.create_view(&TextureViewDescriptor::default());
        let row = (sw * 4).div_ceil(256) * 256;
        let read = device.create_buffer(&BufferDescriptor { label: None, size: (row * sh) as u64, usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(RenderPassColorAttachment { view: &tv, depth_slice: None, resolve_target: None, ops: Operations { load: LoadOp::Clear(Color::TRANSPARENT), store: StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(if high { &exact } else { &standard });
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..6, 0..1);
        }
        enc.copy_texture_to_buffer(
            TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyBufferInfo { buffer: &read, layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(sh) } },
            Extent3d { width: sw, height: sh, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        read.slice(..).map_async(MapMode::Read, |_| {});
        let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
        let data = read.slice(..).get_mapped_range().unwrap();
        (0..sh).flat_map(|y| data[(y * row) as usize..(y * row + sw * 4) as usize].to_vec()).collect()
    }

    /// The view of a `w` × `h` canvas at `zoom` and `angle` (radians),
    /// centered in a `sw` × `sh` frame, nudged off the pixel grid.
    fn view_at(zoom: f32, angle: f32) -> CanvasView {
        CanvasView { zoom, angle, pan: (0.37, -0.21), fit: false }
    }

    /// Compare the shader with the judge over the frame: the largest
    /// difference (0..255) where the canvas is drawn.
    fn worst(g: &Gpu, w: u32, h: u32, zoom: f32, angle: f32, sw: u32, sh: u32) -> (f64, usize) {
        let img = picture(w, h);
        let view = view_at(zoom, angle);
        let got = shader_picture(g, &img, (w, h), &view, (sw, sh), true);
        let inv = screen_to_canvas(&view, (w, h), [0.0, 0.0, sw as f32, sh as f32]);
        let (mut worst, mut n) = (0.0f64, 0);
        for y in 0..sh {
            for x in 0..sw {
                let k = ((y * sw + x) * 4) as usize;
                let drawn = got[k + 3] != 0 || got[k..k + 3] != [0, 0, 0];
                let Some(want) = judge(&img, w, h, inv, x, y) else { continue };
                // The quad covers the pixels whose center is on the canvas;
                // a pixel barely touching it is left to the backdrop.
                let center = (inv[0][0] * (x as f64 + 0.5) + inv[0][1] * (y as f64 + 0.5) + inv[0][2],
                              inv[1][0] * (x as f64 + 0.5) + inv[1][1] * (y as f64 + 0.5) + inv[1][2]);
                let margin = 0.01 / zoom as f64;
                let inside = center.0 > margin && center.1 > margin && center.0 < w as f64 - margin && center.1 < h as f64 - margin;
                if !inside { continue; }
                assert!(drawn, "pixel {x},{y} not drawn");
                n += 1;
                for c in 0..4 { worst = worst.max((got[k + c] as f64 - want[c]).abs()); }
            }
        }
        (worst, n)
    }

    /// High matches the exact average at 33, 25, 12 and 5 %, upright and
    /// tilted, and close up: within one step of 255 (rounding).
    #[test]
    fn the_high_shader_gives_the_exact_average() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter (install Mesa's lavapipe to run the shader): judge only");
            return;
        };
        for (zoom, angle, sw, sh, w, h) in [
            (0.33, 0.0, 96, 72, 300, 220),
            (0.25, 0.0, 80, 60, 300, 220),
            (0.12, 0.0, 48, 40, 400, 300),
            (0.05, 0.0, 30, 24, 520, 420),
            (0.33, 0.52, 120, 110, 300, 220),
            (0.12, -1.1, 70, 70, 400, 300),
            (0.05, 2.3, 40, 40, 520, 420),
            (1.0, 0.0, 64, 64, 50, 40),
            (3.7, 0.3, 120, 120, 30, 30),
            (0.5, std::f32::consts::FRAC_PI_2, 90, 120, 160, 200),
        ] {
            let (d, n) = worst(&g, w, h, zoom, angle, sw, sh);
            assert!(n > 100, "zoom {zoom} angle {angle}: only {n} pixels compared");
            assert!(d <= 1.0, "zoom {zoom} angle {angle}: off by {d:.2} / 255 over {n} pixels");
        }
    }

    /// The High pipeline built on another thread comes back ready, with
    /// its time; built here (OpenGL) it is ready at once.
    #[test]
    fn the_high_pipeline_is_built_in_the_background() {
        let Some(g) = gpu() else { return };
        let device = std::sync::Arc::new(g.device);
        let bgl = canvas_bgl(&device);
        let (shader, layout) = canvas_program(&device, &bgl);
        let (mut high, now) = HighPipeline::start(&device, shader, layout, TextureFormat::Rgba8Unorm, false);
        assert!(now.is_none());
        let t = std::time::Instant::now();
        let took = loop {
            let (p, took) = high.ready();
            if p.is_some() { break took; }
            assert!(t.elapsed().as_secs() < 60, "never ready");
            std::thread::yield_now();
        };
        assert!(took.is_some_and(|ms| ms >= 0.0));
        assert!(high.ready().0.is_some() && high.ready().1.is_none(), "the time is told once");
        let (shader, layout) = canvas_program(&device, &bgl);
        let (mut here, ms) = HighPipeline::start(&device, shader, layout, TextureFormat::Rgba8Unorm, true);
        assert!(ms.is_some() && here.ready().0.is_some());
    }

    /// High at 100 % upright is the canvas itself, pixel for pixel.
    #[test]
    fn high_at_one_to_one_shows_the_pixels() {
        let Some(g) = gpu() else { return };
        let (w, h) = (40u32, 30u32);
        let img = picture(w, h);
        let view = CanvasView { zoom: 1.0, angle: 0.0, pan: (0.0, 0.0), fit: false };
        let got = shader_picture(&g, &img, (w, h), &view, (w, h), true);
        assert_eq!(got, img);
    }
}
