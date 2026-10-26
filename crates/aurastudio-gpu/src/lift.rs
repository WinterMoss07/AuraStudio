//! A transform's live preview, put together on the GPU while a handle is
//! dragged (Photoshop, Procreate): the canvas under the moved layers, what
//! moves (its own mipmaps: shrunk, it stays smooth) drawn through a fine
//! mesh, and what is over them. Drawn straight into the canvas texture,
//! so the view, the mipmaps and the color management work as for paint.
//! The paint thread then sends the exact result, which takes its place.

use aurastudio_canvas::{Canvas, Rect};
use wgpu::util::DeviceExt;
use wgpu::*;

use crate::mip::MipGen;
use crate::upload::Uploads;

/// Cells of the mesh each way.
pub const GRID: u32 = 48;

const SHADER: &str = r#"
@group(0) @binding(0) var t: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(p * 2.0 - 1.0, 0.0, 1.0);
}

// Same pixel of a canvas-sized texture.
@fragment
fn fs_copy(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(t, vec2<i32>(pos.xy), 0);
}

// Straight RGBA, drawn over (premultiplied blending).
@fragment
fn fs_over(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(t, vec2<i32>(pos.xy), 0);
    return vec4(c.rgb * c.a, c.a);
}

struct U { size: vec2<f32>, opacity: f32, pad: f32 }
@group(0) @binding(1) var s: sampler;
@group(0) @binding(2) var<uniform> u: U;

struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> }

@vertex
fn vs_mesh(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>) -> V {
    var o: V;
    o.pos = vec4(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

// Premultiplied source.
@fragment
fn fs_mesh(v: V) -> @location(0) vec4<f32> {
    return textureSample(t, s, v.uv) * u.opacity;
}
"#;

/// Pipelines, made the first time a transform starts.
pub struct Pipes {
    copy:      RenderPipeline,
    over:      RenderPipeline,
    mesh:      RenderPipeline,
    full_bgl:  BindGroupLayout,
    mesh_bgl:  BindGroupLayout,
    linear:    Sampler,
    nearest:   Sampler,
    indices:   Buffer,
}

impl Pipes {
    pub fn new(device: &Device, anisotropy: u16) -> Self {
        let module = device.create_shader_module(ShaderModuleDescriptor { label: Some("lift"), source: ShaderSource::Wgsl(SHADER.into()) });
        let tex = |filterable| BindGroupLayoutEntry {
            binding: 0, visibility: ShaderStages::FRAGMENT,
            ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable }, view_dimension: TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let full_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor { label: Some("lift_full"), entries: &[tex(false)] });
        let mesh_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("lift_mesh"),
            entries: &[
                tex(true),
                BindGroupLayoutEntry { binding: 1, visibility: ShaderStages::FRAGMENT, ty: BindingType::Sampler(SamplerBindingType::Filtering), count: None },
                BindGroupLayoutEntry {
                    binding: 2, visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                    ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipe = |bgl: &BindGroupLayout, vs: &str, fs: &str, blend: Option<BlendState>, buffers: &[VertexBufferLayout]| {
            let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor { label: Some("lift"), bind_group_layouts: &[bgl], push_constant_ranges: &[] });
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some("lift"),
                layout: Some(&layout),
                vertex: VertexState { module: &module, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                fragment: Some(FragmentState {
                    module: &module, entry_point: Some(fs), compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState { format: Canvas::FORMAT, blend, write_mask: ColorWrites::ALL })],
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let vertex = VertexBufferLayout {
            array_stride: 16,
            step_mode: VertexStepMode::Vertex,
            attributes: &[
                VertexAttribute { format: VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                VertexAttribute { format: VertexFormat::Float32x2, offset: 8, shader_location: 1 },
            ],
        };
        let copy = pipe(&full_bgl, "vs_full", "fs_copy", None, &[]);
        let over = pipe(&full_bgl, "vs_full", "fs_over", Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), &[]);
        let mesh = pipe(&mesh_bgl, "vs_mesh", "fs_mesh", Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), &[vertex]);
        let sampler = |f: FilterMode, aniso: u16| device.create_sampler(&SamplerDescriptor {
            label: Some("lift"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter: f, min_filter: f, mipmap_filter: f,
            anisotropy_clamp: aniso,
            ..Default::default()
        });
        let n = GRID + 1;
        let idx: Vec<u32> = (0..GRID).flat_map(|j| (0..GRID).flat_map(move |i| {
            let a = j * n + i;
            [a, a + 1, a + n, a + 1, a + n + 1, a + n]
        })).collect();
        let indices = device.create_buffer_init(&util::BufferInitDescriptor { label: Some("lift_idx"), contents: bytemuck::cast_slice(&idx), usage: BufferUsages::INDEX });
        Self { copy, over, mesh, full_bgl, mesh_bgl, linear: sampler(FilterMode::Linear, anisotropy), nearest: sampler(FilterMode::Nearest, 1), indices }
    }
}

struct Source {
    // (Kept alive for the bind groups.)
    _canvas:  Canvas,
    _uniform: Buffer,
    linear:  BindGroup,
    nearest: BindGroup,
}

/// One transform's preview.
pub struct Lift {
    pub src:    Rect,
    size:       (u32, u32),
    below:      BindGroup,
    above:      Option<BindGroup>,
    sources:    Vec<Source>,
    /// The canvas texture's first level, drawn into.
    target:     TextureView,
    verts:      Buffer,
    /// The mesh drawn last.
    last:       Vec<[f32; 4]>,
    /// Everything drawn on since it began (canvas pixels).
    total:      Option<Rect>,
    /// Canvas pixels to draw again, and the sampling (true: nearest).
    shown:      Option<(Rect, bool)>,
    /// Drawn on every frame while true (the paint thread's uploads go
    /// under it); else once, then left as it is.
    pub live:   bool,
    dirty:      bool,
    /// Textures kept alive for the bind groups.
    _keep:      Vec<Texture>,
}

/// What a live preview is made of. `below`, `above`: canvas-sized RGBA
/// (straight); `layers`: what moves, (source + 2)² premultiplied, and
/// its opacity.
pub struct Pieces<'a> {
    pub src:    Rect,
    pub below:  &'a [u8],
    pub above:  Option<&'a [u8]>,
    pub layers: &'a [(&'a [u8], f32)],
}

impl Lift {
    /// `uploads`: the pixels go through it (bands, staging memory capped).
    pub fn new(device: &Device, queue: &Queue, pipes: &Pipes, canvas: &Canvas, p: Pieces, uploads: &mut Uploads) -> Option<Self> {
        let Pieces { src, below, above, layers } = p;
        let (w, h) = (canvas.width, canvas.height);
        if below.len() != (w * h * 4) as usize || above.is_some_and(|a| a.len() != below.len()) { return None; }
        let (sw, sh) = (src.2 - src.0 + 2, src.3 - src.1 + 2);
        let mut full = |data: &[u8]| {
            let t = device.create_texture(&TextureDescriptor {
                label: Some("lift_full"),
                size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1,
                dimension: TextureDimension::D2,
                format: Canvas::FORMAT,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            });
            uploads.write(device, queue, &t, (0, 0), (w, h), data);
            let view = t.create_view(&TextureViewDescriptor::default());
            let bind = device.create_bind_group(&BindGroupDescriptor { label: Some("lift_full"), layout: &pipes.full_bgl, entries: &[BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) }] });
            (t, bind)
        };
        let mut keep = Vec::new();
        let (bt, below) = full(below);
        keep.push(bt);
        let above = above.map(|a| { let (t, b) = full(a); keep.push(t); b });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("lift_mips") });
        let mut mip: Option<MipGen> = None;
        let mut sources = Vec::new();
        for &(px, opacity) in layers {
            if px.len() != (sw * sh * 4) as usize { return None; }
            let c = Canvas::new(device, sw, sh, true);
            uploads.write(device, queue, &c.texture, (0, 0), (sw, sh), px);
            match &mut mip { Some(m) => m.attach(device, &c), None => mip = Some(MipGen::new(device, &c)) }
            if let Some(m) = &mut mip { m.rebuild(&mut enc, &c); }
            let uniform = device.create_buffer_init(&util::BufferInitDescriptor {
                label: Some("lift_u"),
                contents: bytemuck::cast_slice(&[w as f32, h as f32, opacity.clamp(0.0, 1.0), 0.0]),
                usage: BufferUsages::UNIFORM,
            });
            let bind = |s: &Sampler| device.create_bind_group(&BindGroupDescriptor {
                label: Some("lift_src"),
                layout: &pipes.mesh_bgl,
                entries: &[
                    BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&c.view) },
                    BindGroupEntry { binding: 1, resource: BindingResource::Sampler(s) },
                    BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                ],
            });
            let (linear, nearest) = (bind(&pipes.linear), bind(&pipes.nearest));
            sources.push(Source { _canvas: c, _uniform: uniform, linear, nearest });
        }
        queue.submit(std::iter::once(enc.finish()));
        let target = canvas.texture.create_view(&TextureViewDescriptor { base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
        let n = (GRID + 1) * (GRID + 1);
        let verts = device.create_buffer(&BufferDescriptor { label: Some("lift_v"), size: n as u64 * 16, usage: BufferUsages::VERTEX | BufferUsages::COPY_DST, mapped_at_creation: false });
        Some(Self { src, size: (w, h), below, above, sources, target, verts, last: Vec::new(), total: None, shown: None, live: false, dirty: false, _keep: keep })
    }

    /// The mesh to draw now: (GRID + 1)² points, row by row, each canvas
    /// x, y and texture u, v. Only the cells that moved are drawn again
    /// (where they were and where they go); the first time, the source
    /// too (what moves leaves it).
    pub fn show(&mut self, queue: &Queue, verts: &[[f32; 4]], nearest: bool, live: bool) {
        let n = (GRID + 1) as usize;
        if verts.len() != n * n { return; }
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        fn grow(b: &mut [f32; 4], p: &[f32; 4]) { b[0] = b[0].min(p[0]); b[1] = b[1].min(p[1]); b[2] = b[2].max(p[0]); b[3] = b[3].max(p[1]); }
        if self.last.len() != verts.len() {
            for p in verts { grow(&mut b, p); }
            grow(&mut b, &[self.src.0 as f32, self.src.1 as f32, 0.0, 0.0]);
            grow(&mut b, &[self.src.2 as f32, self.src.3 as f32, 0.0, 0.0]);
        } else {
            for j in 0..n - 1 {
                for i in 0..n - 1 {
                    let k = [j * n + i, j * n + i + 1, (j + 1) * n + i, (j + 1) * n + i + 1];
                    if k.iter().any(|&k| verts[k][..2] != self.last[k][..2]) {
                        for &k in &k { grow(&mut b, &verts[k]); grow(&mut b, &self.last[k]); }
                    }
                }
            }
        }
        let nearest_changed = self.shown.is_some_and(|s| s.1 != nearest);
        if b[0] > b[2] && !nearest_changed && self.shown.is_some() { self.live = live; return; }
        if nearest_changed { for p in verts { grow(&mut b, p); } }
        queue.write_buffer(&self.verts, 0, bytemuck::cast_slice(verts));
        self.last = verts.to_vec();
        // Whole pixels, one more for the filter.
        let c = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
        let a = (c(b[0].floor() - 2.0, self.size.0), c(b[1].floor() - 2.0, self.size.1), c(b[2].ceil() + 2.0, self.size.0), c(b[3].ceil() + 2.0, self.size.1));
        if a.0 < a.2 && a.1 < a.3 {
            self.total = Some(self.total.map_or(a, |t| aurastudio_canvas::union(t, a)));
            self.shown = Some((a, nearest));
            self.dirty = true;
        }
        self.live = live;
    }

    /// Something was uploaded to the canvas: everything drawn again over
    /// it while live.
    pub fn uploaded(&mut self) {
        if let (true, Some(t), Some(s)) = (self.live, self.total, &mut self.shown) {
            s.0 = t;
            self.dirty = true;
        }
    }

    /// Draw into the canvas texture if needed; returns the area drawn
    /// (its mipmaps to rebuild).
    pub fn draw(&mut self, enc: &mut CommandEncoder, pipes: &Pipes) -> Option<Rect> {
        if !std::mem::take(&mut self.dirty) { return None; }
        let (r, nearest) = self.shown?;
        let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
            label: Some("lift"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &self.target, resolve_target: None,
                ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(0.0, 0.0, self.size.0 as f32, self.size.1 as f32, 0.0, 1.0);
        pass.set_scissor_rect(r.0, r.1, r.2 - r.0, r.3 - r.1);
        pass.set_pipeline(&pipes.copy);
        pass.set_bind_group(0, &self.below, &[]);
        pass.draw(0..3, 0..1);
        pass.set_pipeline(&pipes.mesh);
        pass.set_vertex_buffer(0, self.verts.slice(..));
        pass.set_index_buffer(pipes.indices.slice(..), IndexFormat::Uint32);
        for s in &self.sources {
            pass.set_bind_group(0, if nearest { &s.nearest } else { &s.linear }, &[]);
            pass.draw_indexed(0..GRID * GRID * 6, 0, 0..1);
        }
        if let Some(a) = &self.above {
            pass.set_pipeline(&pipes.over);
            pass.set_bind_group(0, a, &[]);
            pass.draw(0..3, 0..1);
        }
        Some(r)
    }
}
