//! Pixels sent to GPU textures with a ceiling on the staging memory.
//!
//! wgpu copies every `write_texture` into a staging buffer that lives until
//! the GPU has done the copy, after the next submit. A whole canvas sent in
//! one go (opening a 2000 × 3000 picture: 24 MB) had all of it alive at
//! once, in fresh driver memory blocks. Here, once `LIMIT` bytes wait, they
//! are submitted and waited for, so the next ones reuse the same memory.

use wgpu::*;

/// Staging bytes allowed to wait for the GPU.
pub const LIMIT: u64 = 4 << 20;

#[derive(Default)]
pub struct Uploads {
    /// Bytes written since the last submit.
    queued: u64,
    /// Waits made (for the tests and the report).
    pub flushes: u64,
}

impl Uploads {
    /// Write `rgba` (rows of `size`, tightly packed) at `origin` of level 0
    /// of `texture`, in bands of at most `LIMIT` bytes.
    pub fn write(&mut self, device: &Device, queue: &Queue, texture: &Texture, origin: (u32, u32), size: (u32, u32), rgba: &[u8]) {
        let (w, h) = size;
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 { return; }
        let row = w as u64 * 4;
        let band = ((LIMIT / row).max(1) as u32).min(h);
        let mut y = 0;
        while y < h {
            let rows = band.min(h - y);
            let start = (y as u64 * row) as usize;
            let end = start + (rows as u64 * row) as usize;
            queue.write_texture(
                ImageCopyTexture { texture, mip_level: 0, origin: Origin3d { x: origin.0, y: origin.1 + y, z: 0 }, aspect: TextureAspect::All },
                &rgba[start..end],
                ImageDataLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(rows) },
                Extent3d { width: w, height: rows, depth_or_array_layers: 1 },
            );
            self.queued += rows as u64 * row;
            if self.queued >= LIMIT { self.flush(device, queue); }
            y += rows;
        }
    }

    /// Submit what waits and let the GPU finish it: its staging memory is
    /// free again.
    pub fn flush(&mut self, device: &Device, queue: &Queue) {
        if self.queued == 0 { return; }
        queue.submit(std::iter::empty());
        device.poll(Maintain::Wait);
        self.queued = 0;
        self.flushes += 1;
    }

    /// A frame was submitted (its uploads went with it).
    pub fn submitted(&mut self) { self.queued = 0; }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6 MB picture goes in bands with waits in between, and arrives
    /// whole (read back from the texture).
    #[test]
    fn a_big_upload_goes_in_bands_and_arrives_whole() {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..Default::default() });
        let Some(adapter) = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default(), None)).unwrap();
        let (w, h) = (1024u32, 1500u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let tex = device.create_texture(&TextureDescriptor {
            label: None, size: Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: TextureDimension::D2, format: TextureFormat::Rgba8Unorm, usage: TextureUsages::COPY_DST | TextureUsages::COPY_SRC, view_formats: &[],
        });
        let mut up = Uploads::default();
        up.write(&device, &queue, &tex, (0, 0), (w, h), &px);
        assert!(up.flushes >= 1, "6 MB with a 4 MB limit waits at least once");
        assert!(up.queued < LIMIT);
        let read = device.create_buffer(&BufferDescriptor { label: None, size: (w * h * 4) as u64, usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            ImageCopyTexture { texture: &tex, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            ImageCopyBuffer { buffer: &read, layout: ImageDataLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        read.slice(..).map_async(MapMode::Read, |_| {});
        device.poll(Maintain::Wait);
        assert!(*read.slice(..).get_mapped_range() == px[..]);
    }
}
