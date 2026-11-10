//! The brush cursor over the frame (shaders/cursor.wgsl): the outline of
//! the dab the brush would lay under the pen, light inside a dark halo
//! (seen on any color). A round tip's outline is computed exactly; a
//! sampled tip's is the 50 % contour of its picture (a small texture, sent
//! again only for another tip). Too small to outline: a crosshair.

use wgpu::*;

/// Below this outline radius (screen pixels) a crosshair is shown instead.
pub const MIN_OUTLINE_RADIUS: f32 = 3.0;

/// What the outline follows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CursorShape {
    /// A round tip: where the tip coordinates are 1 from the centre.
    Round,
    /// A sampled tip: its picture's contour (`GpuState::set_cursor_mask`).
    Mask,
}

/// The brush cursor of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushCursor {
    /// Its centre, physical window pixels.
    pub center: (f32, f32),
    /// A window offset (physical pixels) to the tip's coordinates, row-major
    /// (round: the outline at length 1; mask: the picture over −1..1).
    pub to_tip: [f32; 4],
    /// How far it reaches from the centre (window pixels).
    pub extent: f32,
    /// The outline's radius (window pixels): below `MIN_OUTLINE_RADIUS`, a
    /// crosshair.
    pub radius: f32,
    pub shape: CursorShape,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    center:      [f32; 2],
    screen:      [f32; 2],
    to_tip:      [f32; 4],
    mask_scale:  [f32; 2],
    mask_offset: [f32; 2],
    extent:      f32,
    /// 0 round, 1 mask, 2 crosshair.
    mode:        f32,
    _pad:        [f32; 2],
}

pub struct CursorPass {
    pipeline: RenderPipeline,
    layout:   BindGroupLayout,
    buffer:   Buffer,
    sampler:  Sampler,
    bind:     BindGroup,
    /// The tip picture in use: its id and size (with its clear border).
    mask:     Option<(u64, u32, u32)>,
}

impl CursorPass {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let entry = |binding, ty| BindGroupLayoutEntry { binding, visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT, ty, count: None };
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("cursor_bgl"),
            entries: &[
                entry(0, BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(1, BindingType::Texture { sample_type: TextureSampleType::Float { filterable: true }, view_dimension: TextureViewDimension::D2, multisampled: false }),
                entry(2, BindingType::Sampler(SamplerBindingType::Filtering)),
            ],
        });
        let buffer = device.create_buffer(&BufferDescriptor {
            label:              Some("cursor_buffer"),
            size:               std::mem::size_of::<Uniform>() as u64,
            usage:              BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("cursor_sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("cursor_shader"),
            source: ShaderSource::Wgsl(include_str!("shaders/cursor.wgsl").into()),
        });
        let pl = device.create_pipeline_layout(&PipelineLayoutDescriptor { label: Some("cursor_pl"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:  Some("cursor_pipeline"),
            layout: Some(&pl),
            vertex: VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: PipelineCompilationOptions::default() },
            fragment: Some(FragmentState {
                module: &shader, entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState { format, blend: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: ColorWrites::ALL })],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // (No tip yet: a clear pixel.)
        let bind = Self::bind_with(device, &layout, &buffer, &sampler, &mask_texture(device, 1, 1));
        Self { pipeline, layout, buffer, sampler, bind, mask: None }
    }

    fn bind_with(device: &Device, layout: &BindGroupLayout, buffer: &Buffer, sampler: &Sampler, mask: &Texture) -> BindGroup {
        let view = mask.create_view(&TextureViewDescriptor::default());
        device.create_bind_group(&BindGroupDescriptor {
            label:   Some("cursor_bind"),
            layout,
            entries: &[
                BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                BindGroupEntry { binding: 1, resource: BindingResource::TextureView(&view) },
                BindGroupEntry { binding: 2, resource: BindingResource::Sampler(sampler) },
            ],
        })
    }

    /// The tip picture in use (id: the same picture; None: none yet).
    pub fn mask_id(&self) -> Option<u64> { self.mask.map(|m| m.0) }

    /// The sampled tip's picture: `w` × `h` alpha, its border clear. Sent
    /// only when `id` changes.
    pub fn set_mask(&mut self, device: &Device, queue: &Queue, id: u64, w: u32, h: u32, alpha: &[u8]) {
        if self.mask_id() == Some(id) || w == 0 || h == 0 || alpha.len() != (w * h) as usize { return; }
        let tex = mask_texture(device, w, h);
        queue.write_texture(
            TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            alpha,
            TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.bind = Self::bind_with(device, &self.layout, &self.buffer, &self.sampler, &tex);
        self.mask = Some((id, w, h));
    }

    /// Bytes its tip picture holds on the GPU.
    pub fn bytes(&self) -> u64 { self.mask.map_or(0, |(_, w, h)| u64::from(w) * u64::from(h)) }

    /// Draw `c` over `view` (`screen`: its size).
    pub fn draw(&self, queue: &Queue, enc: &mut CommandEncoder, view: &TextureView, c: &BrushCursor, screen: (u32, u32)) {
        let (w, h) = self.mask.map_or((1, 1), |(_, w, h)| (w, h));
        // −1..1 of the tip -> inside the clear border.
        let (sx, sy) = ((w as f32 - 2.0).max(1.0) / w as f32, (h as f32 - 2.0).max(1.0) / h as f32);
        let mode = if c.radius < MIN_OUTLINE_RADIUS { 2.0 } else if c.shape == CursorShape::Mask { 1.0 } else { 0.0 };
        let u = Uniform {
            center: [c.center.0, c.center.1],
            screen: [screen.0 as f32, screen.1 as f32],
            to_tip: c.to_tip,
            mask_scale: [sx, sy],
            mask_offset: [1.0 / w as f32, 1.0 / h as f32],
            extent: c.extent.max(MIN_OUTLINE_RADIUS),
            mode,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[u]));
        let mut pass = crate::over_pass(enc, view, "cursor_pass");
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..6, 0..1);
    }
}

fn mask_texture(device: &Device, w: u32, h: u32) -> Texture {
    device.create_texture(&TextureDescriptor {
        label: Some("cursor_mask"),
        size: Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: TextureFormat::R8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    })
}
