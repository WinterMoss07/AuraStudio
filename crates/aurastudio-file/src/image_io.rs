//! Pictures: opened as a new stack, the merged image exported, thumbnails.

use std::io::{self, Write};
use std::path::Path;

use aurastudio_core::Rgba8;
use aurastudio_layers::{LayerStack, Start};
use aurastudio_raster::Image;

use crate::limits::{EXTENSION, PREVIEW_MAX};

pub const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff", "gif"];
pub const EXPORT_EXTENSIONS: [&str; 3] = ["png", "jpg", "jpeg"];

fn ext_of(path: &Path) -> String { path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default() }

/// Can Open read this file (by its extension)?
pub fn can_open(path: &Path) -> bool {
    let ext = ext_of(path);
    ext == EXTENSION || IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Rows of a PNG decoded before they become tiles (the next band is
/// decoded while those are made).
const BAND: u32 = 256;

/// Open an image (PNG, JPG...) as a new stack with tiles of `tile`: the
/// picture as the bottom layer named after the file, an empty layer
/// above it active. Returns the name and the stack.
/// A PNG is decoded row by row into the tiles (no copy of the whole
/// picture); other formats are decoded whole, then made into tiles on all
/// cores straight from the decoded pixels (no RGBA copy).
pub fn import_image(path: &Path, max_side: u32, tile: u32) -> Result<(String, LayerStack), String> {
    aurastudio_core::zone!("import image");
    let name = crate::doc_name(path);
    let surface = match import_png(path, max_side, tile)? {
        Some(s) => s,
        None => import_decoded(path, max_side, tile)?,
    };
    let (w, h) = (surface.width(), surface.height());
    let stack = LayerStack::new(w, h, tile, Start::Image { name: name.clone(), surface });
    Ok((name, stack))
}

/// The resolution an image file says it has, dots per inch: a PNG's pHYs
/// chunk, a JPEG's JFIF header. None when it says none (or only a ratio).
pub fn image_dpi(path: &Path) -> Option<f32> {
    use std::io::Read;
    // The header is near the start: no need to read the whole file.
    let mut head = Vec::new();
    std::fs::File::open(path).ok()?.take(1 << 16).read_to_end(&mut head).ok()?;
    dpi_of(&head)
}

fn dpi_of(b: &[u8]) -> Option<f32> {
    let be32 = |s: &[u8]| u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
    let dpi = if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        // Chunks: length, type, data, CRC; pHYs comes before the pixels.
        let mut at = 8;
        loop {
            let c = b.get(at..at + 8)?;
            let (len, kind) = (be32(c) as usize, &c[4..8]);
            if kind == b"IDAT" || kind == b"IEND" { return None; }
            if kind == b"pHYs" {
                let d = b.get(at + 8..at + 17)?;
                // Unit 1: per meter; 0: a ratio only.
                if d[8] != 1 { return None; }
                break be32(d) as f32 * 0.0254;
            }
            at += 12 + len;
        }
    } else if b.starts_with(&[0xFF, 0xD8]) {
        // Segments: FF, marker, length (with itself); JFIF is APP0.
        let mut at = 2;
        loop {
            let s = b.get(at..at + 4)?;
            if s[0] != 0xFF || s[1] == 0xDA { return None; }
            let len = u16::from_be_bytes([s[2], s[3]]) as usize;
            if s[1] == 0xE0 {
                let d = b.get(at + 4..at + 2 + len)?;
                if d.len() >= 12 && d.starts_with(b"JFIF\0") {
                    let x = u16::from_be_bytes([d[8], d[9]]) as f32;
                    match d[7] {
                        1 => break x,
                        2 => break x * 2.54,
                        _ => return None,
                    }
                }
            }
            at += 2 + len;
        }
    } else {
        return None;
    };
    // (Rounded: 2834 dots per meter is 72 dpi.)
    let dpi = dpi.round();
    (1.0..=10_000.0).contains(&dpi).then_some(dpi)
}

fn too_big(max_side: u32) -> String { format!("image bigger than this GPU allows ({max_side} px)") }

/// A PNG decoded by bands into tiles. `None`: not a PNG, or an interlaced
/// one (decoded whole).
fn import_png(path: &Path, max_side: u32, tile: u32) -> Result<Option<Image>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_err() || magic != *b"\x89PNG\r\n\x1a\n" { return Ok(None); }
    let mut dec = png::Decoder::new(io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let (w, h, interlaced) = { let i = reader.info(); (i.width, i.height, i.interlaced) };
    if interlaced { return Ok(None); }
    if w == 0 || h == 0 { return Err("empty image".into()); }
    if w > max_side || h > max_side { return Err(too_big(max_side)); }
    let (color, _) = reader.output_color_type();
    let to_rgba: fn(&[u8], &mut [Rgba8]) = match color {
        png::ColorType::Rgba => |s, d| { for (d, s) in d.iter_mut().zip(s.as_chunks::<4>().0) { *d = *s; } },
        png::ColorType::Rgb => |s, d| { for (d, s) in d.iter_mut().zip(s.as_chunks::<3>().0) { *d = [s[0], s[1], s[2], 255]; } },
        png::ColorType::GrayscaleAlpha => |s, d| { for (d, s) in d.iter_mut().zip(s.as_chunks::<2>().0) { *d = [s[0], s[0], s[0], s[1]]; } },
        png::ColorType::Grayscale => |s, d| { for (d, s) in d.iter_mut().zip(s) { *d = [*s, *s, *s, 255]; } },
        png::ColorType::Indexed => return Ok(None),
    };
    let mut surface = Image::new(w, h, tile, LayerStack::clear_pixel(false));
    let band = BAND.max(surface.tile_size());
    let wu = w as usize;
    let read = |reader: &mut png::Reader<_>, y0: u32| -> Result<Vec<Rgba8>, String> {
        let n = band.min(h - y0) as usize;
        let mut out = vec![[0u8; 4]; wu * n];
        for r in 0..n {
            let row = reader.next_row().map_err(|e| e.to_string())?.ok_or("PNG cut short")?;
            to_rgba(row.data(), &mut out[r * wu..(r + 1) * wu]);
        }
        Ok(out)
    };
    // Tiles of one band made while the next one is decoded.
    let mut next = Some(read(&mut reader, 0)?);
    let mut y0 = 0;
    while let Some(rows) = next.take() {
        let after = y0 + band;
        let (decoded, ()) = rayon::join(
            || if after < h { read(&mut reader, after).map(Some) } else { Ok(None) },
            || surface.write_band(y0, &rows),
        );
        next = decoded?;
        y0 = after;
    }
    Ok(Some(surface))
}

/// Any picture the image crate reads, decoded whole, then made into tiles
/// on all cores from its own pixels.
fn import_decoded(path: &Path, max_side: u32, tile: u32) -> Result<Image, String> {
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits.max_alloc = Some(max_side as u64 * max_side as u64 * 4 * 2);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => too_big(max_side),
        e => e.to_string(),
    })?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 { return Err("empty image".into()); }
    // Rare forms (16-bit, float): one RGBA copy first.
    let img = match img {
        image::DynamicImage::ImageRgb8(_) | image::DynamicImage::ImageRgba8(_) | image::DynamicImage::ImageLuma8(_) | image::DynamicImage::ImageLumaA8(_) => img,
        other => image::DynamicImage::ImageRgba8(other.to_rgba8()),
    };
    let mut surface = Image::new(w, h, tile, LayerStack::clear_pixel(false));
    let wu = w as usize;
    surface.fill_frame(|y0, out| {
        let o = y0 as usize * wu;
        match &img {
            image::DynamicImage::ImageRgb8(b) => for (d, s) in out.iter_mut().zip(b.as_raw()[o * 3..].as_chunks::<3>().0) { *d = [s[0], s[1], s[2], 255]; },
            image::DynamicImage::ImageRgba8(b) => for (d, s) in out.iter_mut().zip(b.as_raw()[o * 4..].as_chunks::<4>().0) { *d = *s; },
            image::DynamicImage::ImageLuma8(b) => for (d, s) in out.iter_mut().zip(&b.as_raw()[o..]) { *d = [*s, *s, *s, 255]; },
            image::DynamicImage::ImageLumaA8(b) => for (d, s) in out.iter_mut().zip(b.as_raw()[o * 2..].as_chunks::<2>().0) { *d = [s[0], s[0], s[0], s[1]]; },
            _ => {}
        }
    });
    Ok(surface)
}

/// Export the merged image (`merged`: frame-sized straight RGBA) to PNG or
/// JPG, chosen by the file extension.
pub fn export_image(merged: &[Rgba8], width: u32, height: u32, path: &Path) -> Result<(), String> {
    use image::ImageEncoder;
    let jpeg = match ext_of(path).as_str() {
        "jpg" | "jpeg" => true,
        "png" => false,
        // (Checked before the file is made: no empty file left behind.)
        ext => return Err(format!("can't export .{ext} (PNG or JPG)")),
    };
    if merged.len() != width as usize * height as usize { return Err("wrong image size".into()); }
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut out = io::BufWriter::new(file);
    if jpeg {
        // (JPEG has no alpha: the colors as they are.)
        let rgb: Vec<u8> = merged.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).write_image(&rgb, width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    } else {
        image::codecs::png::PngEncoder::new(&mut out).write_image(merged.as_flattened(), width, height, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}

/// Small picture of a file for the start screen, at most `max` px: the
/// preview inside an .aurs file, or the image itself shrunk.
pub fn thumbnail(path: &Path, max: u32) -> Option<(u32, u32, Vec<Rgba8>)> {
    let img = if ext_of(path) == EXTENSION { crate::read::preview_of(path)? } else {
        let mut reader = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(1 << 30);
        reader.limits(limits);
        reader.decode().ok()?
    };
    let img = img.into_rgba8();
    let (w, h) = img.dimensions();
    Some(downscale(img.as_raw().as_chunks::<4>().0, w, h, max))
}

/// `thumbnail`, kept as a small PNG in `dir`: read again from there while
/// the file is unchanged (same size and time), not decoded whole.
pub fn thumbnail_cached(path: &Path, max: u32, dir: &Path) -> Option<(u32, u32, Vec<Rgba8>)> {
    let kept = dir.join(format!("{:016x}.png", thumbnail_key(path, max)?));
    if let Some(img) = image::open(&kept).ok().map(image::DynamicImage::into_rgba8) {
        let (w, h) = img.dimensions();
        return Some((w, h, img.into_raw().as_chunks::<4>().0.to_vec()));
    }
    let (w, h, px) = thumbnail(path, max)?;
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = image::save_buffer(&kept, px.as_flattened(), w, h, image::ExtendedColorType::Rgba8);
    }
    Some((w, h, px))
}

/// Remove the kept thumbnails of files not in `paths`.
pub fn trim_thumbnails(dir: &Path, paths: &[std::path::PathBuf], max: u32) {
    let keep: Vec<String> = paths.iter().filter_map(|p| thumbnail_key(p, max)).map(|k| format!("{k:016x}.png")).collect();
    let Ok(list) = std::fs::read_dir(dir) else { return };
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !keep.contains(&name) { let _ = std::fs::remove_file(e.path()); }
    }
}

/// What names a file's thumbnail: its path, size and time, and `max`.
fn thumbnail_key(path: &Path, max: u32) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let m = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (path, m.len(), m.modified().ok(), max).hash(&mut h);
    Some(h.finish())
}

/// `px` (w × h) shrunk to fit in `max` × `max`, averaging the pixels.
pub fn downscale(px: &[Rgba8], w: u32, h: u32, max: u32) -> (u32, u32, Vec<Rgba8>) {
    if w == 0 || h == 0 || px.len() != w as usize * h as usize { return (0, 0, Vec::new()); }
    let k = (max.max(1) as f32 / w.max(h) as f32).min(1.0);
    let (tw, th) = (((w as f32 * k).round() as u32).clamp(1, w), ((h as f32 * k).round() as u32).clamp(1, h));
    let mut out = Vec::with_capacity(tw as usize * th as usize);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let mut sum = [0u32; 4];
            for y in y0..y1 {
                for p in &px[(y * w + x0) as usize..(y * w + x1) as usize] {
                    for c in 0..4 { sum[c] += p[c] as u32; }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            out.push(sum.map(|s| ((s + n / 2) / n) as u8));
        }
    }
    (tw, th, out)
}

/// The preview a save stores: the merged image shrunk to `PREVIEW_MAX`.
pub fn preview(merged: &[Rgba8], w: u32, h: u32) -> (u32, u32, Vec<Rgba8>) { downscale(merged, w, h, PREVIEW_MAX) }

#[cfg(test)]
mod dpi_tests {
    use super::dpi_of;

    #[test]
    fn a_png_or_jpeg_says_its_resolution() {
        // PNG: signature, an IHDR, then pHYs at 11811 dots per meter (300 dpi).
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend([0, 0, 0, 13]);
        png.extend(b"IHDR");
        png.extend([0; 13 + 4]);
        png.extend([0, 0, 0, 9]);
        png.extend(b"pHYs");
        png.extend(11811u32.to_be_bytes());
        png.extend(11811u32.to_be_bytes());
        png.extend([1, 0, 0, 0, 0]);
        assert_eq!(dpi_of(&png), Some(300.0));
        // A ratio only: nothing.
        let n = png.len();
        png[n - 5] = 0;
        assert_eq!(dpi_of(&png), None);
        // JPEG: JFIF at 150 dpi; then in dots per cm.
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        jpg.extend(b"JFIF\0");
        jpg.extend([1, 2, 1, 0, 150, 0, 150, 0, 0]);
        assert_eq!(dpi_of(&jpg), Some(150.0));
        jpg[13] = 2;
        jpg[15] = 118;
        assert_eq!(dpi_of(&jpg), Some(300.0));
        assert_eq!(dpi_of(b"GIF89a"), None);
    }
}
