//! Layers docker (CSP / Krita): blend mode + opacity of the active node,
//! action icons at the top, then the layer tree with eye on the left,
//! thumbnail / folder icon, "100 % Normal" above the name, lock on the right.

use std::collections::{HashMap, HashSet};

use aurastudio_layers::{FolderBlend, LayerStack, Node, Row};
use egui::{pos2, vec2, Align, Color32, Frame, Layout, Rect, Sense, Stroke, Ui};

use crate::widgets::*;
use crate::{theme, Actions, BlendMode, Model, Notice};

const THUMB: f32 = 36.0;
const ROW_H: f32 = 44.0;
const INDENT: f32 = 14.0;

#[derive(Default)]
pub struct LayersState {
    thumbs:   HashMap<u64, egui::TextureHandle>,
    stale:    HashSet<u64>,
    /// Layer being renamed: (id, text, focus not requested yet).
    renaming: Option<(u64, String, bool)>,
}

impl LayersState {
    pub fn invalidate(&mut self, id: u64) {
        self.stale.insert(id);
    }

    pub fn invalidate_all(&mut self) {
        self.thumbs.clear();
    }

    /// Keep one small texture per paint layer, rebuilt when its pixels changed.
    pub fn refresh_thumbnails(&mut self, ctx: &egui::Context, layers: &LayerStack) {
        let ids = layers.layer_ids();
        let alive: HashSet<u64> = ids.iter().copied().collect();
        self.thumbs.retain(|id, _| alive.contains(id));

        for id in ids {
            let stale = self.stale.remove(&id);
            if self.thumbs.contains_key(&id) && !stale { continue; }
            let Some(Node::Layer(layer)) = layers.node(id) else { continue };

            let (w, h) = (layers.width as usize, layers.height as usize);
            let k = (64.0 / w.max(h) as f32).min(1.0);
            let tw = ((w as f32 * k).round() as usize).max(1);
            let th = ((h as f32 * k).round() as usize).max(1);
            let mut rgba = Vec::with_capacity(tw * th * 4);
            for y in 0..th {
                let sy = (y * h) / th;
                for x in 0..tw {
                    let sx = (x * w) / tw;
                    let i = (sy * w + sx) * 4;
                    rgba.extend_from_slice(&layer.pixels[i..i + 4]);
                }
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba);
            match self.thumbs.get_mut(&id) {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    let tex = ctx.load_texture(format!("layer-thumb-{id}"), image, egui::TextureOptions::LINEAR);
                    self.thumbs.insert(id, tex);
                }
            }
        }
    }
}

// --- Actions (shared with the Layer menu) ---------------------------------

pub fn new_layer(m: &mut Model, a: &mut Actions) {
    let n = m.layers.layer_count();
    m.layers.add_layer(&format!("Layer {n}"));
    a.recomposite = true;
}

pub fn new_folder(m: &mut Model, a: &mut Actions) {
    m.layers.add_folder("Folder");
    a.recomposite = true;
}

pub fn delete_layer(notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    if m.layers.remove_active() {
        a.recomposite = true;
    } else {
        notice.set("Can't delete the last layer");
    }
}

// --- Docker ---------------------------------------------------------------

pub fn show(ui: &mut Ui, st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    // Active node settings (CSP / Krita): blend mode + opacity.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        node_blend_combo(ui, m, a);
        let node = m.layers.active_node_mut();
        let mut pct = (node.opacity() * 100.0).round() as u32;
        ui.spacing_mut().slider_width = (ui.available_width() - 58.0).max(40.0);
        if ui.add(egui::Slider::new(&mut pct, 0..=100).suffix(" %")).on_hover_text("Layer opacity").changed() {
            *node.opacity_mut() = pct as f32 / 100.0;
            a.recomposite = true;
        }
    });

    // Actions at the top, like CSP.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if icon_button(ui, Icon::NewLayer, "New layer", true) { new_layer(m, a); }
        if icon_button(ui, Icon::NewFolder, "New folder", true) { new_folder(m, a); }
        if icon_button(ui, Icon::Duplicate, "Duplicate", true) {
            m.layers.duplicate_active();
            a.recomposite = true;
        }
        if icon_button(ui, Icon::Up, "Move up", true) {
            m.layers.move_active(true);
            a.recomposite = true;
        }
        if icon_button(ui, Icon::Down, "Move down", true) {
            m.layers.move_active(false);
            a.recomposite = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Delete, "Delete", m.layers.layer_count() > 1) {
                delete_layer(notice, m, a);
            }
        });
    });

    let list_h = ui.available_height().max(ROW_H);
    let mut event = None;
    Frame::none().fill(theme::SURFACE).rounding(6.0).show(ui, |ui| {
        ui.set_min_height(list_h);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for row in m.layers.rows() {
                if let Some(e) = layer_row(ui, st, m, &row) {
                    event = Some((row.id, e));
                }
            }
        });
    });

    // Applied after the loop: the rows borrow the stack while drawing.
    if let Some((id, e)) = event {
        let node = m.layers.node_mut(id).expect("row node exists");
        match e {
            RowEvent::Select => m.layers.set_active(id),
            RowEvent::ToggleVisible => {
                *node.visible_mut() ^= true;
                a.recomposite = true;
            }
            RowEvent::ToggleLock => *node.locked_mut() ^= true,
            RowEvent::ToggleCollapse => {
                if let Node::Folder(f) = node { f.collapsed ^= true; }
            }
        }
    }
}

/// Blend combo for the active node; folders also offer Pass Through.
fn node_blend_combo(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    let node = m.layers.active_node_mut();
    let label = node.blend_name();
    egui::ComboBox::from_id_salt("node_blend")
        .selected_text(label)
        .width(104.0)
        .show_ui(ui, |ui| match node {
            Node::Layer(l) => {
                for mode in BlendMode::ALL {
                    if ui.selectable_value(&mut l.blend, mode, mode.name()).changed() {
                        a.recomposite = true;
                    }
                }
            }
            Node::Folder(f) => {
                if ui.selectable_value(&mut f.blend, FolderBlend::PassThrough, "Pass Through").changed() {
                    a.recomposite = true;
                }
                for mode in BlendMode::ALL {
                    if ui.selectable_value(&mut f.blend, FolderBlend::Isolated(mode), mode.name()).changed() {
                        a.recomposite = true;
                    }
                }
            }
        });
}

enum RowEvent { Select, ToggleVisible, ToggleLock, ToggleCollapse }

fn layer_row(ui: &mut Ui, st: &mut LayersState, m: &mut Model, row: &Row) -> Option<RowEvent> {
    let node = m.layers.node(row.id)?;
    let (name, visible, opacity, locked) = (node.name().to_owned(), node.visible(), node.opacity(), node.locked());
    let blend = node.blend_name();
    let active = m.layers.active_id() == row.id;

    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let painter = ui.painter_at(rect);

    let bg = if active { theme::ACCENT_DIM } else if resp.hovered() { theme::WIDGET } else { Color32::TRANSPARENT };
    painter.rect_filled(rect.shrink2(vec2(2.0, 0.0)), 5.0, bg);

    // Eye, at the far left whatever the depth (CSP / Krita).
    let eye = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(22.0, 22.0));
    paint_eye(&painter, eye.center(), visible);

    // Lock, at the far right (Krita).
    let lock = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
    let lock_col = if locked {
        theme::TEXT
    } else if row.locked_by_parent {
        theme::TEXT_WEAK
    } else if resp.hovered() {
        theme::TEXT_WEAK.gamma_multiply(0.7)
    } else {
        theme::TEXT_WEAK.gamma_multiply(0.35)
    };
    paint_lock(&painter, lock.center(), locked || row.locked_by_parent, lock_col);

    // Content, indented by folder depth.
    let x = eye.right() + 4.0 + row.depth as f32 * INDENT;
    let mut toggle_rect = Rect::NOTHING;
    let icon_box = if row.is_folder {
        // Disclosure triangle + folder icon.
        toggle_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 20.0));
        let c = toggle_rect.center();
        let tri = if row.collapsed {
            vec![c + vec2(-2.5, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.5, 4.0)]
        } else {
            vec![c + vec2(-4.0, -2.5), c + vec2(4.0, -2.5), c + vec2(0.0, 3.0)]
        };
        painter.add(egui::Shape::convex_polygon(tri, theme::TEXT_WEAK, Stroke::NONE));
        let b = Rect::from_center_size(pos2(toggle_rect.right() + 4.0 + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        folder_shape(&painter, b.center(), 20.0, Stroke::new(1.4_f32, if active { theme::TEXT } else { theme::TEXT_WEAK }));
        b
    } else {
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        if let Some(tex) = st.thumbs.get(&row.id) {
            let s = tex.size_vec2();
            let k = (THUMB / s.x).min(THUMB / s.y);
            let r = Rect::from_center_size(b.center(), s * k);
            checker(&painter, r, 4.0);
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, if active { theme::ACCENT } else { theme::BORDER }));
        }
        b
    };

    // "100 % Normal" above the name, like CSP.
    let text_x = icon_box.right() + 8.0;
    let name_rect = Rect::from_min_max(pos2(text_x, rect.center().y - 4.0), pos2(lock.left() - 4.0, rect.center().y + 14.0));
    painter.text(
        pos2(text_x, rect.center().y - 9.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0} %  {}", opacity * 100.0, blend),
        egui::FontId::proportional(10.5),
        theme::TEXT_WEAK,
    );
    let renaming = st.renaming.as_ref().is_some_and(|(rid, _, _)| *rid == row.id);
    if renaming {
        let (_, buf, first) = st.renaming.as_mut().unwrap();
        let edit = ui.put(name_rect, egui::TextEdit::singleline(buf));
        // Focus once: requesting it every frame kept the field open after
        // Enter (focus came straight back).
        if *first {
            edit.request_focus();
            *first = false;
        }
        if edit.lost_focus() {
            let new_name = buf.trim().to_owned();
            if !new_name.is_empty() {
                if let Some(n) = m.layers.node_mut(row.id) { *n.name_mut() = new_name; }
            }
            st.renaming = None;
        }
    } else {
        painter.text(
            pos2(text_x, rect.center().y + 7.0),
            egui::Align2::LEFT_CENTER,
            &name,
            egui::FontId::proportional(13.0),
            if active { Color32::WHITE } else { theme::TEXT },
        );
    }

    let pt = resp.interact_pointer_pos();
    if resp.double_clicked() && !renaming && pt.is_some_and(|pt| pt.x >= text_x && pt.x < lock.left()) {
        st.renaming = Some((row.id, name, true));
        return None;
    }
    if resp.clicked() {
        let pt = pt?;
        if eye.contains(pt) { return Some(RowEvent::ToggleVisible); }
        if lock.contains(pt) { return Some(RowEvent::ToggleLock); }
        if toggle_rect.contains(pt) { return Some(RowEvent::ToggleCollapse); }
        return Some(RowEvent::Select);
    }
    None
}

/// Eye icon drawn with shapes.
fn paint_eye(painter: &egui::Painter, c: egui::Pos2, open: bool) {
    let col = if open { theme::TEXT } else { theme::TEXT_WEAK.gamma_multiply(0.6) };
    let pts: Vec<_> = (0..=24)
        .map(|k| {
            let a = k as f32 / 24.0 * std::f32::consts::TAU;
            pos2(c.x + 7.0 * a.cos(), c.y + 4.2 * a.sin() * (1.0 - 0.25 * a.cos().abs()))
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, Stroke::new(1.3_f32, col)));
    if open {
        painter.circle_filled(c, 2.2, col);
    } else {
        painter.line_segment([pos2(c.x - 6.5, c.y + 5.5), pos2(c.x + 6.5, c.y - 5.5)], Stroke::new(1.3_f32, col));
    }
}
