//! Canvas mipmaps (thin lines stay visible zoomed out), rebuilt on the GPU
//! only where the canvas changed; 2×2 averages of the stored values.

use aurastudio_canvas::{Canvas, Rect};
use wgpu::*;

pub struct MipGen {
    pipeline: RenderPipeline,
    layout:   BindGroupLayout,
    sampler:  Sampler,
    /// Per level: its own view (render target), and a bind group reading
    /// the level above it.
    levels:   Vec<(TextureView, Option<BindGroup>)>,
    /// Areas of level 0 changed since the last rebuild.
    dirty:    Vec<Rect>,
}

const SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the target.
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Target texel (x, y) covers source texels 2x..2x+1: sample their
    // shared corner, the bilinear filter averages the four.
    let size = vec2<f32>(textureDimensions(src));
    return textureSampleLevel(src, samp, (pos.xy * 2.0) / size, 0.0);
}
"#;

impl MipGen {
    pub fn new(device: &Device, canvas: &Canvas) -> Self {
        let module = device.create_shader_module(ShaderModuleDescriptor {
            label:  Some("mip"),
            source: ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("mip"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0, visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: true }, view_dimension: TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                BindGroupLayoutEntry { binding: 1, visibility: ShaderStages::FRAGMENT, ty: BindingType::Sampler(SamplerBindingType::Filtering), count: None },
            ],
        });
        let pl = device.create_pipeline_layout(&PipelineLayoutDescriptor { label: Some("mip"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label:  Some("mip"),
            layout: Some(&pl),
            vertex: VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(FragmentState {
                module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(ColorTargetState { format: Canvas::FORMAT, blend: None, write_mask: ColorWrites::ALL })],
            }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("mip"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let mut g = Self { pipeline, layout, sampler, levels: Vec::new(), dirty: Vec::new() };
        g.attach(device, canvas);
        g
    }

    /// A new canvas texture (resize): rebuild the views, everything dirty.
    pub fn attach(&mut self, device: &Device, canvas: &Canvas) {
        let n = canvas.texture.mip_level_count();
        let view = |l: u32| canvas.texture.create_view(&TextureViewDescriptor {
            base_mip_level: l, mip_level_count: Some(1), ..Default::default()
        });
        self.levels = (0..n).map(|l| {
            let bind = (l > 0).then(|| device.create_bind_group(&BindGroupDescriptor {
                label:   Some("mip"),
                layout:  &self.layout,
                entries: &[
                    BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&view(l - 1)) },
                    BindGroupEntry { binding: 1, resource: BindingResource::Sampler(&self.sampler) },
                ],
            }));
            (view(l), bind)
        }).collect();
        self.dirty = vec![(0, 0, canvas.width, canvas.height)];
    }

    /// `rect` of level 0 changed.
    pub fn mark(&mut self, rect: Rect) {
        // A few rectangles are enough; past that, one box around them.
        if self.dirty.len() >= 16 {
            let all = self.dirty.iter().fold(rect, |a, r| (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3)));
            self.dirty.clear();
            self.dirty.push(all);
        } else {
            self.dirty.push(rect);
        }
    }

    /// Rebuild the changed areas of every level, into `enc`.
    pub fn rebuild(&mut self, enc: &mut CommandEncoder, canvas: &Canvas) {
        if self.dirty.is_empty() { return; }
        let dirty = std::mem::take(&mut self.dirty);
        for (l, (target, bind)) in self.levels.iter().enumerate().skip(1) {
            let Some(bind) = bind else { continue };
            let (w, h) = ((canvas.width >> l).max(1), (canvas.height >> l).max(1));
            let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
                label: Some("mip"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: target, resolve_target: None,
                    ops: Operations { load: LoadOp::Load, store: StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bind, &[]);
            for &(x0, y0, x1, y1) in &dirty {
                // This level's texels touched by the change (rounded out).
                let (sx0, sy0) = (x0 >> l, y0 >> l);
                let (sx1, sy1) = (((x1 + (1 << l) - 1) >> l).min(w), ((y1 + (1 << l) - 1) >> l).min(h));
                if sx0 >= sx1 || sy0 >= sy1 { continue; }
                pass.set_scissor_rect(sx0, sy0, sx1 - sx0, sy1 - sy0);
                pass.draw(0..3, 0..1);
            }
        }
    }
}

