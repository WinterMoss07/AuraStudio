//! Stroke previews, painted on a thread of their own (2-50 ms each), the
//! latest request per strip; the old picture stays until the new one.
//!
//! Where a painted strip lives:
//! - on the GPU, in a `StripPool` (a few textures of about 1 MB, all strips
//!   of one size stacked): only the strips drawn lately, plus a few more.
//!   A strip drawn in the pass before is never given up; when there is no
//!   room, the new picture waits (`parked`) and nothing visible is
//!   painted again.
//! - on disk (lz4), keyed by everything the picture depends on, so that
//!   the next start (and the next version, as long as `RENDER_VERSION`
//!   stays) does not paint it again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use aurastudio_brush::{Brush, ForgeBrush};
use aurastudio_brushes::hash::Fnv;
use aurastudio_brushes::stroke_preview::{self, Mode, Shape, RENDER_VERSION};
use aurastudio_brushes::Preset;
use egui::{vec2, Color32, ColorImage, FontId, Rect, Ui};

use crate::strip_pool::{slots_per_page, Strip, StripPool};
use crate::theme;

// --- Disk cache ----------------------------------------------------------------

/// Where the pictures are kept between runs (set once by the app).
static CACHE: OnceLock<PathBuf> = OnceLock::new();

/// Files kept at most (the least recently used go).
const KEEP_FILES: usize = 600;

/// A file read again is touched when older than this, so that the clean-up
/// keeps what is used, not what was written last.
const TOUCH_AFTER: Duration = Duration::from_secs(6 * 3600);

/// Pictures read from disk, and painted, since the start.
static DISK_HITS: AtomicU64 = AtomicU64::new(0);
static PAINTED: AtomicU64 = AtomicU64::new(0);

/// Keep previews on disk under `dir`.
pub fn set_cache_dir(dir: &Path) {
    if let Some(d) = prepare_dir(dir) { let _ = CACHE.set(d); }
}

/// The folder for this render version under `dir`: made if needed, the
/// folders of other render versions removed, the oldest files pruned.
/// (A new program version keeps it, unless it changed how a preview looks:
/// `RENDER_VERSION`.)
fn prepare_dir(dir: &Path) -> Option<PathBuf> {
    let mine = format!("previews-r{RENDER_VERSION}");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with("previews-") && n != mine { let _ = std::fs::remove_dir_all(e.path()); }
        }
    }
    let d = dir.join(mine);
    std::fs::create_dir_all(&d).ok()?;
    prune(&d, KEEP_FILES);
    Some(d)
}

/// At most `keep` files: the least recently modified (written or touched) go.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<_> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= keep { return; }
    files.sort();
    for (_, p) in &files[..files.len() - keep] { let _ = std::fs::remove_file(p); }
}

fn file_in(dir: &Path, key: u64, w: u32, h: u32) -> PathBuf { dir.join(format!("{key:016x}-{w}x{h}.lz4")) }

/// A picture kept on disk (None: not there, or not the right size).
fn read_in(dir: &Path, key: u64, w: u32, h: u32) -> Option<Vec<u8>> {
    let path = file_in(dir, key, w, h);
    let bytes = std::fs::read(&path).ok()?;
    let px = lz4_flex::decompress_size_prepended(&bytes).ok()?;
    if px.len() != w as usize * h as usize * 4 { return None; }
    // Used: its date says so (not for every read, a write costs).
    let old = std::fs::metadata(&path).and_then(|m| m.modified()).ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok()).is_some_and(|age| age > TOUCH_AFTER);
    if old {
        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(SystemTime::now()));
    }
    Some(px)
}

fn write_in(dir: &Path, key: u64, w: u32, h: u32, px: &[u8]) {
    let p = file_in(dir, key, w, h);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, lz4_flex::compress_prepend_size(px)).is_ok() { let _ = std::fs::rename(&tmp, &p); }
}

// --- What a strip is ------------------------------------------------------------

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

/// Strips kept beyond the ones on screen (a little scrolling back and forth).
const MARGIN: usize = 4;
/// Pictures waiting for room, at most.
const MAX_PARKED: usize = 8;
/// A strip not drawn for this long (and not in the last pass) is let go.
const LET_GO_AFTER: Duration = Duration::from_secs(1);

type Class = (usize, usize);

/// When and at what size a slot was last drawn.
struct Seen { pass: u64, class: Class, at: Instant }

pub struct StrokePreviews {
    /// Per slot: the key its picture shows, and its place in the pool.
    slots:   HashMap<Slot, (u64, Strip)>,
    seen:    HashMap<Slot, Seen>,
    /// Per slot: the key last asked for (painting or waiting).
    asked:   HashMap<Slot, u64>,
    /// Painted pictures with no room yet: slot -> key, picture.
    parked:  HashMap<Slot, (u64, ColorImage)>,
    pool:    StripPool,
    /// The pass `begin_pass` last ran for.
    pass:    u64,
    /// Pages each size may hold in this pass: its strips drawn in the last
    /// one, plus `MARGIN`.
    allowed: HashMap<Class, usize>,
    queue:   Queue,
    done:    Receiver<Done>,
    /// Kept to start the thread on first use (it needs the egui context).
    send:    Option<Sender<Done>>,
}

impl Default for StrokePreviews {
    fn default() -> Self {
        let (send, done) = channel();
        Self {
            slots: HashMap::new(), seen: HashMap::new(), asked: HashMap::new(), parked: HashMap::new(),
            pool: StripPool::default(), pass: 0, allowed: HashMap::new(),
            queue: Arc::new((Mutex::new(Vec::new()), Condvar::new())), done, send: Some(send),
        }
    }
}

impl StrokePreviews {
    /// Draw the stroke preview in `rect` (`name` in its bottom right
    /// corner, if any); ask for a new one if the settings or the size
    /// changed.
    pub fn show(&mut self, ui: &Ui, rect: Rect, slot: Slot, stroke: Stroke, name: Option<&str>) {
        let ctx = ui.ctx();
        self.start(ctx);
        self.begin_pass(ctx, ctx.cumulative_pass_nr(), Instant::now());
        let ppp = ctx.pixels_per_point();
        // On whole screen pixels, one texel each (drawn without
        // filtering): edges don't shimmer from one picture to the next.
        let min = (rect.min.to_vec2() * ppp).round() / ppp;
        let (w, h) = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        let rect = Rect::from_min_size(min.to_pos2(), vec2(w as f32, h as f32) / ppp);
        let key = key_of(&stroke, w, h, ppp);
        self.note_drawn(slot, (w as usize, h as usize), Instant::now());
        let shown = self.slots.get(&slot).map(|(k, _)| *k);
        if shown != Some(key) && self.asked.get(&slot) != Some(&key) && w > 0 && h > 0 && StripPool::fits(w as usize, h as usize) {
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
        if let Some((id, uv)) = self.slots.get(&slot).and_then(|(_, s)| self.pool.image(*s)) {
            painter.image(id, rect, uv, Color32::WHITE);
        }
        if let Some(name) = name { name_label(&painter, rect, name); }
    }

    /// Once per frame, whether or not a strip is drawn: a docker that was
    /// closed or hidden lets go of its strips and textures.
    pub fn idle(&mut self, ctx: &egui::Context) {
        if !self.slots.is_empty() || !self.seen.is_empty() || self.pool.textures() > 0 {
            self.begin_pass(ctx, ctx.cumulative_pass_nr(), Instant::now());
        }
    }

    /// `slot` was drawn now, at this size.
    fn note_drawn(&mut self, slot: Slot, class: Class, at: Instant) {
        self.seen.insert(slot, Seen { pass: self.pass, class, at });
        self.pool.mark_drawn(class.0, class.1, self.pass);
    }

    /// Once per pass, before the first strip is drawn: work out how much
    /// room each size may take, let go what is no longer drawn, place the
    /// pictures that waited and those that arrived.
    fn begin_pass(&mut self, ctx: &egui::Context, pass: u64, now: Instant) {
        if pass == self.pass { return; }
        self.pass = pass;

        // Strips drawn in the last pass, per size: those stay, with a margin.
        let mut visible: HashMap<Class, usize> = HashMap::new();
        for s in self.seen.values() {
            if s.pass + 1 >= pass { *visible.entry(s.class).or_default() += 1; }
        }
        self.allowed = visible.iter().map(|(c, n)| (*c, (n + MARGIN).div_ceil(slots_per_page(c.0, c.1)))).collect();

        // Let go of what has not been drawn for a while.
        let gone: Vec<Slot> = self.seen.iter()
            .filter(|(_, s)| s.pass + 1 < pass && now.saturating_duration_since(s.at) >= LET_GO_AFTER)
            .map(|(slot, _)| *slot).collect();
        for slot in gone { self.forget(slot, now); }
        self.parked.retain(|slot, _| self.seen.get(slot).is_some_and(|s| s.pass + 1 >= pass));
        self.pool.tidy(pass, now);

        // The pictures that waited for room, then the new ones.
        let waiting: Vec<(Slot, (u64, ColorImage))> = self.parked.drain().collect();
        for (slot, (key, image)) in waiting { self.place(ctx, slot, key, image, now); }
        self.receive(ctx, now);

        // Something to let go later: wake up for it (nothing else would).
        let hanging = self.seen.values().filter(|s| s.pass + 1 < pass)
            .map(|s| LET_GO_AFTER.saturating_sub(now.saturating_duration_since(s.at))).min();
        if let Some(d) = hanging.into_iter().chain(self.pool.next_release(now)).min() {
            ctx.request_repaint_after(d + Duration::from_millis(50));
        }
        publish_stats(self);
    }

    /// Forget a slot altogether: its strip goes back to the pool.
    fn forget(&mut self, slot: Slot, now: Instant) {
        if let Some((_, strip)) = self.slots.remove(&slot) { self.pool.free(strip, now); }
        self.seen.remove(&slot);
        self.asked.remove(&slot);
        self.parked.remove(&slot);
    }

    /// Previews painted since the last pass: into the pool.
    fn receive(&mut self, ctx: &egui::Context, now: Instant) {
        while let Ok((slot, key, w, h, px)) = self.done.try_recv() {
            let image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            self.place(ctx, slot, key, image, now);
        }
    }

    /// Give a painted picture its place. A slot now drawn at another size
    /// (the docker was resized meanwhile) does not want it.
    fn place(&mut self, ctx: &egui::Context, slot: Slot, key: u64, image: ColorImage, now: Instant) {
        let class = (image.size[0], image.size[1]);
        match self.seen.get(&slot) {
            Some(s) if s.class == class => {}
            _ => { self.asked.remove(&slot); return; }
        }
        // Same size as the strip it has: written over it.
        if let Some((k, strip)) = self.slots.get_mut(&slot) {
            if strip.size() == class {
                self.pool.write(*strip, image);
                *k = key;
                return;
            }
        }
        if let Some((_, old)) = self.slots.remove(&slot) { self.pool.free(old, now); }
        let allowed = self.allowed.get(&class).copied().unwrap_or(1);
        loop {
            if let Some(strip) = self.pool.alloc(ctx, class.0, class.1, allowed, self.pass) {
                self.pool.write(strip, image);
                self.slots.insert(slot, (key, strip));
                return;
            }
            // No room: the least recently drawn strip of this size, if it
            // was not drawn in the last pass, makes some.
            let pass = self.pass;
            let victim = self.slots.iter()
                .filter(|(s, (_, st))| st.size() == class && self.seen.get(*s).is_none_or(|v| v.pass + 1 < pass))
                .min_by_key(|(s, _)| self.seen.get(*s).map_or(0, |v| v.pass)).map(|(s, _)| *s);
            match victim {
                Some(v) => {
                    if let Some((_, st)) = self.slots.remove(&v) { self.pool.free(st, now); }
                    // Asked again if it comes back on screen.
                    self.asked.remove(&v);
                }
                None => {
                    // Everything here is on screen: wait.
                    if self.parked.len() >= MAX_PARKED {
                        if let Some(old) = self.parked.keys().next().copied() { self.parked.remove(&old); self.asked.remove(&old); }
                    }
                    self.parked.insert(slot, (key, image));
                    return;
                }
            }
        }
    }

    /// Drop the brush rows' previews (the brush list changed: an import).
    pub fn clear_brushes(&mut self) {
        let now = Instant::now();
        let brushes: Vec<Slot> = self.seen.keys().chain(self.slots.keys()).copied().filter(|s| matches!(s, Slot::Brush(_))).collect();
        for s in brushes { self.forget(s, now); }
        self.queue.0.lock().unwrap().retain(|j| !matches!(j.slot, Slot::Brush(_)));
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
            let cached = CACHE.get().and_then(|d| read_in(d, job.key, job.w, job.h));
            let px = match cached {
                Some(px) => { DISK_HITS.fetch_add(1, Ordering::Relaxed); px }
                None => {
                    let px = stroke_preview::render(&job.brush, &job.settings, job.shape, job.mode, job.w, job.h, job.scale);
                    PAINTED.fetch_add(1, Ordering::Relaxed);
                    if let Some(d) = CACHE.get() { write_in(d, job.key, job.w, job.h, &px); }
                    px
                }
            };
            if send.send((job.slot, job.key, job.w, job.h, px)).is_err() { return; }
            ctx.request_repaint();
        });
        if let Err(e) = spawned { log::error!("stroke preview thread: {e}"); }
    }
}

// --- For the memory report ------------------------------------------------------

/// What the previews hold, for Window > Memory report.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreviewStats {
    /// Textures of the strip pool, and their bytes.
    pub textures: usize,
    pub bytes: usize,
    /// Strips placed, and pictures waiting for room.
    pub strips: usize,
    pub parked: usize,
    /// Pictures read from disk / painted since the start.
    pub disk_hits: u64,
    pub painted: u64,
}

static STATS: Mutex<PreviewStats> = Mutex::new(PreviewStats { textures: 0, bytes: 0, strips: 0, parked: 0, disk_hits: 0, painted: 0 });

fn publish_stats(p: &StrokePreviews) {
    if let Ok(mut s) = STATS.lock() {
        *s = PreviewStats {
            textures: p.pool.textures(), bytes: p.pool.bytes(), strips: p.slots.len(), parked: p.parked.len(),
            disk_hits: DISK_HITS.load(Ordering::Relaxed), painted: PAINTED.load(Ordering::Relaxed),
        };
    }
}

pub fn stats() -> PreviewStats { STATS.lock().map(|s| *s).unwrap_or_default() }

/// Files and bytes in the disk cache.
pub fn cache_usage() -> (usize, u64) {
    let Some(dir) = CACHE.get() else { return (0, 0) };
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
    rd.flatten().filter_map(|e| e.metadata().ok()).fold((0, 0), |(n, b), m| (n + 1, b + m.len()))
}

// --- Drawing --------------------------------------------------------------------

fn name_label(painter: &egui::Painter, rect: Rect, name: &str) {
    let font = FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(name.to_string(), font, theme::TEXT);
    let size = galley.size() + vec2(8.0, 2.0);
    let r = Rect::from_min_size(rect.right_bottom() - size - vec2(3.0, 3.0), size);
    painter.rect_filled(r, 2.0, Color32::from_black_alpha(150));
    painter.galley(r.left_top() + vec2(4.0, 1.0), galley, theme::TEXT);
}

/// Everything the picture depends on, as a number that means the same in
/// every build (it names a file): the render version, the brush's
/// definition, its size, opacity, hardness, eraser and blend mode (the
/// color is replaced by the preview's own ink), the shape and mode, the
/// pixel size and the screen scale.
fn key_of(s: &Stroke, w: u32, h: u32, ppp: f32) -> u64 {
    let b = s.settings;
    let mut h_ = Fnv::default();
    h_.u32(RENDER_VERSION).u64(s.preset.definition_hash)
        .f32(b.radius).f32(b.opacity).f32(b.hardness).bool(b.erase).str(&format!("{:?}", b.blend))
        .str(&format!("{:?}", s.shape)).str(&format!("{:?}", s.mode))
        .u32(w).u32(h).f32(ppp);
    h_.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_brushes::stroke_preview::Shape;

    const CLASS: Class = (560, 88);

    fn px() -> Vec<u8> { vec![0u8; CLASS.0 * CLASS.1 * 4] }
    fn ms(t: Instant, n: u64) -> Instant { t + Duration::from_millis(n) }

    /// A pass: the rows drawn, the images that arrive.
    fn pass(p: &mut StrokePreviews, ctx: &egui::Context, n: u64, at: Instant, rows: std::ops::Range<usize>, arrive: &[(usize, u64)]) {
        let tx = p.send.clone().unwrap();
        for (i, key) in arrive { tx.send((Slot::Brush(*i), *key, CLASS.0 as u32, CLASS.1 as u32, px())).unwrap(); }
        p.begin_pass(ctx, n, at);
        for i in rows {
            p.note_drawn(Slot::Brush(i), CLASS, at);
            p.asked.entry(Slot::Brush(i)).or_insert(1);
        }
    }

    fn pages_for(visible: usize) -> usize { (visible + MARGIN).div_ceil(slots_per_page(CLASS.0, CLASS.1)) }

    /// Strips drawn in the last pass are never given up, however the
    /// pictures arrive, and nothing is asked twice.
    #[test]
    fn visible_strips_are_never_evicted_and_nothing_loops() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        pass(&mut p, &ctx, 1, t0, 0..30, &[]);
        let all: Vec<_> = (0..30).map(|i| (i, 1)).collect();
        pass(&mut p, &ctx, 2, ms(t0, 10), 0..30, &all);
        assert_eq!(p.slots.len(), 30);
        assert!(p.parked.is_empty());
        for n in 3..15u64 {
            // A new picture for one row, as a setting changes.
            pass(&mut p, &ctx, n, ms(t0, 10 * n), 0..30, &[((n % 30) as usize, n)]);
            assert_eq!(p.slots.len(), 30, "pass {n}");
            assert!(p.parked.is_empty());
        }
        assert_eq!(p.asked.len(), 30);
        assert!(p.pool.textures() <= pages_for(30), "{} textures", p.pool.textures());
    }

    /// Scrolling by a third: the rows that left make room for the rows
    /// that came, within the same pages.
    #[test]
    fn scrolling_reuses_the_places_of_rows_that_left() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        pass(&mut p, &ctx, 1, t0, 0..30, &[]);
        let first: Vec<_> = (0..30).map(|i| (i, 1)).collect();
        pass(&mut p, &ctx, 2, ms(t0, 10), 0..30, &first);
        // Rows 10..40 on screen now; their pictures arrive a pass later.
        pass(&mut p, &ctx, 3, ms(t0, 20), 10..40, &[]);
        let new: Vec<_> = (30..40).map(|i| (i, 1)).collect();
        pass(&mut p, &ctx, 4, ms(t0, 30), 10..40, &new);
        for i in 10..40 { assert!(p.slots.contains_key(&Slot::Brush(i)), "row {i} lost"); }
        assert!(p.parked.is_empty());
        // Within the allowance: the rows that left gave up their places.
        let room = pages_for(30) * slots_per_page(CLASS.0, CLASS.1);
        assert!(p.slots.len() <= room, "{} strips for {room} places", p.slots.len());
        assert!(p.pool.textures() <= pages_for(30));
        let gone: Vec<_> = (0..10).filter(|i| !p.slots.contains_key(&Slot::Brush(*i))).collect();
        assert!(gone.len() >= 40 - room, "{gone:?}");
        for i in gone { assert!(!p.asked.contains_key(&Slot::Brush(i)), "row {i} may be asked again"); }
    }

    /// With no room and nothing to give up, a picture waits, then takes the
    /// place of a row that left.
    #[test]
    fn a_picture_with_no_room_waits_instead_of_evicting_a_visible_row() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        let n = slots_per_page(CLASS.0, CLASS.1);
        pass(&mut p, &ctx, 1, t0, 0..n + 1, &[]);
        p.allowed.insert(CLASS, 1);
        let imgs: Vec<_> = (0..n + 1).map(|i| (i, 1)).collect();
        let tx = p.send.clone().unwrap();
        for (i, key) in &imgs { tx.send((Slot::Brush(*i), *key, CLASS.0 as u32, CLASS.1 as u32, px())).unwrap(); }
        // Receive without recomputing the allowance.
        p.pass = 2;
        p.receive(&ctx, ms(t0, 5));
        for i in 0..n + 1 { p.note_drawn(Slot::Brush(i), CLASS, ms(t0, 5)); }
        assert_eq!(p.slots.len(), n, "one page");
        assert_eq!(p.parked.len(), 1, "the last one waits");
        for i in 0..n { assert!(p.slots.contains_key(&Slot::Brush(i))); }
        // The next pass counts all of them as visible: room for one more page.
        pass(&mut p, &ctx, 3, ms(t0, 10), 0..n + 1, &[]);
        assert!(p.slots.contains_key(&Slot::Brush(n)), "placed once there was room");
        assert!(p.parked.is_empty());
        for i in 0..n { assert!(p.slots.contains_key(&Slot::Brush(i)), "row {i} kept"); }
    }

    /// 160 rows scrolled through, 22 on screen: the GPU part stays within
    /// the rows on screen plus the margin, the whole way.
    #[test]
    fn scrolling_through_a_long_list_keeps_a_fixed_gpu_budget() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        let mut widest = 0;
        let mut arrive: Vec<(usize, u64)> = Vec::new();
        for step in 0..140usize {
            let at = t0 + Duration::from_millis(50 * step as u64);
            pass(&mut p, &ctx, step as u64 + 1, at, step..step + 22, &arrive);
            // The row that just came in is painted for the next pass.
            arrive = vec![(step + 21, 1)];
            widest = widest.max(p.pool.textures());
            assert!(p.parked.is_empty(), "step {step}");
        }
        assert!(widest <= pages_for(22), "{widest} textures, {} allowed", pages_for(22));
        let bytes = p.pool.bytes();
        assert!(bytes <= pages_for(22) * (1 << 20) + (1 << 18), "{bytes} bytes");
        // Every row of the last screen has its strip.
        for i in 139..160 { assert!(p.slots.contains_key(&Slot::Brush(i)), "row {i}"); }
    }

    /// A picture painted for a size the strip no longer has is dropped.
    #[test]
    fn a_picture_for_an_old_size_is_dropped() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        pass(&mut p, &ctx, 1, t0, 0..1, &[]);
        let tx = p.send.clone().unwrap();
        tx.send((Slot::Brush(0), 1, 300, 40, vec![0u8; 300 * 40 * 4])).unwrap();
        p.begin_pass(&ctx, 2, ms(t0, 5));
        assert!(p.slots.is_empty() && p.pool.textures() == 0);
        assert!(!p.asked.contains_key(&Slot::Brush(0)), "asked again at the new size");
    }

    /// Rows that stop being drawn go, their pages after them, and the app
    /// is woken for it (nothing else would).
    #[test]
    fn rows_not_drawn_are_let_go_and_their_textures_released() {
        let ctx = egui::Context::default();
        let mut p = StrokePreviews::default();
        let t0 = Instant::now();
        pass(&mut p, &ctx, 1, t0, 0..12, &[]);
        let all: Vec<_> = (0..12).map(|i| (i, 1)).collect();
        pass(&mut p, &ctx, 2, ms(t0, 5), 0..12, &all);
        assert!(p.pool.textures() >= 2);
        // The docker is hidden: passes go on without these rows.
        pass(&mut p, &ctx, 3, ms(t0, 100), 0..0, &[]);
        assert_eq!(p.slots.len(), 12, "not yet one second");
        pass(&mut p, &ctx, 4, t0 + Duration::from_millis(1500), 0..0, &[]);
        assert!(p.slots.is_empty() && p.seen.is_empty() && p.asked.is_empty());
        pass(&mut p, &ctx, 5, t0 + Duration::from_secs(4), 0..0, &[]);
        assert_eq!(p.pool.textures(), 0, "pages went back to the GPU");
    }

    fn preset() -> Preset {
        aurastudio_brushes::Library::load_bundled().presets.into_iter().next().expect("a bundled brush")
    }

    #[test]
    fn the_cache_key_follows_the_picture_and_nothing_else() {
        let p = preset();
        let key = |p: &Preset, b: &Brush, w, h, ppp| key_of(&Stroke { preset: p, settings: b, shape: Shape::S, mode: Mode::List }, w, h, ppp);
        let b = p.settings.clone();
        let base = key(&p, &b, 560, 88, 2.0);
        assert_eq!(base, key(&p, &b, 560, 88, 2.0));
        // The color is replaced by the preview's ink: not part of it.
        let mut c = b.clone();
        c.color = [1, 2, 3];
        assert_eq!(base, key(&p, &c, 560, 88, 2.0));
        // These are.
        let mut r = b.clone();
        r.radius += 1.0;
        assert_ne!(base, key(&p, &r, 560, 88, 2.0));
        assert_ne!(base, key(&p, &b, 561, 88, 2.0));
        assert_ne!(base, key(&p, &b, 560, 88, 1.5));
        let mut e = b.clone();
        e.erase = !e.erase;
        assert_ne!(base, key(&p, &e, 560, 88, 2.0));
        // A brush of the same name, medium and author, edited.
        let mut edited = p.clone();
        let mut def = (*edited.brush).clone();
        def.options.dab_on_press = !def.options.dab_on_press;
        edited.brush = Arc::new(def);
        edited.rehash();
        assert_ne!(base, key(&edited, &b, 560, 88, 2.0));
        assert_eq!((edited.name.as_str(), edited.medium.as_str()), (p.name.as_str(), p.medium.as_str()));
    }

    /// The number written in file names must not change between builds:
    /// it is pinned. Change it only together with RENDER_VERSION.
    #[test]
    fn the_cache_key_is_stable_across_builds() {
        let mut p = preset();
        p.definition_hash = 0x1234_5678_9abc_def0;
        let b = Brush { color: [0, 0, 0], opacity: 1.0, radius: 12.5, blend: aurastudio_canvas::BlendMode::Normal, hardness: 0.5, erase: false };
        let k = key_of(&Stroke { preset: &p, settings: &b, shape: Shape::S, mode: Mode::List }, 560, 88, 2.0);
        assert_eq!(k, PINNED_KEY, "{k:#018x}");
    }
    const PINNED_KEY: u64 = 0x2c03_89fa_af42_b451;

    #[test]
    fn the_disk_cache_keeps_its_render_version_and_drops_the_others() {
        let dir = std::env::temp_dir().join(format!("aura-previews-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("previews-0.15.1")).unwrap();
        std::fs::create_dir_all(dir.join("previews-r0")).unwrap();
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let d = prepare_dir(&dir).unwrap();
        assert!(d.ends_with(format!("previews-r{RENDER_VERSION}")));
        write_in(&d, 7, 4, 6, &(0..4 * 6 * 4).map(|i| i as u8).collect::<Vec<_>>());
        // A second start (a new program version): same folder, same file.
        let again = prepare_dir(&dir).unwrap();
        assert_eq!(again, d);
        assert!(read_in(&d, 7, 4, 6).is_some());
        assert!(!dir.join("previews-0.15.1").exists() && !dir.join("previews-r0").exists());
        assert!(dir.join("other").exists(), "not ours");
        // Another key (an edited brush) or size is another file.
        assert_eq!(read_in(&d, 8, 4, 6), None);
        assert_eq!(read_in(&d, 7, 6, 4), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_an_old_file_refreshes_its_date_so_pruning_keeps_what_is_used() {
        let dir = std::env::temp_dir().join(format!("aura-touch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_in(&dir, 1, 2, 2, &[7u8; 16]);
        write_in(&dir, 2, 2, 2, &[8u8; 16]);
        let old = SystemTime::now() - Duration::from_secs(48 * 3600);
        for k in [1u64, 2] {
            std::fs::File::options().write(true).open(file_in(&dir, k, 2, 2)).unwrap().set_modified(old).unwrap();
        }
        // File 1 is used again, file 2 is not.
        assert!(read_in(&dir, 1, 2, 2).is_some());
        prune(&dir, 1);
        assert!(file_in(&dir, 1, 2, 2).exists(), "the used one stays");
        assert!(!file_in(&dir, 2, 2, 2).exists(), "the unused one goes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
