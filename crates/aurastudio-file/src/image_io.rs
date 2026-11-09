//! Pictures: opened as a new stack, the merged image exported, thumbnails.

use std::io::{self, Write};
use std::path::Path;

use aurastudio_core::{Rect, Rgba8};
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

/// Rows written at a time when a picture becomes a surface: no second
/// full-size RGBA copy of an RGB or grey picture.
const BAND: u32 = 256;

/// Open an image (PNG, JPG...) as a new stack with tiles of `tile`: the
/// picture as the bottom layer named after the file, an empty layer
/// above it active. Returns the name and the stack.
pub fn import_image(path: &Path, max_side: u32, tile: u32) -> Result<(String, LayerStack), String> {
    aurastudio_core::zone!("import image");
    let mut reader = image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(max_side);
    limits.max_image_height = Some(max_side);
    limits.max_alloc = Some(max_side as u64 * max_side as u64 * 4 * 2);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => format!("image bigger than this GPU allows ({max_side} px)"),
        e => e.to_string(),
    })?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 { return Err("empty image".into()); }
    let name = crate::doc_name(path);
    let mut surface = Image::new(w, h, tile, LayerStack::clear_pixel(false));
    for y0 in (0..h).step_by(BAND as usize) {
        let y1 = (y0 + BAND).min(h);
        let rect = Rect::from_xywh(0, y0 as i32, w, y1 - y0);
        match img.as_rgba8() {
            Some(rgba) => {
                let row = w as usize * 4;
                surface.write(rect, rgba.as_raw()[y0 as usize * row..y1 as usize * row].as_chunks::<4>().0);
            }
            None => surface.write(rect, img.crop_imm(0, y0, w, y1 - y0).to_rgba8().as_raw().as_chunks::<4>().0),
        }
    }
    drop(img);
    surface.compact(surface.rect().snapped_out(tile));
    let stack = LayerStack::new(w, h, tile, Start::Image { name: name.clone(), surface });
    Ok((name, stack))
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
