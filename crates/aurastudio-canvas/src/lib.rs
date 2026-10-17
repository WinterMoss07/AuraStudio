use wgpu::*;

pub mod blend;
pub use blend::BlendMode;

pub type Rect = (u32, u32, u32, u32); // x0, y0, x1, y1 (exclusive)

/// Anything the brush engines can paint on (today: the active layer).
/// pixels / stroke_base / stroke_mask are width*height*4, width*height*4
/// and width*height bytes. surfaces_mut() returns all three at once to
/// avoid overlapping mutable/immutable borrows in the brush engine.
pub trait PaintSurface {
    fn width(&self)  -> u32;
    fn height(&self) -> u32;
    /// (pixels, stroke_base, stroke_mask)
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]);
}

pub struct Canvas {
    pub texture:     Texture,
    pub view:        TextureView,
    pub sampler:     Sampler,
    /// Composite of all layers, uploaded to the GPU texture (dirty rect).
    pub pixels:      Vec<u8>,
    pub width:       u32,
    pub height:      u32,
    pub dirty_rect:  Option<Rect>,
}

impl Canvas {
    pub const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

    pub fn new(device: &Device, queue: &Queue, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label:           Some("canvas"),
            size:            Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       TextureDimension::D2,
            format:          Self::FORMAT,
            usage:           TextureUsages::TEXTURE_BINDING
                           | TextureUsages::COPY_DST
                           | TextureUsages::RENDER_ATTACHMENT,
            view_formats:    &[],
        });

        let pixels = vec![255u8; (width * height * 4) as usize];

        queue.write_texture(
            ImageCopyTexture {
                texture:   &texture,
                mip_level: 0,
                origin:    Origin3d::ZERO,
                aspect:    TextureAspect::All,
            },
            &pixels,
            ImageDataLayout {
                offset:         0,
                bytes_per_row:  Some(width * 4),
                rows_per_image: Some(height),
            },
            Extent3d { width, height, depth_or_array_layers: 1 },
        );

        let view    = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("canvas_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            ..Default::default()
        });

        Self { texture, view, sampler, pixels, width, height, dirty_rect: None }
    }

    pub fn mark_dirty(&mut self, rect: Rect) {
        self.dirty_rect = Some(match self.dirty_rect {
            None    => rect,
            Some(d) => (
                d.0.min(rect.0), d.1.min(rect.1),
                d.2.max(rect.2), d.3.max(rect.3),
            ),
        });
    }
}

