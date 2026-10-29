//! Pixels sent to GPU textures through a ring of staging buffers that are
//! kept and reused.
//!
//! `queue.write_texture` makes a fresh staging buffer for every call and
//! frees it once the GPU has copied it. On Vulkan (Intel driver at least)
//! that host memory was not given back: a whole canvas sent again (a layer
//! shown / hidden) left +16 MB of private bytes for good, where GL gave
//! everything back. As Krita does with its ring of pixel buffers, the
//! staging memory here is made once, in chunks of `CHUNK`, and reused:
//! no buffer is made or freed per upload. At most `LIMIT` bytes wait for
//! the GPU; past that, uploads wait for it, so the ring stays small.

use wgpu::util::StagingBelt;
use wgpu::*;

/// Staging bytes allowed to wait for the GPU (the ring's size, about).
pub const LIMIT: u64 = 4 << 20;
/// Size of one staging buffer of the ring.
pub const CHUNK: u64 = 1 << 20;

#[derive(Default)]
pub struct Uploads {
    /// The ring (made on the first upload).
    belt:   Option<StagingBelt>,
    /// Copies recorded and not yet submitted.
    enc:    Option<CommandEncoder>,
    /// Staging bytes used since the GPU was last waited for.
    queued: u64,
    /// Waits made (for the tests and the report).
    pub flushes: u64,
}

impl Uploads {
    /// Write `rgba` (rows of `size`, tightly packed) at `origin` of level 0
    /// of `texture`, in pieces of at most one chunk. The copies are
    /// submitted before this returns (ahead of the next frame).
    #[profiling::function]
    pub fn write(&mut self, device: &Device, queue: &Queue, texture: &Texture, origin: (u32, u32), size: (u32, u32), rgba: &[u8]) {
        let (w, h) = size;
        if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 { return; }
        let row = w as usize * 4;
        let padded = row.next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let band = ((CHUNK as usize / padded).max(1) as u32).min(h);
        let mut y = 0;
        while y < h {
            let rows = band.min(h - y);
            let bytes = (padded * rows as usize) as u64;
            if self.queued > 0 && self.queued + bytes > LIMIT { self.flush(device, queue); }
            let belt = self.belt.get_or_insert_with(|| StagingBelt::new(device.clone(), CHUNK));
            let enc = self.enc.get_or_insert_with(|| device.create_command_encoder(&CommandEncoderDescriptor { label: Some("uploads") }));
            let slice = belt.allocate(BufferSize::new(bytes).unwrap(), BufferSize::new(COPY_BYTES_PER_ROW_ALIGNMENT as u64).unwrap());
            {
                let Ok(mut view) = slice.get_mapped_range_mut() else { return };
                let src = &rgba[y as usize * row..(y + rows) as usize * row];
                if padded == row {
                    view.copy_from_slice(src);
                } else {
                    for r in 0..rows as usize {
                        view.slice(r * padded..r * padded + row).copy_from_slice(&src[r * row..(r + 1) * row]);
                    }
                }
            }
            enc.copy_buffer_to_texture(
                TexelCopyBufferInfo { buffer: slice.buffer(), layout: TexelCopyBufferLayout { offset: slice.offset(), bytes_per_row: Some(padded as u32), rows_per_image: Some(rows) } },
                TexelCopyTextureInfo { texture, mip_level: 0, origin: Origin3d { x: origin.0, y: origin.1 + y, z: 0 }, aspect: TextureAspect::All },
                Extent3d { width: w, height: rows, depth_or_array_layers: 1 },
            );
            self.queued += bytes;
            y += rows;
        }
        self.submit(queue);
    }

    /// Submit the recorded copies; their chunks come back to the ring
    /// once the GPU has done them.
    fn submit(&mut self, queue: &Queue) {
        let (Some(enc), Some(belt)) = (self.enc.take(), self.belt.as_mut()) else { return };
        belt.finish_and_recall_on_submit(&enc);
        queue.submit([enc.finish()]);
    }

    /// Submit what waits and let the GPU finish it: its chunks are free
    /// again.
    pub fn flush(&mut self, device: &Device, queue: &Queue) {
        self.submit(queue);
        if self.queued == 0 { return; }
        let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
        self.queued = 0;
        self.flushes += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6 MB picture goes in bands with waits in between, and arrives
    /// whole (read back from the texture).
    #[test]
    fn a_big_upload_goes_in_bands_and_arrives_whole() {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
        let (w, h) = (1024u32, 1500u32);
        let px: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        let tex = device.create_texture(&TextureDescriptor {
            label: None, size: Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: TextureDimension::D2, format: TextureFormat::Rgba8Unorm, usage: TextureUsages::COPY_DST | TextureUsages::COPY_SRC, view_formats: &[],
        });
        let mut up = Uploads::default();
        up.write(&device, &queue, &tex, (0, 0), (w, h), &px);
        assert!(up.flushes >= 1, "6 MB with a 4 MB limit waits at least once");
        assert!(up.queued <= LIMIT);
        let read = device.create_buffer(&BufferDescriptor { label: None, size: (w * h * 4) as u64, usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor::default());
        enc.copy_texture_to_buffer(
            TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyBufferInfo { buffer: &read, layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        read.slice(..).map_async(MapMode::Read, |_| {});
        let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
        assert!(*read.slice(..).get_mapped_range().unwrap() == px[..]);
    }

    /// Sending a whole canvas again and again reuses the same staging
    /// buffers: the ring does not grow (a layer shown / hidden many times
    /// used to leave fresh driver memory each time on Vulkan).
    #[test]
    fn repeated_uploads_reuse_the_ring() {
        let instance = Instance::new(InstanceDescriptor { backends: Backends::VULKAN, ..InstanceDescriptor::new_without_display_handle() });
        let Ok(adapter) = pollster::block_on(instance.request_adapter(&RequestAdapterOptions::default())) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();
        let (w, h) = (1920u32, 1080u32);
        let px = vec![200u8; (w * h * 4) as usize];
        let tex = device.create_texture(&TextureDescriptor {
            label: None, size: Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: TextureDimension::D2, format: TextureFormat::Rgba8Unorm, usage: TextureUsages::COPY_DST, view_formats: &[],
        });
        let staging = |d: &Device| d.generate_allocator_report().map(|r| r.allocations.iter().filter(|a| a.name.contains("StagingBelt")).count());
        let mut up = Uploads::default();
        up.write(&device, &queue, &tex, (0, 0), (w, h), &px);
        let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
        let first = staging(&device);
        for _ in 0..10 {
            up.write(&device, &queue, &tex, (0, 0), (w, h), &px);
            let _ = device.poll(PollType::Wait { submission_index: None, timeout: None });
        }
        assert_eq!(staging(&device), first, "the ring keeps the same buffers");
        if let Some(n) = first { assert!(n > 0, "the ring is listed"); assert!(n as u64 <= LIMIT / CHUNK + 1, "{n} staging buffers"); }
    }
}
