//! Stroke previews, painted on a thread of their own (2-50 ms each), the
//! latest request per strip; the old picture stays until the new one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use aurastudio_brush::{Brush, ForgeBrush};
use aurastudio_brushes::stroke_preview::{self, Mode, Shape};
use aurastudio_brushes::Preset;
use egui::{vec2, Color32, FontId, Rect, Ui};

use crate::atlas::{Atlas, Region};
use crate::theme;

/// Where the pictures are kept between runs (set once by the app).
static CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Keep previews on disk under `dir`, in a folder per program version (a
/// new version may draw brushes differently); older versions' folders go.
pub fn set_cache_dir(dir: &Path) {
    let mine = format!("previews-{}", env!("CARGO_PKG_VERSION"));
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("previews-") && n != mine { let _ = std::fs::remove_dir_all(e.path()); }
        }
    }
    let d = dir.join(mine);
    if std::fs::create_dir_all(&d).is_ok() { prune(&d, 600); let _ = CACHE.set(d); }
}

/// At most `keep` files: the oldest go.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep { return; }
    files.sort();
    for (_, p) in &files[..files.len() - keep] { let _ = std::fs::remove_file(p); }
}

fn disk_path(key: u64, w: u32, h: u32) -> Option<PathBuf> {
    Some(CACHE.get()?.join(format!("{key:016x}-{w}x{h}.lz4")))
}

/// A picture kept on disk (None: not there, or not the right size).
fn disk_read(key: u64, w: u32, h: u32) -> Option<Vec<u8>> {
    let bytes = std::fs::read(disk_path(key, w, h)?).ok()?;
    let px = lz4_flex::decompress_size_prepended(&bytes).ok()?;
    (px.len() == w as usize * h as usize * 4).then_some(px)
}

fn disk_write(key: u64, w: u32, h: u32, px: &[u8]) {
    let Some(p) = disk_path(key, w, h) else { return };
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, lz4_flex::compress_prepend_size(px)).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

/// Which strip: a row of the Tools docker (brush or geometry tool), or
/// Tool options'.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot { Brush(usize), Shape(Shape), ToolOptions }

struct Job { slot: Slot, key: u64, brush: Arc<ForgeBrush>, settings: Brush, shape: Shape, mode: Mode, w: u32, h: u32, scale: f32 }

/// A painted preview: slot, settings key, size, straight RGBA.
type Done = (Slot, u64, u32, u32, Vec<u8>);

/// Waiting jobs, at most one per slot (a new request replaces it in place).
type Queue = Arc<(Mutex<Vec<Job>>, Condvar)>;

/// What a strip shows.
pub struct Stroke<'a> {
    pub preset:   &'a Preset,
    pub settings: &'a Brush,
    pub shape:    Shape,
    pub mode:     Mode,
}

pub struct StrokePreviews {
    /// Per slot: the settings key its picture shows, and its place in the atlas.
    slots:   HashMap<Slot, (u64, Region)>,
    /// Per slot: the frame it was last drawn (the least recent leave when the atlas is full).
    seen:    HashMap<Slot, u64>,
    /// All the previews share a few pages instead of a texture each.
    atlas:   Atlas,
    /// Per slot: the key last asked for (painting or waiting).
    asked:   HashMap<Slot, u64>,
    queue:   Queue,
    done:    Receiver<Done>,
    /// Kept to start the thread on first use (it needs the egui context).
    send:    Option<Sender<Done>>,
}

/// Pages kept for previews (1 MB each): about what a screenful needs; the
/// others come back from the disk cache when scrolled to.
const PAGES: usize = 3;

impl Default for StrokePreviews {
    fn default() -> Self {
        let (send, done) = channel();
        Self { slots: HashMap::new(), atlas: Atlas::new("stroke-previews", egui::TextureOptions::NEAREST, PAGES), seen: HashMap::new(), asked: HashMap::new(), queue: Arc::new((Mutex::new(Vec::new()), Condvar::new())), done, send: Some(send) }
    }
}

impl StrokePreviews {
    /// Draw the stroke preview in `rect` (`name` in its bottom right
    /// corner, if any); ask for a new one if the settings or the size
    /// changed.
    pub fn show(&mut self, ui: &Ui, rect: Rect, slot: Slot, stroke: Stroke, name: Option<&str>) {
        let ctx = ui.ctx();
        self.start(ctx);
        self.receive(ctx);
        let ppp = ctx.pixels_per_point();
        // On whole screen pixels, one texel each (drawn without
        // filtering): edges don't shimmer from one picture to the next.
        let min = (rect.min.to_vec2() * ppp).round() / ppp;
        let (w, h) = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        let rect = Rect::from_min_size(min.to_pos2(), vec2(w as f32, h as f32) / ppp);
        let key = key_of(&stroke, w, h, ppp);
        self.seen.insert(slot, ctx.cumulative_pass_nr());
        let shown = self.slots.get(&slot).map(|(k, _)| *k);
        if shown != Some(key) && self.asked.get(&slot) != Some(&key) && w > 0 && h > 0 {
            self.asked.insert(slot, key);
            // Canvas pixels at true size: one per screen pixel at 100 %
            // zoom (HiDPI image scaling).
            let job = Job { slot, key, brush: stroke.preset.brush.clone(), settings: stroke.settings.clone(),
                shape: stroke.shape, mode: stroke.mode, w, h, scale: ppp };
            let (lock, wake) = &*self.queue;
            let mut q = lock.lock().unwrap();
            match q.iter_mut().find(|j| j.slot == slot) {
                Some(j) => *j = job,
                None => q.push(job),
            }
            wake.notify_one();
        }

        let painter = ui.painter_at(rect);
        if let Some((_, r)) = self.slots.get(&slot) {
            let (id, uv) = self.atlas.image(*r);
            painter.image(id, rect, uv, Color32::WHITE);
        }
        if let Some(name) = name { name_label(&painter, rect, name); }
    }

    /// Room for a `w` × `h` picture: if the atlas is full, the previews
    /// not drawn this frame leave, least recent first.
    fn place(&mut self, ctx: &egui::Context, w: usize, h: usize) -> Option<Region> {
        loop {
            if let Some(r) = self.atlas.alloc(ctx, w, h) { return Some(r); }
            let now = ctx.cumulative_pass_nr();
            let oldest = self.slots.keys().filter(|s| self.seen.get(*s).is_some_and(|f| *f < now))
                .min_by_key(|s| self.seen.get(*s).copied().unwrap_or(0)).copied()?;
            if let Some((_, r)) = self.slots.remove(&oldest) { self.atlas.free(r); }
            // Asked again if it comes back on screen.
            self.asked.remove(&oldest);
        }
    }

    /// Drop the brush rows' previews (the brush list changed: an import).
    pub fn clear_brushes(&mut self) {
        let keep = |s: &Slot| !matches!(s, Slot::Brush(_));
        for (s, (_, r)) in &self.slots { if !keep(s) { self.atlas.free(*r); } }
        self.slots.retain(|s, _| keep(s));
        self.seen.retain(|s, _| keep(s));
        self.asked.retain(|s, _| keep(s));
        self.queue.0.lock().unwrap().retain(|j| keep(&j.slot));
    }

    /// The preview thread, started the first time a preview is shown.
    fn start(&mut self, ctx: &egui::Context) {
        let Some(send) = self.send.take() else { return };
        let (queue, ctx) = (Arc::clone(&self.queue), ctx.clone());
        let spawned = std::thread::Builder::new().name("stroke previews".into()).spawn(move || loop {
            let job = {
                let (lock, wake) = &*queue;
                let mut q = lock.lock().unwrap();
                while q.is_empty() { q = wake.wait(q).unwrap(); }
                q.remove(0)
            };
            let px = match disk_read(job.key, job.w, job.h) {
                Some(px) => px,
                None => {
                    let px = stroke_preview::render(&job.brush, &job.settings, job.shape, job.mode, job.w, job.h, job.scale);
                    disk_write(job.key, job.w, job.h, &px);
                    px
                }
            };
            if send.send((job.slot, job.key, job.w, job.h, px)).is_err() { return; }
            ctx.request_repaint();
        });
        if let Err(e) = spawned { log::error!("stroke preview thread: {e}"); }
    }

    /// Previews painted since the last frame: into their textures.
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((slot, key, w, h, px)) = self.done.try_recv() {
            // Superseded meanwhile (a newer one is coming): still shown,
            // closer than the old one.
            let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            // Same size: written over its place; else a new place.
            let same = self.slots.get(&slot).map(|(_, r)| *r).filter(|r| (r.w as u32, r.h as u32) == (w, h));
            let region = match same {
                Some(r) => Some(r),
                None => {
                    if let Some((_, old)) = self.slots.remove(&slot) { self.atlas.free(old); }
                    self.place(ctx, w as usize, h as usize)
                }
            };
            if let Some(r) = region {
                self.atlas.write(r, image);
                self.slots.insert(slot, (key, r));
            }
        }
    }
}

/// The name in the strip's bottom right corner, on a dark backing so it
/// reads over the stroke.
fn name_label(painter: &egui::Painter, rect: Rect, name: &str) {
    let font = FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(name.to_string(), font, theme::TEXT);
    let size = galley.size() + vec2(8.0, 2.0);
    let r = Rect::from_min_size(rect.right_bottom() - size - vec2(3.0, 3.0), size);
    painter.rect_filled(r, 2.0, Color32::from_black_alpha(150));
    painter.galley(r.left_top() + vec2(4.0, 1.0), galley, theme::TEXT);
}

/// Everything the picture depends on.
fn key_of(s: &Stroke, w: u32, h: u32, ppp: f32) -> u64 {
    let (p, b) = (s.preset, s.settings);
    let mut h_ = std::collections::hash_map::DefaultHasher::new();
    (&p.name, &p.medium, &p.author, w, h, ppp.to_bits(), s.shape, s.mode).hash(&mut h_);
    (b.radius.to_bits(), b.opacity.to_bits(), b.hardness.to_bits(), b.erase, format!("{:?}", b.blend)).hash(&mut h_);
    h_.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full atlas lets the previews not drawn this frame go; those drawn
    /// now stay; a preview that left is asked for again.
    #[test]
    fn a_full_atlas_drops_the_least_recent_previews_not_the_visible_ones() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let mut ids = Vec::new();
        let mut frame = 0u64;
        let mut n = 0usize;
        // Fill the three pages with 200 × 100 pictures, drawn over time.
        loop {
            let _ = ctx.run(Default::default(), |ctx| {
                let slot = Slot::Brush(n);
                p.seen.insert(slot, ctx.cumulative_pass_nr());
                if let Some(r) = p.place(ctx, 200, 100) { p.slots.insert(slot, (1, r)); p.asked.insert(slot, 1); ids.push(slot); }
            });
            frame += 1;
            n += 1;
            if p.atlas.pages() == PAGES && p.atlas.alloc(&ctx, 200, 100).is_none() || n > 200 { break; }
        }
        assert!(frame > 6 && p.slots.len() >= 6, "{} slots", p.slots.len());
        let before = p.slots.len();
        // One more frame draws only the last picture, then asks for room.
        let keep = *ids.last().unwrap();
        let _ = ctx.run(Default::default(), |ctx| {
            p.seen.insert(keep, ctx.cumulative_pass_nr());
            let slot = Slot::Brush(999);
            p.seen.insert(slot, ctx.cumulative_pass_nr());
            let r = p.place(ctx, 200, 100).expect("room made by dropping the oldest");
            p.slots.insert(slot, (1, r));
        });
        assert!(p.slots.contains_key(&keep) && p.slots.contains_key(&Slot::Brush(999)));
        assert!(p.slots.len() <= before, "no growth past the pages");
        assert!(!p.slots.contains_key(&ids[0]) && !p.asked.contains_key(&ids[0]), "the oldest left and may be asked again");
    }

    #[test]
    fn a_picture_comes_back_from_disk_and_a_wrong_size_does_not() {
        let dir = std::env::temp_dir().join(format!("aura-previews-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_cache_dir(&dir);
        let px: Vec<u8> = (0..4 * 6 * 4).map(|i| i as u8).collect();
        assert_eq!(disk_read(7, 4, 6), None);
        disk_write(7, 4, 6, &px);
        assert_eq!(disk_read(7, 4, 6), Some(px));
        // Another key (an edited brush) or size is another file.
        assert_eq!(disk_read(8, 4, 6), None);
        assert_eq!(disk_read(7, 6, 4), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_oldest_files_go_first() {
        let dir = std::env::temp_dir().join(format!("aura-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}")), b"x").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        prune(&dir, 2);
        let mut left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["3", "4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
