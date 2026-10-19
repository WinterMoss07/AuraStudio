//! Layers docker (CSP / Krita): blend mode + opacity of the active node,
//! action icons, then the layer tree: eye, thumbnail / folder, "100 %
//! Normal" over the name, lock. Rows drag and drop above, below, or into
//! a folder.

use std::collections::{HashMap, HashSet};

use aurastudio_layers::{FolderBlend, LayerStack, Node, Place, Row};
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
                    rgba.extend_from_slice(&layer.tiles.pixel(sx as u32, sy as u32));
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
    match m.layers.removal_refused() {
        Some(why) => notice.set(why),
        None => {
            m.layers.remove_active();
            a.recomposite = true;
        }
    }
}

/// A layer row being dragged (egui drag and drop payload).
struct LayerDrag(u64);

// --- Docker ---------------------------------------------------------------

pub fn show(ui: &mut Ui, st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    // Active node settings (CSP / Krita): blend mode + opacity.
    let full = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let left = ui.cursor().left();
        node_blend_combo(ui, m, a);
        // The rest of the row (available_width overshoots in a row).
        let w = (full - (ui.cursor().left() - left)).max(40.0);
        let node = m.layers.active_node_mut();
        let pct = (node.opacity() * 100.0).round();
        if let Some(v) = crate::widgets::fill_slider(ui, "layer_opacity", crate::widgets::percent("Opacity"), pct, w)
        {
            *node.opacity_mut() = v / 100.0;
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
            if icon_button(ui, Icon::Delete, "Delete", m.layers.removal_refused().is_none()) {
                delete_layer(notice, m, a);
            }
        });
    });

    // A layer being renamed may have been deleted meanwhile.
    if st.renaming.as_ref().is_some_and(|(id, _, _)| m.layers.node(*id).is_none()) {
        st.renaming = None;
    }

    let list_h = ui.available_height().max(ROW_H);
    let mut event = None;
    let mut dropped = None;
    Frame::none().fill(theme::DESK).rounding(6.0).show(ui, |ui| {
        ui.set_min_height(list_h);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for row in m.layers.rows() {
                let (e, resp) = layer_row(ui, st, m, &row);
                if let Some(e) = e { event = Some((row.id, e)); }
                if let Some(d) = drop_target(ui, &resp, &row) { dropped = Some(d); }
            }
        });
    });
    if let Some(d) = egui::DragAndDrop::payload::<LayerDrag>(ui.ctx()) {
        if let Some(n) = m.layers.node(d.0) { drag_ghost(ui.ctx(), n.name()); }
    }
    if let Some((id, target, place)) = dropped {
        if m.layers.move_node(id, target, place) { a.recomposite = true; }
    }

    // Applied after the loop: the rows borrow the stack while drawing.
    if let Some((id, e)) = event {
        let node = m.layers.node_mut(id).expect("row node exists");
        match e {
            RowEvent::Select => m.layers.set_active(id),
            RowEvent::ToggleVisible => {
                *node.visible_mut() ^= true;
                a.recomposite = true;
            }
            RowEvent::ToggleLock => {
                *node.locked_mut() ^= true;
                a.modified = true;
            }
            RowEvent::Renamed => a.modified = true,
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

enum RowEvent { Select, ToggleVisible, ToggleLock, ToggleCollapse, Renamed }

/// Drag and drop on a row: where the dragged layer would land (a line, or
/// the folder outlined), and the move once released there.
fn drop_target(ui: &Ui, resp: &egui::Response, row: &Row) -> Option<(u64, u64, Place)> {
    let dragged = resp.dnd_hover_payload::<LayerDrag>()?;
    if dragged.0 == row.id { return None; }
    let pointer = ui.ctx().pointer_interact_pos()?;
    let r = resp.rect;
    let t = (pointer.y - r.top()) / r.height();
    let place = match (row.is_folder, t) {
        (true, t) if (0.25..0.75).contains(&t) => Place::Inside,
        (_, t) if t < 0.5 => Place::Above,
        _ => Place::Below,
    };
    let p = ui.painter();
    let stroke = Stroke::new(2.0_f32, theme::DROP);
    match place {
        Place::Inside => { p.rect_stroke(r.shrink(1.0), 5.0, stroke); }
        Place::Above => { p.hline(r.x_range(), r.top(), stroke); }
        Place::Below => { p.hline(r.x_range(), r.bottom(), stroke); }
    }
    let released = resp.dnd_release_payload::<LayerDrag>()?;
    Some((released.0, row.id, place))
}

fn layer_row(ui: &mut Ui, st: &mut LayersState, m: &mut Model, row: &Row) -> (Option<RowEvent>, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
    if resp.drag_started() { resp.dnd_set_drag_payload(LayerDrag(row.id)); }
    let event = row_content(ui, st, m, row, rect, &resp);
    (event, resp)
}

fn row_content(ui: &mut Ui, st: &mut LayersState, m: &mut Model, row: &Row, rect: Rect, resp: &egui::Response) -> Option<RowEvent> {
    let node = m.layers.node(row.id)?;
    let (name, visible, opacity, locked) = (node.name().to_owned(), node.visible(), node.opacity(), node.locked());
    let blend = node.blend_name();
    let active = m.layers.active_id() == row.id;

    let painter = ui.painter_at(rect);

    let bg = if active { theme::ACCENT } else if resp.hovered() { theme::WIDGET } else { Color32::TRANSPARENT };
    painter.rect_filled(rect.shrink2(vec2(2.0, 0.0)), 5.0, bg);

    // Eye, at the far left whatever the depth (CSP / Krita).
    let eye = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(22.0, 22.0));
    let (eye_icon, eye_col) = if visible { (Icon::Eye, theme::TEXT) } else { (Icon::EyeClosed, if active { Color32::from_white_alpha(170) } else { theme::TEXT_WEAK }) };
    crate::icons::paint(ui, eye_icon, eye, 16.0, eye_col);

    // Lock, at the far right (Krita).
    let lock = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
    let lock_col = if locked {
        theme::TEXT
    } else if row.locked_by_parent {
        theme::TEXT_WEAK
    } else if resp.hovered() {
        theme::TEXT_WEAK.gamma_multiply(0.7)
    } else {
        if active { Color32::from_white_alpha(110) } else { theme::TEXT_WEAK.gamma_multiply(0.35) }
    };
    let lock_icon = if locked || row.locked_by_parent { Icon::Lock } else { Icon::LockOpen };
    crate::icons::paint(ui, lock_icon, lock, 15.0, lock_col);

    // Content, indented by folder depth.
    let x = eye.right() + 4.0 + row.depth as f32 * INDENT;
    let mut toggle_rect = Rect::NOTHING;
    let icon_box = if row.is_folder {
        // Disclosure triangle + folder icon.
        toggle_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 20.0));
        let chevron = if row.collapsed { Icon::Collapsed } else { Icon::Expanded };
        crate::icons::paint(ui, chevron, toggle_rect, 13.0, theme::TEXT_WEAK);
        let b = Rect::from_center_size(pos2(toggle_rect.right() + 4.0 + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let folder = if row.collapsed { Icon::Folder } else { Icon::FolderOpen };
        crate::icons::paint(ui, folder, b, 20.0, if active { theme::TEXT } else { theme::TEXT_WEAK });
        b
    } else {
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        if let Some(tex) = st.thumbs.get(&row.id) {
            let s = tex.size_vec2();
            let k = (THUMB / s.x).min(THUMB / s.y);
            let r = Rect::from_center_size(b.center(), s * k);
            checker(&painter, r, 4.0);
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, if active { theme::TEXT } else { theme::BORDER }));
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
        if active { Color32::from_white_alpha(200) } else { theme::TEXT_WEAK },
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
            st.renaming = None;
            if !new_name.is_empty() && new_name != name {
                if let Some(n) = m.layers.node_mut(row.id) { *n.name_mut() = new_name; }
                return Some(RowEvent::Renamed);
            }
        }
    } else {
        // Long names (an imported image's file name) end with "…" before
        // the lock instead of running under it.
        crate::widgets::elided_text(
            &painter,
            pos2(text_x, rect.center().y + 7.0),
            &name,
            egui::FontId::proportional(13.0),
            if active { Color32::WHITE } else { theme::TEXT },
            name_rect.width(),
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

