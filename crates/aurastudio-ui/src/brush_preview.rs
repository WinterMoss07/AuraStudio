//! Brush preview textures for the Brushes and Tool options dockers: a
//! stroke painted by the brush itself (aurastudio_brushes::preview), kept
//! until the brush's settings or the strip's size change.
//!
//! Painting one takes 2 to 50 ms (textured MyPaint brushes): done on the
//! interface thread, a slider drag stuttered. A preview thread paints
//! them instead, one at a time (each brush still paints on one thread),
//! the Tool options strip first; it sleeps when there is nothing to do.
//! Only the latest request per strip counts: dragging a slider doesn't
//! queue a preview per frame. The old picture stays until the new one.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use aurastudio_brush::Brush;
use aurastudio_brushes::{preview, Engine, Preset};
use egui::{pos2, vec2, Color32, FontId, Rect, TextureHandle, Ui};

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot { List(usize), ToolOptions }

struct Job { slot: Slot, key: u64, engine: Engine, settings: Brush, w: u32, h: u32, scale: f32 }

/// A painted preview: slot, settings key, size, straight RGBA.
type Done = (Slot, u64, u32, u32, Vec<u8>);

/// Waiting jobs, at most one per slot (a new request replaces it).
type Queue = Arc<(Mutex<Vec<Job>>, Condvar)>;

pub struct PreviewCache {
    /// Per slot: the settings key its picture shows, and the texture.
    slots:   HashMap<Slot, (u64, TextureHandle)>,
    /// Per slot: the key last asked for (painting or waiting).
    asked:   HashMap<Slot, u64>,
    queue:   Queue,
    done:    Receiver<Done>,
    /// Kept to start the thread on first use (it needs the egui context).
    send:    Option<Sender<Done>>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        let (send, done) = channel();
        Self { slots: HashMap::new(), asked: HashMap::new(), queue: Arc::new((Mutex::new(Vec::new()), Condvar::new())), done, send: Some(send) }
    }
}

impl PreviewCache {
    /// Draw the preview of `preset` with `settings` in `rect`, the name in
    /// its bottom right corner; ask for a new one if the settings or the
    /// size changed.
    pub fn show(&mut self, ui: &Ui, rect: Rect, slot: Slot, preset: &Preset, settings: &Brush, name: &str) {
        let ctx = ui.ctx();
        self.start(ctx);
        self.receive(ctx);
        let ppp = ctx.pixels_per_point();
        let (w, h) = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        let key = key_of(preset, settings, w, h, ppp);
        let shown = self.slots.get(&slot).map(|(k, _)| *k);
        if shown != Some(key) && self.asked.get(&slot) != Some(&key) && w > 0 && h > 0 {
            self.asked.insert(slot, key);
            // Canvas pixels at true size: one per screen pixel at 100 %
            // zoom (HiDPI image scaling).
            let job = Job { slot, key, engine: preset.engine.clone(), settings: settings.clone(), w, h, scale: ppp };
            let (lock, wake) = &*self.queue;
            let mut q = lock.lock().unwrap();
            q.retain(|j| j.slot != slot);
            q.push(job);
            wake.notify_one();
        }

        let painter = ui.painter_at(rect);
        if let Some((_, tex)) = self.slots.get(&slot) {
            painter.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        name_label(&painter, rect, name);
    }

    /// Drop the list's previews (the brush list changed: an import).
    pub fn clear_list(&mut self) {
        self.slots.retain(|s, _| *s == Slot::ToolOptions);
        self.asked.retain(|s, _| *s == Slot::ToolOptions);
        self.queue.0.lock().unwrap().retain(|j| j.slot == Slot::ToolOptions);
    }

    /// The preview thread, started the first time a preview is shown.
    fn start(&mut self, ctx: &egui::Context) {
        let Some(send) = self.send.take() else { return };
        let (queue, ctx) = (Arc::clone(&self.queue), ctx.clone());
        let spawned = std::thread::Builder::new().name("brush previews".into()).spawn(move || loop {
            let job = {
                let (lock, wake) = &*queue;
                let mut q = lock.lock().unwrap();
                while q.is_empty() { q = wake.wait(q).unwrap(); }
                // The Tool options strip first (the one being adjusted).
                let i = q.iter().position(|j| j.slot == Slot::ToolOptions).unwrap_or(0);
                q.remove(i)
            };
            let px = preview::render(&job.engine, &job.settings, job.w, job.h, job.scale);
            if send.send((job.slot, job.key, job.w, job.h, px)).is_err() { return; }
            ctx.request_repaint();
        });
        if let Err(e) = spawned { log::error!("brush preview thread: {e}"); }
    }

    /// Previews painted since the last frame: into their textures.
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((slot, key, w, h, px)) = self.done.try_recv() {
            // Superseded meanwhile (a newer one is coming): still shown,
            // closer than the old one.
            let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            match self.slots.get_mut(&slot) {
                Some((k, tex)) => {
                    tex.set(image, egui::TextureOptions::LINEAR);
                    *k = key;
                }
                None => {
                    let tex = ctx.load_texture(format!("brush-stroke-{slot:?}"), image, egui::TextureOptions::LINEAR);
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
fn key_of(p: &Preset, b: &Brush, w: u32, h: u32, ppp: f32) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    (&p.name, &p.group, w, h, ppp.to_bits()).hash(&mut s);
    (b.radius.to_bits(), b.opacity.to_bits(), b.hardness.to_bits(), b.erase, format!("{:?}", b.blend)).hash(&mut s);
    s.finish()
}
