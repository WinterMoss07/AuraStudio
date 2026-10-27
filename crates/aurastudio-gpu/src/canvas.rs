use wgpu::*;

pub struct Canvas {
    pub texture:     Texture,
    pub view:        TextureView,
    pub sampler:     Sampler,
    pub width:       u32,
    pub height:      u32,
}

impl Canvas {
    pub const FORMAT: TextureFormat = TextureFormat::Rgba8Unorm;

    /// `mipmaps`: the whole chain down to 1 px (built by aurastudio-gpu;
    /// a third more memory), else the pixels alone.
    pub fn new(device: &Device, width: u32, height: u32, mipmaps: bool) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label:           Some("canvas"),
            size:            Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: if mipmaps { 32 - width.max(height).max(1).leading_zeros() } else { 1 },
            sample_count:    1,
            dimension:       TextureDimension::D2,
            format:          Self::FORMAT,
            // (COPY_SRC: copied whole into a new texture when the display
            // quality changes.)
            usage:           TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT,
            view_formats:    &[],
        });

        // No initial upload: the app uploads the merge before the first frame.
        let view    = texture.create_view(&TextureViewDescriptor::default());
        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("canvas_sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            mipmap_filter:  wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        Self { texture, view, sampler, width, height }
    }
}
