//! Cel template docker: the sub-cels of the animation folder chosen (or
//! of its frame shown), top to bottom. The radio picks the sub-cel strokes
//! go to (in every frame); eye and lock are the frame's; Apply to folder
//! gives every frame (and the frames to come) those states. Below, the
//! template itself: add, remove, move, rename (double click).

use egui::{vec2, Color32, RichText, Sense, Stroke, Ui};

use aurastudio_layers::{LayerStack, Node, SubCel};

use crate::icons::Icon;
use crate::widgets::icon_button;
use crate::{theme, Actions, Model};

/// Rows of the docker.
const ROW_H: f32 = 24.0;

/// The frame whose sub-cels are shown: the active one, else the one the
/// folder shows now.
fn shown_frame(layers: &LayerStack, folder: u64, frame: Option<u64>) -> Option<u64> {
    frame.or_else(|| layers.exposed(folder))
}

/// Sub-cel layer ids of `frame`, top to bottom.
fn subs_of(layers: &LayerStack, frame: u64) -> Vec<u64> {
    match layers.node(frame) {
        Some(Node::Folder(f)) => f.children.iter().rev().map(Node::id).collect(),
        _ => Vec::new(),
    }
}

pub fn show(ui: &mut Ui, renaming: &mut Option<(usize, String, bool)>, m: &mut Model, a: &mut Actions) {
    let Some((folder, frame)) = m.layers.anim_of(m.layers.active_id()) else {
        ui.label(RichText::new("Select an animation folder or one of its frames.").color(theme::TEXT_WEAK));
        return;
    };
    let Some(anim) = m.layers.anim(folder).cloned() else { return };
    let time = m.layers.time();
    let shown = shown_frame(m.layers, folder, frame);
    let folder_name = m.layers.node(folder).map_or(String::new(), |n| n.name().to_owned());

    // Where we are: the frame drawn now, a hold, or nothing yet.
    let state = match (m.layers.frame_at(folder, time), shown.and_then(|f| m.layers.node(f)).and_then(Node::frame)) {
        (Some(_), _) => format!("Frame {time}"),
        (None, Some(k)) => format!("Frame {time}: holds Frame {k}, drawing makes Frame {time}"),
        (None, None) => format!("Frame {time}: empty, drawing makes it"),
    };
    ui.label(RichText::new(&folder_name).strong());
    ui.label(RichText::new(state).small().color(theme::TEXT_WEAK));
    ui.add_space(4.0);

    let subs = shown.map(|f| subs_of(m.layers, f)).unwrap_or_default();
    let mut chosen = anim.sub;
    let mut changed_states = false;
    let mut lock_edit = false;
    for (r, t) in anim.template.iter().enumerate() {
        let layer = subs.get(r).copied();
        let (visible, locked) = match layer.and_then(|id| m.layers.node(id)) {
            Some(n) => (n.visible(), n.locked()),
            None => (t.visible, t.locked),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let on = r == anim.sub;
        let p = ui.painter_at(rect);
        let bg = if on { theme::ACCENT } else if resp.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
        p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 2.5, bg);
        // Eye on the left, lock on the right (as in the Layers docker).
        let eye = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), vec2(22.0, 22.0));
        let lock = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
        crate::icons::paint(ui, if visible { Icon::Eye } else { Icon::EyeClosed }, eye, 14.0, if visible { theme::TEXT } else { theme::TEXT_WEAK });
        crate::icons::paint(ui, if locked { Icon::Lock } else { Icon::LockOpen }, lock, 13.0,
            if locked { theme::TEXT } else if on { Color32::from_white_alpha(110) } else { theme::TEXT_WEAK.gamma_multiply(0.5) });
        // The radio: where strokes go.
        let dot = egui::pos2(eye.right() + 12.0, rect.center().y);
        p.circle_stroke(dot, 6.0, Stroke::new(1.5_f32, if on { Color32::WHITE } else { theme::TEXT_WEAK }));
        if on { p.circle_filled(dot, 3.2, Color32::WHITE); }
        let text_x = dot.x + 14.0;
        let name_rect = egui::Rect::from_min_max(egui::pos2(text_x, rect.top() + 4.0), egui::pos2(lock.left() - 40.0, rect.bottom() - 4.0));
        if renaming.as_ref().is_some_and(|x| x.0 == r) {
            let (_, buf, first) = renaming.as_mut().unwrap();
            let edit = ui.put(name_rect, egui::TextEdit::singleline(buf));
            if *first { edit.request_focus(); *first = false; }
            if edit.lost_focus() {
                let name = buf.trim().to_owned();
                *renaming = None;
                if !name.is_empty() && name != t.name && !anim.template.iter().any(|x| x.name == name) {
                    m.layers.rename_sub(folder, r, &name);
                    a.modified = true;
                    a.layer_edit = Some("Rename sub-cel");
                }
            }
        } else {
            p.text(egui::pos2(text_x, rect.center().y), egui::Align2::LEFT_CENTER, &t.name, egui::FontId::proportional(13.0),
                if on { Color32::WHITE } else { theme::TEXT });
        }
        if t.colo {
            p.text(egui::pos2(lock.left() - 6.0, rect.center().y), egui::Align2::RIGHT_CENTER, "colo", egui::FontId::proportional(10.5),
                if on { Color32::from_white_alpha(200) } else { theme::TEXT_WEAK });
        }
        let pt = resp.interact_pointer_pos();
        if resp.double_clicked() && pt.is_some_and(|pt| name_rect.contains(pt)) {
            *renaming = Some((r, t.name.clone(), true));
        } else if resp.clicked() {
            match pt {
                Some(pt) if eye.contains(pt) => match layer {
                    Some(id) => { if let Some(n) = m.layers.node_mut(id) { *n.visible_mut() ^= true; } changed_states = true; }
                    None => { set_template_state(m.layers, folder, r, Some(!visible), None); }
                },
                Some(pt) if lock.contains(pt) => match layer {
                    Some(id) => { if let Some(n) = m.layers.node_mut(id) { *n.locked_mut() ^= true; } lock_edit = true; }
                    None => { set_template_state(m.layers, folder, r, None, Some(!locked)); lock_edit = true; }
                },
                _ => chosen = r,
            }
        }
    }
    if chosen != anim.sub { m.layers.set_sub(folder, chosen); }
    if changed_states { a.recomposite = true; }
    if lock_edit { a.modified = true; a.layer_edit = Some("Lock"); }

    ui.add_space(4.0);
    let apply = ui.add_enabled(shown.is_some(), egui::Button::new("Apply to folder"))
        .on_hover_text("Every frame (and the frames to come) gets these eyes and locks");
    if apply.clicked() {
        if let Some(f) = shown {
            m.layers.apply_to_folder(f);
            a.recomposite = true;
            a.layer_edit = Some("Apply to folder");
        }
    }

    // The template itself.
    ui.add_space(8.0);
    ui.label(RichText::new("Template").small().color(theme::TEXT_WEAK));
    ui.horizontal(|ui| {
        let n = anim.template.len();
        let r = anim.sub;
        if icon_button(ui, Icon::Plus, "Add a sub-cel above", true) {
            let mut k = 1;
            while anim.template.iter().any(|t| t.name == format!("Sub-cel {k}")) { k += 1; }
            m.layers.add_sub(folder, r, SubCel::new(&format!("Sub-cel {k}"), false));
            a.recomposite = true;
            a.layer_edit = Some("Add sub-cel");
        }
        if icon_button(ui, Icon::Minus, "Remove this sub-cel (in every frame)", n > 1) {
            m.layers.remove_sub(folder, r);
            a.recomposite = true;
            a.layer_edit = Some("Remove sub-cel");
        }
        if icon_button(ui, Icon::ArrowUp, "Move up", r > 0) {
            m.layers.move_sub(folder, r, r - 1);
            a.recomposite = true;
            a.layer_edit = Some("Move sub-cel");
        }
        if icon_button(ui, Icon::ArrowDown, "Move down", r + 1 < n) {
            m.layers.move_sub(folder, r, r + 1);
            a.recomposite = true;
            a.layer_edit = Some("Move sub-cel");
        }
    });
    ui.label(RichText::new("Double click a name to rename it.").small().color(theme::TEXT_WEAK));
}

/// No frame drawn yet: eye and lock are the template's (what frames
/// start with).
fn set_template_state(layers: &mut LayerStack, folder: u64, r: usize, visible: Option<bool>, locked: Option<bool>) {
    let Some(Node::Folder(f)) = layers.node_mut(folder) else { return };
    let aurastudio_layers::FolderKind::Animation(a) = &mut f.kind else { return };
    let Some(t) = a.template.get_mut(r) else { return };
    if let Some(v) = visible { t.visible = v; }
    if let Some(l) = locked { t.locked = l; }
}
