use wgpu::*;

pub type Rect = (u32, u32, u32, u32); // x0, y0, x1, y1 (exclusive)

pub struct Canvas {
    pub texture:     Texture,
    pub view:        TextureView,
    pub sampler:     Sampler,
    pub pixels:      Vec<u8>, // état affiché / uploadé au GPU
    pub stroke_base: Vec<u8>, // snapshot du canvas avant le trait en cours
    pub stroke_mask: Vec<u8>, // couverture 0-255 du trait en cours (max-accumulate)
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

        let stroke_base = pixels.clone();
        let stroke_mask = vec![0u8; (width * height) as usize];

        Self {
            texture, view, sampler, pixels,
            stroke_base, stroke_mask,
            width, height, dirty_rect: None,
        }
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

    /// À appeler quand un nouveau trait commence (pointer/souris down).
    /// Fige l'état du canvas comme base, et vide le masque de couverture
    /// pour que le prochain trait ne se mélange pas avec le précédent.
    pub fn begin_stroke(&mut self) {
        self.stroke_base.copy_from_slice(&self.pixels);
        self.stroke_mask.iter_mut().for_each(|m| *m = 0);
    }
}
