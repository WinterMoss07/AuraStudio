//! The dialog's preview: before / after on either side of a line, zoom
//! and pan, filtered on its own thread (view math after PhotoCraft, MIT,
//! see NOTICE).

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use aurastudio_filters::Filter;
use egui::{pos2, vec2, Align2, Color32, Rect, RichText, Sense, Stroke, TextureHandle, Ui};

use super::draw::{self, WIDTH};
use super::source::{cut, Crop, Source, Worker};
use crate::theme;

const ZOOM_STEPS: [f32; 12] = [0.05, 0.1, 0.167, 0.25, 0.333, 0.5, 0.667, 1.0, 2.0, 3.0, 4.0, 8.0];

#[derive(Clone, Copy, PartialEq)]
enum Drag { Split, Pan }

pub struct View {
    /// Points per layer pixel; None: fit the painted area.
    zoom:   Option<f32>,
    /// Layer pixel at the middle (None: the area's center).
    center: Option<(f32, f32)>,
    /// Where the line is, 0..1 across.
    split:  f32,
    drag:   Option<Drag>,
    before: Option<(Crop, TextureHandle)>,
    after:  Option<(u64, Crop, TextureHandle)>,
    asked:  Option<u64>,
    /// Since when (egui time, s) the shown result is behind the values.
    behind: Option<f64>,
    worker: Worker,
}

impl View {
    pub fn new(ctx: &egui::Context) -> View {
        View { zoom: None, center: None, split: 0.5, drag: None, before: None, after: None, asked: None, behind: None, worker: Worker::new(ctx) }
    }

    pub fn show(&mut self, ui: &mut Ui, src: &Arc<Source>, filter: &Filter, height: f32) {
        let (r, resp) = ui.allocate_exact_size(vec2(WIDTH, height), Sense::click_and_drag());
        let (w, h) = (src.full.w as f32, src.full.h as f32);
        let a = src.area;
        let (aw, ah) = ((a.2 - a.0).max(1) as f32, (a.3 - a.1).max(1) as f32);
        let fit = (r.width() / aw).min(r.height() / ah).min(8.0);
        let mut s = self.zoom.unwrap_or(fit);
        let (mut cx, mut cy) = self.center.unwrap_or(((a.0 + a.2) as f32 / 2.0, (a.1 + a.3) as f32 / 2.0));

        // Zoom: the wheel (around the pointer), a double-click (100 % /
        // fit).
        let pointer = resp.hover_pos();
        let wheel = if resp.hovered() { ui.input(|i| i.raw_scroll_delta.y) } else { 0.0 };
        let mut zoom_to = None;
        if wheel != 0.0 { zoom_to = Some(step(s, wheel > 0.0)); }
        if resp.double_clicked() { zoom_to = Some(if (s - 1.0).abs() < 1e-3 { fit } else { 1.0 }); }

        // The buttons under the view.
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut z = None;
            if ui.selectable_label(self.zoom.is_none(), "Fit").clicked() { z = Some(None); }
            if ui.selectable_label(self.zoom == Some(1.0), "100 %").clicked() { z = Some(Some(1.0)); }
            if ui.button("−").clicked() { z = Some(Some(step(s, false))); }
            if ui.button("+").clicked() { z = Some(Some(step(s, true))); }
            ui.label(RichText::new(format!("{:.0} %", s * 100.0)).color(theme::TEXT_WEAK));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("drag the line to compare").small().color(theme::TEXT_WEAK));
            });
            z
        });
        match row.inner {
            Some(None) => { self.zoom = None; self.center = None; s = fit; (cx, cy) = ((a.0 + a.2) as f32 / 2.0, (a.1 + a.3) as f32 / 2.0); }
            Some(Some(z)) => zoom_to = Some(z),
            None => {}
        }
        if let Some(z) = zoom_to {
            // Keep the layer pixel under the pointer (or the middle) still.
            let at = pointer.unwrap_or(r.center());
            let (px, py) = (cx + (at.x - r.center().x) / s, cy + (at.y - r.center().y) / s);
            cx = px - (at.x - r.center().x) / z;
            cy = py - (at.y - r.center().y) / z;
            s = z;
            self.zoom = Some(z);
        }

        // Drags: the line, or the picture (pan).
        let split_x = r.left() + self.split * r.width();
        if resp.drag_started() {
            let x = ui.input(|i| i.pointer.press_origin()).map_or(0.0, |p| p.x);
            self.drag = Some(if (x - split_x).abs() < 10.0 { Drag::Split } else { Drag::Pan });
        }
        if resp.dragged() {
            match self.drag {
                Some(Drag::Split) => if let Some(p) = resp.interact_pointer_pos() { self.split = ((p.x - r.left()) / r.width()).clamp(0.0, 1.0); },
                _ => {
                    let d = resp.drag_delta();
                    cx -= d.x / s;
                    cy -= d.y / s;
                    if self.zoom.is_none() { self.zoom = Some(s); }
                }
            }
        }
        if resp.drag_stopped() { self.drag = None; }
        // Keep the view on the layer.
        let (vw, vh) = (r.width() / s, r.height() / s);
        let clamp = |c: f32, v: f32, total: f32| if v >= total { total / 2.0 } else { c.clamp(v / 2.0, total - v / 2.0) };
        cx = clamp(cx, vw, w);
        cy = clamp(cy, vh, h);
        if self.zoom.is_some() || self.center.is_some() { self.center = Some((cx, cy)); }
        let (x0, y0) = (cx - vw / 2.0, cy - vh / 2.0);

        // Which pixels: the smallest reduced copy that still has a pixel
        // per screen pixel (a thumbnail at Fit), the layer's own from 100 %.
        let k = src.k_for(s, ui.ctx().pixels_per_point());
        let level = src.level(k);
        let kf = k as f32;
        let shown = (
            ((x0 / kf).floor().max(0.0) as usize).min(level.w),
            ((y0 / kf).floor().max(0.0) as usize).min(level.h),
            (((x0 + vw) / kf).ceil().max(0.0) as usize).min(level.w),
            (((y0 + vh) / kf).ceil().max(0.0) as usize).min(level.h),
        );
        let fl = if k > 1 { filter.scaled(1.0 / kf) } else { filter.clone() };
        let crop = Crop::new(level, k, shown, fl.reach() as usize);
        let empty = shown.2 <= shown.0 || shown.3 <= shown.1;
        let nearest = s * kf >= 1.5;
        let options = if nearest { egui::TextureOptions::NEAREST } else { egui::TextureOptions::LINEAR };
        if !empty && self.before.as_ref().map(|b| b.0) != Some(crop) {
            let (px, _) = cut(level, crop.shown);
            let (sw, sh) = crop.shown_size();
            let img = egui::ColorImage::from_rgba_unmultiplied([sw, sh], &px);
            match &mut self.before {
                Some((c, t)) => { t.set(img, options); *c = crop; }
                None => self.before = Some((crop, ui.ctx().load_texture("filter-before", img, options))),
            }
        }
        let key = key_of(&fl, &crop);
        if !empty && self.after.as_ref().map(|a| a.0) != Some(key) && self.asked != Some(key) {
            self.worker.ask(key, fl, crop, Arc::clone(src));
            self.asked = Some(key);
        }
        // Every result for this crop is shown, even one for values already
        // changed again: the preview follows a moving slider instead of
        // waiting for it to stop.
        for (k2, c2, px) in self.worker.take() {
            if c2 != crop { continue; }
            let (sw, sh) = c2.shown_size();
            let img = egui::ColorImage::from_rgba_unmultiplied([sw, sh], &px);
            match &mut self.after {
                Some((k, c, t)) => { t.set(img, options); *k = k2; *c = c2; }
                None => self.after = Some((k2, c2, ui.ctx().load_texture("filter-after", img, options))),
            }
        }
        let now = ui.input(|i| i.time);
        let busy = !empty && self.after.as_ref().map(|a| a.0) != Some(key);
        self.behind = if busy { Some(self.behind.unwrap_or(now)) } else { None };
        // A working sign only when the result keeps you waiting.
        let slow = self.behind.is_some_and(|t| now - t > 0.3);
        if busy && !slow { ui.ctx().request_repaint_after(std::time::Duration::from_millis(320)); }

        // Draw.
        let p = ui.painter_at(r);
        p.rect_filled(r, 3.0, theme::DESK);
        let dest = Rect::from_min_max(
            pos2(r.left() + (shown.0 as f32 * kf - x0) * s, r.top() + (shown.1 as f32 * kf - y0) * s),
            pos2(r.left() + (shown.2 as f32 * kf - x0) * s, r.top() + (shown.3 as f32 * kf - y0) * s),
        );
        if !empty {
            if src.transparent { crate::widgets::checker(&p, dest.intersect(r), 8.0); }
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            let split_x = r.left() + self.split * r.width();
            let left = Rect::from_min_max(r.min, pos2(split_x, r.bottom()));
            let right = Rect::from_min_max(pos2(split_x, r.top()), r.max);
            if let Some((_, t)) = &self.before {
                ui.painter_at(left).image(t.id(), dest, uv, Color32::WHITE);
            }
            match &self.after {
                // (The result of an older value on the same crop is closer
                // than none while the new one comes.)
                Some((_, c, t)) if *c == crop => { ui.painter_at(right).image(t.id(), dest, uv, Color32::WHITE); }
                _ => if let Some((_, t)) = &self.before {
                    ui.painter_at(right).image(t.id(), dest, uv, Color32::from_white_alpha(120));
                },
            }
            // The line, with its grip.
            p.line_segment([pos2(split_x, r.top()), pos2(split_x, r.bottom())], Stroke::new(1.5_f32, Color32::WHITE));
            p.line_segment([pos2(split_x + 1.5, r.top()), pos2(split_x + 1.5, r.bottom())], Stroke::new(1.0_f32, Color32::from_black_alpha(120)));
            let grip = pos2(split_x, r.center().y);
            p.circle(grip, 9.0, theme::PANEL, Stroke::new(1.5_f32, Color32::WHITE));
            for d in [-1.0f32, 1.0] {
                let tip = grip + vec2(5.0 * d, 0.0);
                p.add(egui::Shape::convex_polygon(vec![tip, grip + vec2(1.5 * d, -3.5), grip + vec2(1.5 * d, 3.5)], theme::TEXT, Stroke::NONE));
            }
            if self.split > 0.12 { draw::corner(&p, r, Align2::LEFT_TOP, "Before"); }
            if self.split < 0.88 { draw::corner(&p, r, Align2::RIGHT_TOP, "After"); }
            if slow {
                // Three dots turning, left of the label (which keeps its
                // size).
                let c = r.right_top() + vec2(-52.0, 11.0);
                let phase = (now * 3.0) as usize;
                for i in 0..3 {
                    let on = i == phase % 3;
                    p.circle_filled(c + vec2(i as f32 * 6.0 - 6.0, 0.0), 2.0, if on { Color32::WHITE } else { Color32::from_white_alpha(90) });
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        } else {
            p.text(r.center(), Align2::CENTER_CENTER, "Nothing painted here", egui::FontId::proportional(12.0), theme::TEXT_WEAK);
        }
        p.rect_stroke(r, 3.0, Stroke::new(1.0_f32, theme::BORDER));
        // The hand opens over the picture and closes on it while it is
        // dragged (even out of the view); the line gets the resize arrows.
        let holding = resp.dragged() || resp.is_pointer_button_down_on();
        let cursor = match self.drag {
            Some(Drag::Split) if holding => Some(egui::CursorIcon::ResizeHorizontal),
            Some(Drag::Pan) if holding => Some(egui::CursorIcon::Grabbing),
            _ if resp.hovered() => {
                let near = pointer.is_some_and(|q| (q.x - split_x).abs() < 10.0);
                Some(if near { egui::CursorIcon::ResizeHorizontal } else if holding { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab })
            }
            _ => None,
        };
        if let Some(c) = cursor { ui.ctx().set_cursor_icon(c); }
    }
}

/// The next zoom step up or down from `z`.
fn step(z: f32, up: bool) -> f32 {
    if up { ZOOM_STEPS.iter().copied().find(|s| *s > z * 1.01).unwrap_or(8.0) } else { ZOOM_STEPS.iter().rev().copied().find(|s| *s < z / 1.01).unwrap_or(0.05) }
}

fn key_of(f: &Filter, crop: &Crop) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (f.kind, f.color, crop).hash(&mut h);
    for v in &f.values { v.to_bits().hash(&mut h); }
    h.finish()
}
