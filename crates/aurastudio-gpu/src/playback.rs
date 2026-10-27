//! Animation playback from the GPU: the parts of the stack (aurastudio-
//! layers play.rs) sent once as 16 px tiles into atlas pages, then each
//! frame drawn into the canvas texture over white, part after part, with
//! the drawings that frame shows. No merge on the CPU, nothing sent while
//! playing. The view, mipmaps and color management work as for paint.

use std::collections::HashMap;

use aurastudio_canvas::Canvas;
use aurastudio_layers::play::{PlayPart, PlayPlan, PlayTile, PlayTiles, PLAY_TILE};
use wgpu::util::DeviceExt;
use wgpu::*;

/// Tiles per page side (a page of 1024 × 1024 pixels).
const SLOTS_SIDE: u32 = 64;
const PAGE: u32 = SLOTS_SIDE * PLAY_TILE;
const SLOTS: u32 = SLOTS_SIDE * SLOTS_SIDE;

const SHADER: &str = r#"
struct U { size: vec2<f32>, pad: vec2<f32> }
@group(0) @binding(0) var page: texture_2d<f32>;
@group(0) @binding(1) var<uniform> u: U;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) origin: vec2<f32>,
    @location(1) @interpolate(flat) slot: vec2<i32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
}

@vertex
fn vs(@builtin(vertex_index) i: u32, @location(0) dst: u32, @location(1) slot: u32, @location(2) color: vec4<f32>) -> V {
    // Two triangles: (0,0) (1,0) (0,1), (0,1) (1,0) (1,1).
    let c = vec2(f32(i == 1u || i == 4u || i == 5u), f32(i == 2u || i == 3u || i == 5u));
    let origin = vec2(f32(dst & 0xffffu), f32(dst >> 16u)) * 16.0;
    let p = origin + c * 16.0;
    var o: V;
    o.pos = vec4(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0, 0.0, 1.0);
    o.origin = origin;
    o.slot = vec2(i32(slot % 64u), i32(slot / 64u)) * 16;
    o.color = color;
    return o;
}

// Alpha tiles (color.a = 1): one color, the coverage from the page's red;
// RGBA tiles: straight colors. Premultiplied out.
@fragment
fn fs(v: V) -> @location(0) vec4<f32> {
    let local = vec2<i32>(v.pos.xy - v.origin);
    let t = textureLoad(page, v.slot + local, 0);
    if (v.color.a > 0.5) {
        return vec4(v.color.rgb * t.r, t.r);
    }
    return vec4(t.rgb * t.a, t.a);
}
"#;

/// One tile drawn: where (tile coordinates), from which slot, its color
/// and kind (alpha 255: an alpha tile).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Inst {
    dst:   u32,
    slot:  u32,
    color: [u8; 4],
}

struct Page {
    _texture: Texture,
    bind:     BindGroup,
    /// Slots given out (rows are filled from the top).
    used:     u32,
}

/// A part's draws: (alpha page?, page index, instances range).
struct Part {
    buffer: Buffer,
    draws:  Vec<(bool, usize, std::ops::Range<u32>)>,
}

/// Which part of the plan tiles belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlayKey { Static(usize), Drawing(u64) }

pub struct Playback {
    pipeline: RenderPipeline,
    bgl:      BindGroupLayout,
    uniform:  Buffer,
    plan:     Option<PlayPlan>,
    /// Alpha pages (R8) and RGBA pages.
    alpha:    Vec<Page>,
    rgba:     Vec<Page>,
    parts:    HashMap<PlayKey, Part>,
    /// Bytes of tiles held (pixels; the pages hold a little more).
    pub bytes: u64,
}

impl Playback {
    pub fn new(device: &Device) -> Self {
        let module = device.create_shader_module(ShaderModuleDescriptor { label: Some("playback"), source: ShaderSource::Wgsl(SHADER.into()) });
        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("playback"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0, visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: false }, view_dimension: TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1, visibility: ShaderStages::VERTEX,
                    ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor { label: Some("playback"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("playback"),
            layout: Some(&layout),
            vertex: VertexState {
                module: &module, entry_point: Some("vs"), compilation_options: Default::default(),
                buffers: &[Some(VertexBufferLayout {
                    array_stride: std::mem::size_of::<Inst>() as u64,
                    step_mode: VertexStepMode::Instance,
                    attributes: &[
                        VertexAttribute { format: VertexFormat::Uint32, offset: 0, shader_location: 0 },
                        VertexAttribute { format: VertexFormat::Uint32, offset: 4, shader_location: 1 },
                        VertexAttribute { format: VertexFormat::Unorm8x4, offset: 8, shader_location: 2 },
                    ],
                })],
            },
            fragment: Some(FragmentState {
                module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(ColorTargetState { format: Canvas::FORMAT, blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: ColorWrites::ALL })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&BufferDescriptor { label: Some("playback"), size: 16, usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, bgl, uniform, plan: None, alpha: Vec::new(), rgba: Vec::new(), parts: HashMap::new(), bytes: 0 }
    }

    /// Start over for `plan` (what was held goes).
    pub fn begin(&mut self, plan: PlayPlan) {
        self.plan = Some(plan);
        self.alpha.clear();
        self.rgba.clear();
        self.parts.clear();
        self.bytes = 0;
    }

    /// Let everything go.
    pub fn end(&mut self) {
        self.plan = None;
        self.alpha.clear();
        self.rgba.clear();
        self.parts.clear();
        self.bytes = 0;
    }

    pub fn plan(&self) -> Option<&PlayPlan> { self.plan.as_ref() }

    pub fn has(&self, key: PlayKey) -> bool { self.parts.contains_key(&key) }

    /// Send the tiles of `key`.
    pub fn add(&mut self, device: &Device, queue: &Queue, key: PlayKey, tiles: &PlayTiles) {
        let mut draws = Vec::new();
        let mut insts: Vec<Inst> = Vec::new();
        for alpha in [true, false] {
            let list: Vec<&PlayTile> = tiles.tiles.iter().filter(|t| matches!(t, PlayTile::Alpha { .. }) == alpha).collect();
            let mut at = 0;
            while at < list.len() {
                let (page, first) = self.room(device, alpha, list.len() - at);
                let n = (SLOTS - first).min((list.len() - at) as u32) as usize;
                let chunk = &list[at..at + n];
                // The rows of slots this chunk fills, sent at once.
                let (row0, rows) = (first / SLOTS_SIDE, (first + n as u32).div_ceil(SLOTS_SIDE) - first / SLOTS_SIDE);
                let bpp = if alpha { 1 } else { 4 };
                let stride = (PAGE * bpp) as usize;
                let mut band = vec![0u8; stride * (rows * PLAY_TILE) as usize];
                let start = insts.len() as u32;
                for (k, t) in chunk.iter().enumerate() {
                    let slot = first + k as u32;
                    let (sx, sy) = ((slot % SLOTS_SIDE) as usize * PLAY_TILE as usize, (slot / SLOTS_SIDE - row0) as usize * PLAY_TILE as usize);
                    let (src, color): (&[u8], [u8; 4]) = match t {
                        PlayTile::Alpha { rgb, a, .. } => (&a[..], [rgb[0], rgb[1], rgb[2], 255]),
                        PlayTile::Rgba { px, .. } => (&px[..], [0; 4]),
                    };
                    let row = PLAY_TILE as usize * bpp as usize;
                    for y in 0..PLAY_TILE as usize {
                        let o = (sy + y) * stride + sx * bpp as usize;
                        band[o..o + row].copy_from_slice(&src[y * row..(y + 1) * row]);
                    }
                    let (x, y) = t.pos();
                    insts.push(Inst { dst: x as u32 | (y as u32) << 16, slot, color });
                }
                let pages = if alpha { &mut self.alpha } else { &mut self.rgba };
                queue.write_texture(
                    TexelCopyTextureInfo { texture: &pages[page]._texture, mip_level: 0, origin: Origin3d { x: 0, y: row0 * PLAY_TILE, z: 0 }, aspect: TextureAspect::All },
                    &band,
                    TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(PAGE * bpp), rows_per_image: None },
                    Extent3d { width: PAGE, height: rows * PLAY_TILE, depth_or_array_layers: 1 },
                );
                // The next part starts on a fresh row (its band never
                // overwrites this one's).
                pages[page].used = (first + n as u32).div_ceil(SLOTS_SIDE) * SLOTS_SIDE;
                draws.push((alpha, page, start..insts.len() as u32));
                at += n;
            }
        }
        self.bytes += tiles.bytes() as u64;
        if insts.is_empty() {
            self.parts.insert(key, Part { buffer: device.create_buffer(&BufferDescriptor { label: None, size: 16, usage: BufferUsages::VERTEX, mapped_at_creation: false }), draws });
            return;
        }
        let buffer = device.create_buffer_init(&util::BufferInitDescriptor { label: Some("playback tiles"), contents: bytemuck::cast_slice(&insts), usage: BufferUsages::VERTEX });
        self.parts.insert(key, Part { buffer, draws });
    }

    /// A page of that kind with free slots (made if none): (index, first
    /// free slot).
    fn room(&mut self, device: &Device, alpha: bool, _want: usize) -> (usize, u32) {
        let pages = if alpha { &mut self.alpha } else { &mut self.rgba };
        if let Some((i, p)) = pages.iter().enumerate().find(|(_, p)| p.used < SLOTS) { return (i, p.used); }
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(if alpha { "playback alpha tiles" } else { "playback color tiles" }),
            size: Extent3d { width: PAGE, height: PAGE, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: TextureDimension::D2,
            format: if alpha { TextureFormat::R8Unorm } else { TextureFormat::Rgba8Unorm },
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind = device.create_bind_group(&BindGroupDescriptor {
            label: Some("playback page"), layout: &self.bgl,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view) },
                BindGroupEntry { binding: 1, resource: self.uniform.as_entire_binding() },
            ],
        });
        let pages = if alpha { &mut self.alpha } else { &mut self.rgba };
        pages.push(Page { _texture: texture, bind, used: 0 });
        (pages.len() - 1, 0)
    }

    /// Bytes of the pages on the GPU.
    pub fn page_bytes(&self) -> u64 {
        (self.alpha.len() as u64 * (PAGE * PAGE) as u64) + (self.rgba.len() as u64 * (PAGE * PAGE * 4) as u64)
    }

    /// Draw frame `t` into the canvas, over white. False (nothing drawn)
    /// when a part it needs isn't sent yet.
    pub fn draw(&self, queue: &Queue, enc: &mut CommandEncoder, canvas: &Canvas, t: u32) -> bool {
        let Some(plan) = &self.plan else { return false };
        if (plan.width, plan.height) != (canvas.width, canvas.height) { return false; }
        let shown = plan.shown_at(t);
        let mut keys = Vec::new();
        let mut k = 0;
        for p in &plan.parts {
            match p {
                PlayPart::Static(i) => keys.push(PlayKey::Static(*i)),
                PlayPart::Anim { .. } => {
                    if let Some(d) = shown[k] { keys.push(PlayKey::Drawing(d)); }
                    k += 1;
                }
            }
        }
        if !keys.iter().all(|key| self.parts.contains_key(key)) { return false; }
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&[canvas.width as f32, canvas.height as f32, 0.0, 0.0]));
        let target = canvas.texture.create_view(&TextureViewDescriptor { base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
        let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
            label: Some("playback"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &target, depth_slice: None, resolve_target: None,
                ops: Operations { load: LoadOp::Clear(Color::WHITE), store: StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        for key in keys {
            let part = &self.parts[&key];
            if part.draws.is_empty() { continue; }
            pass.set_vertex_buffer(0, part.buffer.slice(..));
            for (alpha, page, range) in &part.draws {
                let pages = if *alpha { &self.alpha } else { &self.rgba };
                pass.set_bind_group(0, &pages[*page].bind, &[]);
                pass.draw(0..6, range.clone());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_layers::{LayerStack, Pick};

    /// A drawn frame read back from the GPU equals the merge (to 1 level)
    /// at every frame of a small scene.
    #[test]
    fn a_frame_drawn_from_the_tiles_is_the_merge() {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
        let (w, h) = (90u32, 70u32);
        let mut s = LayerStack::new(w, h);
        let folder = s.add_animation_folder("A");
        for (n, x, c) in [(1u32, 4u32, [180u8, 20, 20]), (2, 40, [20, 20, 180])] {
            let cel = s.make_sheet(folder, n).unwrap();
            let px: Vec<u8> = (0..30 * 30).flat_map(|i| if i % 5 < 3 { [c[0], c[1], c[2], (i % 251) as u8] } else { [0, 0, 0, 0] }).collect();
            s.set_layer_rect(cel, (x, 20, x + 30, 50), &px);
        }
        let over = s.add_layer("over");
        let px: Vec<u8> = (0..20 * 20).flat_map(|i| [(i * 3) as u8, 200, (i * 7) as u8, 128]).collect();
        s.set_layer_rect(over, (30, 30, 50, 50), &px);
        let plan = s.play_plan().unwrap();
        let mut pb = Playback::new(&device);
        pb.begin(plan.clone());
        for i in 0..plan.statics() { pb.add(&device, &queue, PlayKey::Static(i), &s.play_static(&plan, i)); }
        for d in plan.drawings() { pb.add(&device, &queue, PlayKey::Drawing(d), &s.play_drawing(&plan, d)); }
        let canvas = Canvas::new(&device, w, h, false);
        for t in 1..=2 {
            s.set_time(t);
            let want = s.composite_pick((0, 0, w, h), Pick::ALL, false);
            let mut enc = device.create_command_encoder(&CommandEncoderDescriptor::default());
            assert!(pb.draw(&queue, &mut enc, &canvas, t));
            let row = (w * 4).div_ceil(256) * 256;
            let read = device.create_buffer(&BufferDescriptor { label: None, size: (row * h) as u64, usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, mapped_at_creation: false });
            enc.copy_texture_to_buffer(
                TexelCopyTextureInfo { texture: &canvas.texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
                TexelCopyBufferInfo { buffer: &read, layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
                Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            queue.submit([enc.finish()]);
            read.slice(..).map_async(MapMode::Read, |_| {});
            let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
            let got = read.slice(..).get_mapped_range().unwrap();
            let mut worst = 0;
            for y in 0..h as usize {
                for x in 0..w as usize {
                    for c in 0..3 {
                        let a = want[(y * w as usize + x) * 4 + c] as i32;
                        let b = got[y * row as usize + x * 4 + c] as i32;
                        worst = worst.max((a - b).abs());
                    }
                }
            }
            assert!(worst <= 2, "frame {t}: off by {worst}");
        }
        assert!(pb.bytes > 0 && pb.page_bytes() >= pb.bytes);
    }
}
