use wgpu::*;

pub mod blend;
pub use blend::BlendMode;

/// x0, y0, x1, y1 in pixels (x1 / y1 exclusive).
pub type Rect = (u32, u32, u32, u32);

/// Anything the brush engines can paint on (a paint layer).
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
    pub width:       u32,
    pub height:      u32,
}

impl Canvas {
    pub const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

    pub fn new(device: &Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label:           Some("canvas"),
            size:            Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count:    1,
            dimension:       TextureDimension::D2,
            format:          Self::FORMAT,
            usage:           TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats:    &[],
        });

        // No initial upload: wgpu zero-fills new textures, and the app
        // uploads the real composite before the first frame. (It used to
        // build a full white canvas on the CPU here: 16 MB at 2048 px, up
        // to 1 GB at 16384 px, for nothing.)
        let view    = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("canvas_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            ..Default::default()
        });

        Self { texture, view, sampler, width, height }
    }
}

