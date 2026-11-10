//! Stroke previews (the interface's stroke_preview asks, this paints):
//! an S across a picture, pressed lightly, then hard, then lifting, slow
//! then fast, tilted more and more, so pressure, velocity, tilt and the
//! random parts show (a geometry tool draws its shape). The brush in use
//! is drawn over gray paper on the left half (an eraser cuts it, a smudge
//! drags it, the blend modes show) in a light ink of its own; the brush
//! list's are the stroke's alpha alone, painted once and kept on disk (one
//! small file a brush: `%LOCALAPPDATA%\AuraStudio\stroke-previews`).
//!
//! Sizes: true size up to a third of the room, then compressed (log)
//! up to 100 px, the same past it. A brush that paints past its radius
//! (scattering, size jitter) is drawn smaller so that nothing is cut.
//!
//! One thread does them all, in this order: the brush in use (only the
//! last asked of each place), the list's rows on screen, then the ones
//! not on disk yet (all of them at the first start, an import's at once).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use aurastudio_core::Rect;
use aurastudio_forge::{BrushSettings, Forge, PaintMode, PenSample, TipReader};
use aurastudio_raster::Image;
use aurastudio_ui::{PreviewShape as Shape, PreviewSlot as Slot, PREVIEW_BIGGEST as BIGGEST, PREVIEW_INK as INK, PREVIEW_LIST as LIST, PREVIEW_PAPER as PAPER};

/// Bump when a list preview would look different: those on disk under
/// another version are made again.
pub const RENDER_VERSION: u32 = 2;

/// Largest radius shown, as a share of the picture's height.
const MAX_RADIUS: f32 = 0.26;

/// Pen samples along the S.
const SAMPLES: usize = 160;

/// Diameter (pixels) a brush of diameter `d` is drawn at in a picture `h`
/// high: true size up to a third of the room, then compressed up to
/// `BIGGEST`; past it, the same.
pub fn shown_size(d: f32, h: u32) -> f32 {
    let dmax = 2.0 * h as f32 * MAX_RADIUS;
    let d0 = dmax / 3.0;
    let d = d.clamp(0.5, BIGGEST);
    if d <= d0 { return d; }
    if BIGGEST <= d0 { return d.min(dmax); }
    d0 + (dmax - d0) * ((d / d0).ln() / (BIGGEST / d0).ln()).min(1.0)
}

/// Thickness of a geometry tool's shape for a brush of diameter `d`: 1 to
/// about 10 px, logarithmic up to `BIGGEST`.
pub fn shape_thickness(d: f32) -> f32 { 1.0 + 14.0 * (d.clamp(1.0, BIGGEST).ln() / 1000f32.ln()) }

/// The preview, `w` × `h` straight RGBA. `detail`: the brush in use (gray
/// paper on the left half, the brush as it is); else the list's (every
/// brush painting, to show its stroke's shape).
pub fn render(brush: &BrushSettings, shape: Shape, detail: bool, w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w.max(8), h.max(8));
    let mut b = brush.clone();
    let [r, g, bl] = INK;
    b.color = [r as f32 / 255.0, g as f32 / 255.0, bl as f32 / 255.0, 1.0];
    b.seed = 1;
    if !detail {
        b.erase = false;
        b.behind = false;
        b.mixer.enabled = false;
        b.mode = PaintMode::Normal;
    }
    let d = match shape { Shape::S => shown_size(brush.size, h), _ => shape_thickness(brush.size) };
    b.resize_with_dual_brush(d.max(1.0));
    // Painted in a strip three times as tall: what lands outside the
    // picture's rows shows how much smaller the brush must be.
    let (px, reach, extent) = paint(&b, shape, detail, w, h);
    if reach <= 0.0 { return px; }
    let fit = ((extent - reach) / extent).clamp(0.05, 1.0) * 0.95;
    b.resize_with_dual_brush((b.size * fit).max(1.0));
    paint(&b, shape, detail, w, h).0
}

/// The stroke in the middle of a strip 3 × `h` high: the picture's rows,
/// how far (px) the paint went past them, and how far it went from the
/// path (px) where it went past most.
fn paint(b: &BrushSettings, shape: Shape, detail: bool, w: u32, h: u32) -> (Vec<u8>, f32, f32) {
    let dy = h as i32;
    let mut img = Image::new(w, h * 3, 64, [0; 4]);
    if detail {
        let [r, g, bl] = PAPER;
        img.fill_rect(Rect::new(0, dy, (w / 2) as i32, dy + h as i32), [r, g, bl, 255]);
    }
    let path = path(shape, w, h);
    {
        let state = img.stroke_state(None);
        let mut stroke = state.on(&mut img);
        let mut engine = Forge::new(b.clone());
        for s in &path { engine.stroke_to(&mut stroke, PenSample { y: s.y + dy as f32, ..*s }); }
        engine.end_stroke(&mut stroke);
        let _ = stroke.end();
    }
    let all = img.read(img.rect());
    let row_painted = |y: u32| all[(y * w) as usize..((y + 1) * w) as usize].iter().any(|p| p[3] > 8);
    let top = (0..dy as u32).find(|&y| row_painted(y));
    let bottom = ((h * 2)..(h * 3)).rev().find(|&y| row_painted(y));
    let over_top = top.map_or(0.0, |t| (dy as u32 - t) as f32);
    let over_bottom = bottom.map_or(0.0, |b| (b + 1 - h * 2) as f32);
    let (y0, y1) = path.iter().fold((f32::MAX, f32::MIN), |(a, z), s| (a.min(s.y), z.max(s.y)));
    let reach = over_top.max(over_bottom);
    // From the path's highest / lowest point to the farthest paint.
    let extent = if over_top >= over_bottom { y0 + dy as f32 - top.unwrap_or(dy as u32) as f32 } else { bottom.map_or(0.0, |b| b as f32 + 1.0) - (y1 + dy as f32) };
    let rows: Vec<u8> = all[(dy as u32 * w) as usize..((dy as u32 + h) * w) as usize].iter().flatten().copied().collect();
    (rows, reach, extent.max(reach + 1.0))
}

/// Pen samples. The S crosses the picture left to right: pressure rising
/// then falling (tapered ends), slow then fast, tilted more and more;
/// shapes are drawn at full pressure, as the geometry tools paint them.
fn path(shape: Shape, w: u32, h: u32) -> Vec<PenSample> {
    let (w, h) = (w as f32, h as f32);
    let margin = if shape == Shape::S { h * MAX_RADIUS } else { h * 0.12 } + 3.0;
    let (x0, x1, y0, y1) = (margin, w - margin, margin, h - margin);
    let (cx, cy) = ((x0 + x1) / 2.0, h / 2.0);
    let half = ((y1 - y0) * 1.6).min(x1 - x0) / 2.0;
    let tau = std::f32::consts::TAU;
    let pts: Vec<(f32, f32, f32)> = match shape {
        Shape::S => {
            let amp = (h / 2.0 - margin).max(0.0) * 0.9;
            (0..=SAMPLES).map(|i| {
                let t = i as f32 / SAMPLES as f32;
                // (sin π is a hair below 0 in f32: kept at 0, not NaN.)
                let p = 0.1 + 0.9 * (t * std::f32::consts::PI).sin().max(0.0).powf(0.8);
                (x0 + t * (x1 - x0), cy - amp * (t * tau).sin(), p)
            }).collect()
        }
        Shape::Line => polyline(&[(cx - half, y1), (cx + half, y0)]),
        Shape::Rectangle => polyline(&[(cx - half, y0), (cx + half, y0), (cx + half, y1), (cx - half, y1), (cx - half, y0)]),
        Shape::Ellipse => (0..=96).map(|i| {
            let a = i as f32 / 96.0 * tau;
            (cx + half * a.cos(), cy + (y1 - y0) / 2.0 * a.sin(), 1.0)
        }).collect(),
        Shape::Arc => (0..=64).map(|i| {
            // A quadratic curve bulging upward.
            let t = i as f32 / 64.0;
            let u = 1.0 - t;
            let (ax, bx, ky) = (cx - half, cx + half, y0 - (y1 - y0));
            (u * u * ax + 2.0 * u * t * cx + t * t * bx, u * u * y1 + 2.0 * u * t * ky + t * t * y1, 1.0)
        }).collect(),
    };
    let n = pts.len().max(2) - 1;
    let mut time = 0.0f64;
    pts.into_iter().enumerate().map(|(i, (x, y, p))| {
        let t = i as f32 / n as f32;
        // Slow at first, faster after the middle (velocity).
        time += if t < 0.5 { 0.008 } else { 0.003 };
        PenSample::new(x, y, p, time).with_pen((60.0 * t, 0.0), 0.0)
    }).collect()
}

/// Points every ~2 px along the segments, full pressure.
fn polyline(corners: &[(f32, f32)]) -> Vec<(f32, f32, f32)> {
    let mut out = vec![(corners[0].0, corners[0].1, 1.0)];
    for w in corners.windows(2) {
        let (a, b) = (w[0], w[1]);
        let n = (((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() / 2.0).ceil().max(1.0) as usize;
        out.extend((1..=n).map(|i| { let t = i as f32 / n as f32; (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, 1.0) }));
    }
    out
}

// --- The list's previews on disk -----------------------------------------------

/// A list preview's file: its brush's shape hash and the render version.
fn file_of(dir: &Path, settings: &BrushSettings) -> PathBuf {
    dir.join(format!("{:016x}.preview", key_of(settings)))
}

fn key_of(settings: &BrushSettings) -> u64 {
    settings.shape_hash() ^ (u64::from(RENDER_VERSION) << 48) ^ (u64::from(LIST.0) << 16) ^ u64::from(LIST.1)
}

/// File: "ASP1", width and height (u16 LE), the alpha compressed (LZ4).
fn write_alpha(path: &Path, w: u32, h: u32, alpha: &[u8]) -> std::io::Result<()> {
    let mut out = b"ASP1".to_vec();
    out.extend((w as u16).to_le_bytes());
    out.extend((h as u16).to_le_bytes());
    out.extend(lz4_flex::compress_prepend_size(alpha));
    // (Written aside, then renamed: never half a file under its name.)
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

fn read_alpha(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 8 || &b[..4] != b"ASP1" { return None; }
    let (w, h) = (u16::from_le_bytes([b[4], b[5]]) as u32, u16::from_le_bytes([b[6], b[7]]) as u32);
    let alpha = lz4_flex::decompress_size_prepended(&b[8..]).ok()?;
    (alpha.len() == (w * h) as usize).then_some((w, h, alpha))
}

fn alpha_of(rgba: &[u8]) -> Vec<u8> { rgba.chunks_exact(4).map(|p| p[3]).collect() }

// --- The thread ------------------------------------------------------------------

/// A finished preview, for the interface.
#[derive(Debug)]
pub enum Done {
    /// The brush in use's, straight RGBA.
    Current { slot: Slot, w: u32, h: u32, rgba: Vec<u8> },
    /// A list row's alpha (`settings`: the brush it shows).
    List { index: usize, settings: Box<BrushSettings>, w: u32, h: u32, alpha: Vec<u8> },
}

#[derive(Default)]
struct Queue {
    /// The last asked for each place (the ones before it are skipped).
    current: HashMap<Slot, (Box<BrushSettings>, Shape)>,
    rows:    VecDeque<(usize, Box<BrushSettings>)>,
    /// Brushes whose preview may not be on disk yet; `full`: all of them
    /// (the files of no brush there are deleted once done).
    missing: VecDeque<BrushSettings>,
    full:    Option<HashSet<PathBuf>>,
    quit:    bool,
}

pub struct Previews {
    shared: Arc<(Mutex<Queue>, Condvar)>,
}

impl Previews {
    /// The thread, painting with tips read by `tips`, keeping the list's in
    /// `dir` (None: painted each time, nothing kept), handing results to
    /// `send`.
    pub fn start(tips: TipReader, dir: Option<PathBuf>, send: impl Fn(Done) + Send + 'static) -> Self {
        let shared: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let s = Arc::clone(&shared);
        if let Some(d) = &dir { let _ = std::fs::create_dir_all(d); }
        let spawned = std::thread::Builder::new().name("stroke previews".into()).spawn(move || work(&s, &tips, dir.as_deref(), &send));
        if let Err(e) = spawned { log::warn!("stroke previews: no thread ({e})"); }
        Self { shared }
    }

    fn queue(&self, f: impl FnOnce(&mut Queue)) {
        let (lock, wake) = &*self.shared;
        f(&mut lock.lock().unwrap_or_else(PoisonError::into_inner));
        wake.notify_one();
    }

    pub fn ask(&self, ask: aurastudio_ui::PreviewAsk) {
        use aurastudio_ui::PreviewAsk as A;
        self.queue(|q| match ask {
            A::Current { slot, brush, shape } => { q.current.insert(slot, (brush, shape)); }
            A::List { index, settings } => {
                q.rows.retain(|r| r.0 != index);
                q.rows.push_back((index, settings));
            }
        });
    }

    /// Every brush of the library (at the start, after an import): those
    /// not on disk are painted, the files of brushes gone are deleted.
    pub fn keep_all(&self, all: impl IntoIterator<Item = BrushSettings>) {
        self.queue(|q| {
            q.missing = all.into_iter().collect();
            q.full = Some(HashSet::new());
        });
    }
}

impl Drop for Previews {
    fn drop(&mut self) { self.queue(|q| q.quit = true); }
}

enum Next {
    Current(Slot, Box<BrushSettings>, Shape),
    Row(usize, Box<BrushSettings>),
    Missing(BrushSettings),
    Sweep(HashSet<PathBuf>),
}

fn work(shared: &(Mutex<Queue>, Condvar), tips: &TipReader, dir: Option<&Path>, send: &dyn Fn(Done)) {
    let (lock, wake) = shared;
    loop {
        let next = {
            let mut q = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if q.quit { return; }
                if let Some(slot) = q.current.keys().next().copied() {
                    let (b, s) = q.current.remove(&slot).expect("listed");
                    break Next::Current(slot, b, s);
                }
                if let Some((i, b)) = q.rows.pop_front() { break Next::Row(i, b); }
                if let Some(b) = q.missing.pop_front() { break Next::Missing(b); }
                if let Some(keep) = q.full.take() { break Next::Sweep(keep); }
                q = wake.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        match next {
            Next::Current(slot, b, shape) => {
                let (w, h) = slot.size();
                send(Done::Current { slot, w, h, rgba: render(&b, shape, true, w, h) });
            }
            Next::Row(index, settings) => {
                let (w, h, alpha) = list_preview(&settings, tips, dir);
                send(Done::List { index, settings, w, h, alpha });
            }
            Next::Missing(settings) => {
                let Some(d) = dir else { continue };
                let path = file_of(d, &settings);
                if !path.is_file() { let _ = list_preview(&settings, tips, dir); }
                let mut q = lock.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(keep) = &mut q.full { keep.insert(path); }
            }
            Next::Sweep(keep) => {
                let Some(d) = dir else { continue };
                let Ok(files) = std::fs::read_dir(d) else { continue };
                for f in files.flatten() {
                    let p = f.path();
                    if !keep.contains(&p) { let _ = std::fs::remove_file(&p); }
                }
            }
        }
    }
}

/// A list row's preview: read from disk, or painted (and kept).
fn list_preview(settings: &BrushSettings, tips: &TipReader, dir: Option<&Path>) -> (u32, u32, Vec<u8>) {
    let path = dir.map(|d| file_of(d, settings));
    if let Some(got) = path.as_deref().and_then(read_alpha) { return got; }
    let mut b = settings.clone();
    tips.load(&mut b);
    let (w, h) = LIST;
    let alpha = alpha_of(&render(&b, Shape::S, false, w, h));
    if let Some(p) = &path {
        if let Err(e) = write_alpha(p, w, h, &alpha) { log::warn!("stroke preview not kept: {e}"); }
    }
    (w, h, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(rgba: &[u8]) -> usize { rgba.chunks(4).filter(|p| p[3] > 0).count() }

    /// Every built-in brush draws something in both modes; an eraser cuts
    /// the paper and a smudge drags it.
    #[test]
    fn every_brush_draws_something() {
        let (w, h) = (240, 38);
        let mut blank = Image::new(w, h, 64, [0; 4]);
        let [r, g, b] = PAPER;
        blank.fill_rect(Rect::new(0, 0, (w / 2) as i32, h as i32), [r, g, b, 255]);
        let blank: Vec<u8> = blank.read(blank.rect()).into_iter().flatten().collect();
        for p in aurastudio_forge::Preset::builtin() {
            let list = render(&p.settings, Shape::S, false, w, h);
            assert!(coverage(&list) > 20, "{}: empty list preview", p.name);
            let detail = render(&p.settings, Shape::S, true, w, h);
            assert!(detail != blank, "{}: empty preview", p.name);
        }
    }

    /// Its end is no blob: the pen lifts (the last pressure was NaN, read
    /// as full).
    #[test]
    fn the_stroke_tapers_at_its_end() {
        let pen = aurastudio_forge::Preset::builtin().into_iter().find(|p| p.name == "Pen").unwrap();
        let px = render(&pen.settings, Shape::S, false, 320, 38);
        let column = |x: u32| (0..38).filter(|&y| px[((y * 320 + x) * 4 + 3) as usize] > 128).count();
        let middle = column(160);
        let end = (250..320).map(column).max().unwrap();
        assert!(end <= middle, "end {end} middle {middle}");
    }

    /// Size shows up to 100 px, the same past it; nothing is cut by the
    /// picture's edges.
    #[test]
    fn sizes_show_up_to_100_px_and_nothing_is_cut() {
        let b = |d: f32| BrushSettings { size: d, ..BrushSettings::default() };
        let cov: Vec<usize> = [2.0, 8.0, 30.0, 100.0].iter().map(|&d| coverage(&render(&b(d), Shape::S, false, 240, 38))).collect();
        assert!(cov.windows(2).all(|w| w[0] < w[1]), "{cov:?}");
        assert_eq!(render(&b(100.0), Shape::S, false, 240, 38), render(&b(900.0), Shape::S, false, 240, 38));
        // A scattered brush keeps inside.
        let leaves = aurastudio_forge::Preset::builtin().into_iter().find(|p| p.settings.scattering.enabled).unwrap();
        let px = render(&leaves.settings, Shape::S, false, 240, 38);
        let edge = |y: u32| (0..240).filter(|&x| px[((y * 240 + x) * 4 + 3) as usize] > 8).count();
        assert!(edge(0) + edge(37) < 4, "{}: cut at the edges", leaves.name);
    }

    /// Eraser: holes in the paper; smudge: paper dragged to the right.
    #[test]
    fn an_eraser_cuts_the_paper() {
        let e = BrushSettings { size: 10.0, erase: true, ..BrushSettings::default() };
        let px = render(&e, Shape::S, true, 200, 38);
        let holes = px.chunks(4).enumerate().filter(|(i, p)| (*i as u32 % 200) < 100 && p[3] < 128).count();
        assert!(holes > 50, "{holes}");
    }

    /// Written, read back the same; a brush changed is another file.
    #[test]
    fn a_list_preview_is_kept_on_disk() {
        let dir = std::env::temp_dir().join(format!("aura-previews-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let b = BrushSettings { size: 12.0, ..BrushSettings::default() };
        let first = list_preview(&b, &TipReader::default(), Some(&dir));
        let path = file_of(&dir, &b);
        assert!(path.is_file());
        assert!(std::fs::metadata(&path).unwrap().len() < 4096, "a few KB");
        assert_eq!(read_alpha(&path).unwrap(), first);
        let red = BrushSettings { color: [1.0, 0.0, 0.0, 1.0], ..b.clone() };
        assert_eq!(file_of(&dir, &red), path, "a color is the same picture");
        assert_ne!(file_of(&dir, &BrushSettings { size: 13.0, ..b }), path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
