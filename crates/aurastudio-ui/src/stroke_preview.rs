//! Stroke previews, painted on a thread of their own (2-50 ms each), the
//! latest request per strip; the old picture stays until the new one.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use aurastudio_brush::{Brush, ForgeBrush};
use aurastudio_brushes::stroke_preview::{self, Mode, Shape};
use aurastudio_brushes::Preset;
use egui::{pos2, vec2, Color32, FontId, Rect, TextureHandle, Ui};

use crate::theme;

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
    /// Per slot: the settings key its picture shows, and the texture.
    slots:   HashMap<Slot, (u64, TextureHandle)>,
    /// Per slot: the key last asked for (painting or waiting).
    asked:   HashMap<Slot, u64>,
    queue:   Queue,
    done:    Receiver<Done>,
    /// Kept to start the thread on first use (it needs the egui context).
    send:    Option<Sender<Done>>,
}

impl Default for StrokePreviews {
    fn default() -> Self {
        let (send, done) = channel();
        Self { slots: HashMap::new(), asked: HashMap::new(), queue: Arc::new((Mutex::new(Vec::new()), Condvar::new())), done, send: Some(send) }
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
        if let Some((_, tex)) = self.slots.get(&slot) {
            painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        if let Some(name) = name { name_label(&painter, rect, name); }
    }

    /// Drop the brush rows' previews (the brush list changed: an import).
    pub fn clear_brushes(&mut self) {
        let keep = |s: &Slot| !matches!(s, Slot::Brush(_));
        self.slots.retain(|s, _| keep(s));
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
            let px = stroke_preview::render(&job.brush, &job.settings, job.shape, job.mode, job.w, job.h, job.scale);
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
            match self.slots.get_mut(&slot) {
                Some((k, tex)) => {
                    tex.set(image, egui::TextureOptions::NEAREST);
                    *k = key;
                }
                None => {
                    let tex = ctx.load_texture(format!("stroke-preview-{slot:?}"), image, egui::TextureOptions::NEAREST);
                    self.slots.insert(slot, (key, tex));
                }
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
    painter.rect_filled(r, 3.0, Color32::from_black_alpha(150));
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
