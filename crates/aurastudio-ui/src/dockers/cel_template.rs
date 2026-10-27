//! Cel template docker: the cels of the animation folder chosen (or of
//! its sheet shown), listed top to bottom as the Layers docker lists a
//! stack: each new cel stacks above the last. The radio picks the cel
//! strokes go to (in every sheet); eye and lock are the sheet's; the
//! lighthouse marks the reference of every sheet (the Ink: what the Colo's
//! fill reads); Apply to folder gives every sheet (and the sheets to come)
//! those states. Below, the template itself: add, remove, drag to reorder,
//! rename (double click).

use egui::{vec2, Color32, RichText, Sense, Stroke, Ui};

use aurastudio_layers::{Cel, LayerStack, Node};

use crate::icons::Icon;
use crate::widgets::icon_button;
use crate::{theme, Actions, Model};

/// Rows of the docker.
const ROW_H: f32 = 24.0;

/// What the docker keeps between frames.
#[derive(Default)]
pub struct CelState {
    /// The cel being renamed: rank, text, focus asked.
    pub rename: Option<(usize, String, bool)>,
    /// A cel to delete, waiting for the answer: folder, rank.
    ask: Option<(u64, usize)>,
}

impl CelState {
    /// A question is open (the canvas waits).
    pub fn is_asking(&self) -> bool { self.ask.is_some() }
}

/// A row being dragged (egui drag and drop payload): its rank.
struct CelDrag(usize);

/// The sheet whose cels are shown: the active one, else the one the
/// folder shows now.
fn shown_sheet(layers: &LayerStack, folder: u64, sheet: Option<u64>) -> Option<u64> {
    sheet.or_else(|| layers.exposed(folder))
}

/// Cel layer ids of sheet `frame`, top to bottom.
fn cels_of(layers: &LayerStack, frame: u64) -> Vec<u64> {
    match layers.node(frame) {
        Some(Node::Folder(f)) => f.children.iter().rev().map(Node::id).collect(),
        _ => Vec::new(),
    }
}

pub fn show(ui: &mut Ui, st: &mut CelState, m: &mut Model, a: &mut Actions) {
    let Some((folder, frame)) = m.layers.anim_of(m.layers.active_id()) else {
        ui.label(RichText::new("Select an animation folder or one of its sheets.").color(theme::TEXT_WEAK));
        return;
    };
    let Some(anim) = m.layers.anim(folder).cloned() else { return };
    let time = m.layers.time();
    let shown = shown_sheet(m.layers, folder, frame);
    let folder_name = m.layers.node(folder).map_or(String::new(), |n| n.name().to_owned());

    // Where we are: a sheet exposed from now, a hold, or nothing yet.
    let name = |id: u64| m.layers.node(id).map_or(String::new(), |n| n.name().to_owned());
    let state = match (m.layers.sheet_at(folder, time), m.layers.exposed(folder)) {
        (Some(s), _) => format!("Frame {time}: {}", name(s)),
        (None, Some(s)) => format!("Frame {time}: {} held, drawing makes a new sheet", name(s)),
        (None, None) => format!("Frame {time}: empty, drawing makes a sheet"),
    };
    ui.label(RichText::new(&folder_name).strong());
    ui.label(RichText::new(state).small().color(theme::TEXT_WEAK));
    ui.add_space(4.0);

    let cels = shown.map(|f| cels_of(m.layers, f)).unwrap_or_default();
    let mut chosen = anim.cel;
    let mut changed_states = false;
    let mut lock_edit = false;
    let mut reference_edit = None;
    let mut dropped: Option<(usize, usize)> = None;
    for (r, t) in anim.template.iter().enumerate() {
        let layer = cels.get(r).copied();
        let (visible, locked) = match layer.and_then(|id| m.layers.node(id)) {
            Some(n) => (n.visible(), n.locked()),
            None => (t.visible, t.locked),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
        if resp.drag_started() { resp.dnd_set_drag_payload(CelDrag(r)); }
        let on = r == anim.cel;
        let p = ui.painter_at(rect);
        let bg = if on { theme::ACCENT } else if resp.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
        p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 2.5, bg);
        // Eye on the left, lock on the right (as in the Layers docker).
        let eye = egui::Rect::from_center_size(egui::pos2(rect.left() + 16.0, rect.center().y), vec2(22.0, 22.0));
        let lock = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
        crate::icons::paint(ui, if visible { Icon::Eye } else { Icon::EyeClosed }, eye, 14.0, if visible { theme::TEXT } else { theme::TEXT_WEAK });
        crate::icons::paint_lock(ui, lock, locked, false, resp.hovered(), on);
        // The reference mark, before the lock.
        let mark = egui::Rect::from_center_size(egui::pos2(lock.left() - 12.0, rect.center().y), vec2(20.0, 22.0));
        let mark_col = if t.reference { if on { Color32::WHITE } else { Color32::from_rgb(240, 200, 90) } }
            else if on { Color32::from_white_alpha(70) } else { theme::TEXT_WEAK.gamma_multiply(0.3) };
        crate::icons::paint(ui, Icon::Reference, mark, 14.0, mark_col);
        // The radio: where strokes go.
        let dot = egui::pos2(eye.right() + 12.0, rect.center().y);
        p.circle_stroke(dot, 6.0, Stroke::new(1.5_f32, if on { Color32::WHITE } else { theme::TEXT_WEAK }));
        if on { p.circle_filled(dot, 3.2, Color32::WHITE); }
        let text_x = dot.x + 14.0;
        let colo_w = if t.colo { 34.0 } else { 0.0 };
        let name_rect = egui::Rect::from_min_max(egui::pos2(text_x, rect.top() + 4.0), egui::pos2(mark.left() - colo_w - 4.0, rect.bottom() - 4.0));
        if st.rename.as_ref().is_some_and(|x| x.0 == r) {
            let (_, buf, first) = st.rename.as_mut().unwrap();
            let edit = ui.put(name_rect, egui::TextEdit::singleline(buf));
            if *first { edit.request_focus(); *first = false; }
            if edit.lost_focus() {
                let name = buf.trim().to_owned();
                st.rename = None;
                if !name.is_empty() && name != t.name && !anim.template.iter().any(|x| x.name == name) {
                    m.layers.rename_cel(folder, r, &name);
                    a.modified = true;
                    a.layer_edit = Some("Rename cel");
                }
            }
        } else {
            crate::widgets::elided_text(&p, egui::pos2(text_x, rect.center().y), &t.name, egui::FontId::proportional(13.0),
                if on { Color32::WHITE } else { theme::TEXT }, name_rect.width());
        }
        if t.colo {
            p.text(egui::pos2(mark.left() - 4.0, rect.center().y), egui::Align2::RIGHT_CENTER, "colo", egui::FontId::proportional(10.5),
                if on { Color32::from_white_alpha(200) } else { theme::TEXT_WEAK });
        }
        // Dragged here: a line where the cel would land (above or below).
        if let Some(d) = resp.dnd_hover_payload::<CelDrag>() {
            if d.0 != r {
                let upper = ui.ctx().pointer_interact_pos().is_some_and(|q| q.y < rect.center().y);
                let to = if upper { if d.0 < r { r - 1 } else { r } } else if d.0 < r { r } else { r + 1 };
                let y = if upper { rect.top() } else { rect.bottom() };
                if to != d.0 { p.hline(rect.x_range(), y, Stroke::new(2.0_f32, theme::DROP)); }
                if let Some(rel) = resp.dnd_release_payload::<CelDrag>() { if to != rel.0 { dropped = Some((rel.0, to)); } }
            }
        }
        let pt = resp.interact_pointer_pos();
        if resp.double_clicked() && pt.is_some_and(|pt| name_rect.contains(pt)) {
            st.rename = Some((r, t.name.clone(), true));
        } else if resp.clicked() {
            match pt {
                Some(pt) if eye.contains(pt) => match layer {
                    Some(id) => { if let Some(n) = m.layers.node_mut(id) { *n.visible_mut() ^= true; } changed_states = true; }
                    None => { set_template_state(m.layers, folder, r, Some(!visible), None); changed_states = true; }
                },
                Some(pt) if lock.contains(pt) => match layer {
                    Some(id) => { if let Some(n) = m.layers.node_mut(id) { *n.locked_mut() ^= true; } lock_edit = true; }
                    None => { set_template_state(m.layers, folder, r, None, Some(!locked)); lock_edit = true; }
                },
                Some(pt) if mark.contains(pt) => reference_edit = Some((r, !t.reference)),
                _ => chosen = r,
            }
        }
    }
    if let Some((from, to)) = dropped {
        m.layers.move_cel(folder, from, to);
        a.recomposite = true;
        a.layer_edit = Some("Move cel");
    }
    if let Some((r, on)) = reference_edit {
        m.layers.set_cel_reference(folder, r, on);
        a.modified = true;
        a.layer_edit = Some("Reference cel");
    }
    if chosen != anim.cel { m.layers.set_cel(folder, chosen); }
    if changed_states { a.recomposite = true; }
    if lock_edit { a.modified = true; a.layer_edit = Some("Lock"); }

    ui.add_space(4.0);
    let apply = ui.add_enabled(shown.is_some(), egui::Button::new("Apply to folder"))
        .on_hover_text("Every sheet (and the sheets to come) gets these eyes and locks");
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
        let r = anim.cel;
        if icon_button(ui, Icon::Plus, "Add a cel above", true) {
            let mut k = 1;
            while anim.template.iter().any(|t| t.name == format!("Cel {k}")) { k += 1; }
            m.layers.add_cel(folder, r, Cel::new(&format!("Cel {k}"), false));
            a.recomposite = true;
            a.layer_edit = Some("Add cel");
        }
        if icon_button(ui, Icon::Minus, "Remove this cel (in every sheet)", n > 1) {
            // Sheets drawn on it: ask first (it goes from all of them).
            if m.layers.cel_content(folder, r).1 > 0 {
                st.ask = Some((folder, r));
            } else {
                m.layers.remove_cel(folder, r);
                a.recomposite = true;
                a.layer_edit = Some("Remove cel");
            }
        }
    });
    ui.label(RichText::new("Drag a cel to move it, double click a name to rename it.").small().color(theme::TEXT_WEAK));
    ask_dialog(ui.ctx(), st, m, a);
}

/// Deleting a cel takes it from every sheet of the folder: say how many
/// have a drawing on it. Cancel is the default (Enter, Escape).
fn ask_dialog(ctx: &egui::Context, st: &mut CelState, m: &mut Model, a: &mut Actions) {
    let Some((folder, r)) = st.ask else { return };
    // (Gone meanwhile: an undo, another document.)
    let Some(name) = m.layers.anim(folder).and_then(|t| t.template.get(r)).map(|c| c.name.clone()) else { st.ask = None; return };
    let (sheets, drawn) = m.layers.cel_content(folder, r);
    let mut delete = false;
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("cel_delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(format!("Delete cel {name}?")).strong());
        ui.add_space(4.0);
        ui.label(format!("{sheets} sheet{}, {drawn} contain{} a drawing on it.", super::layers::plural(sheets), if drawn == 1 { "s" } else { "" }));
        ui.label(RichText::new("It goes from every sheet of this folder, not just this one.").color(theme::TEXT_WEAK));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Delete cel").clicked() { delete = true; }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = egui::Button::new(RichText::new("Cancel").color(theme::TEXT)).fill(theme::ACCENT);
                if ui.add(b).clicked() { cancel = true; }
            });
        });
    });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)) { cancel = true; }
    if delete {
        st.ask = None;
        if m.layers.remove_cel(folder, r) {
            a.recomposite = true;
            a.layer_edit = Some("Remove cel");
        }
    } else if cancel {
        st.ask = None;
    }
}

/// No sheet yet: eye and lock are the template's (what sheets start
/// with).
fn set_template_state(layers: &mut LayerStack, folder: u64, r: usize, visible: Option<bool>, locked: Option<bool>) {
    let Some(Node::Folder(f)) = layers.node_mut(folder) else { return };
    let aurastudio_layers::FolderKind::Animation(a) = &mut f.kind else { return };
    let Some(t) = a.template.get_mut(r) else { return };
    if let Some(v) = visible { t.visible = v; }
    if let Some(l) = locked { t.locked = l; }
}
