//! Brush preview textures for the Brushes and Tool options dockers: a
//! stroke painted by the brush itself (aurastudio_brushes::preview), kept
//! until the brush's settings or the strip's size change. Painting them
//! all at once would freeze the interface (~200 brushes, a few ms each):
//! the list paints a few per frame, the Tool options one first.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use aurastudio_brush::Brush;
use aurastudio_brushes::{preview, Preset};
use egui::{pos2, vec2, Color32, FontId, Rect, TextureHandle, Ui};

use crate::theme;

/// Time the list may spend painting previews per frame.
const BUDGET: Duration = Duration::from_millis(12);

#[derive(Default)]
pub struct PreviewCache {
    /// Per slot (a list row, or the Tool options strip): the settings key
    /// it was painted for, and the texture.
    slots: HashMap<Slot, (u64, TextureHandle)>,
    /// Frame being counted, time spent painting in it.
    frame: u64,
    spent: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Slot { List(usize), ToolOptions }

impl PreviewCache {
    /// Draw the preview of `preset` with `settings` in `rect`, the name in
    /// its bottom right corner. The Tool options strip (the one the user
    /// is adjusting) is painted at once, whatever the frame's budget.
    pub fn show(&mut self, ui: &Ui, rect: Rect, slot: Slot, preset: &Preset, settings: &Brush, name: &str) {
        let urgent = slot == Slot::ToolOptions;
        let ctx = ui.ctx();
        let ppp = ctx.pixels_per_point();
        let tex_w = (rect.width() * ppp).round() as u32;
        let tex_h = (rect.height() * ppp).round() as u32;
        let key = key_of(preset, settings, tex_w, tex_h, ppp);

        let pass = ctx.cumulative_pass_nr();
        if pass != self.frame {
            self.frame = pass;
            self.spent = Duration::ZERO;
        }
        let stale = self.slots.get(&slot).is_none_or(|(k, _)| *k != key);
        if stale && tex_w > 0 && tex_h > 0 {
            if urgent || self.spent < BUDGET {
                let t = Instant::now();
                // Canvas pixels at true size: one per screen pixel at 100 %
                // zoom (HiDPI image scaling).
                let px = preview::render(&preset.engine, settings, tex_w, tex_h, ppp);
                self.spent += t.elapsed();
                let image = egui::ColorImage::from_rgba_unmultiplied([tex_w as usize, tex_h as usize], &px);
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
            } else {
                // The rest next frame (the old picture meanwhile).
                ctx.request_repaint();
            }
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
