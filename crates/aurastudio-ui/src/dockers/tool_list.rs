//! Tools docker: the chosen group's tools, or its brushes by library
//! group (the built-in sets, then each imported file), with a search and
//! the import of Photoshop / Procreate brushes.

use aurastudio_forge::Preset;
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::tools::{Group, Tool};
use crate::{theme, Model};

/// Height of a row (points).
const ROW: f32 = 34.0;

#[derive(Default)]
pub struct ToolListState {
    filter: String,
}

pub fn show(ui: &mut Ui, st: &mut ToolListState, m: &mut Model, a: &mut crate::Actions) {
    let group = Group::current(*m.tool, *m.preset, m.presets);
    if group.is_brushes() { brushes(ui, st, group, m, a); } else { tools(ui, group, m); }
}

fn brushes(ui: &mut Ui, st: &mut ToolListState, group: Group, m: &mut Model, a: &mut crate::Actions) {
    let presets = m.presets;
    ui.horizontal(|ui| {
        let w = ui.available_width() - 70.0;
        ui.add_sized(vec2(w.max(40.0), 22.0), egui::TextEdit::singleline(&mut st.filter).hint_text("Search brushes"));
        if ui.add_sized(vec2(64.0, 22.0), egui::Button::new("Import…")).on_hover_text("Import brushes: Photoshop .abr, Procreate .brush / .brushset").clicked() {
            a.import_brushes = true;
        }
    });
    ui.add_space(4.0);
    let filter = st.filter.trim().to_lowercase();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let mut any = false;
        let mut heading: Option<&str> = None;
        for (i, p) in presets.iter().enumerate() {
            let shown = if filter.is_empty() { Some(p.kind) == group.kind() } else { p.name.to_lowercase().contains(&filter) };
            if !shown { continue; }
            any = true;
            if heading != Some(p.group.as_str()) {
                heading = Some(&p.group);
                ui.add_space(if i == 0 { 0.0 } else { 4.0 });
                ui.label(RichText::new(&p.group).small().color(theme::TEXT_WEAK));
            }
            if brush_row(ui, p, i == *m.preset).clicked() {
                // A brush of another group (search): go there.
                let g = Group::of_kind(p.kind);
                *m.tool = if g == Group::Eraser { Tool::Eraser } else { Tool::Brush };
                // (The app swaps the settings: the brush left keeps its own.)
                *m.preset = i;
            }
        }
        if !any { ui.label(RichText::new("No brush found").color(theme::TEXT_WEAK)); }
    });
}

/// The tools of a group that isn't brushes.
fn tools(ui: &mut Ui, group: Group, m: &mut Model) {
    ui.label(RichText::new(group.name()).strong());
    ui.add_space(2.0);
    ui.spacing_mut().item_spacing.y = 3.0;
    for &t in group.tools() {
        let selected = t == *m.tool;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        let resp = resp.on_hover_text(t.hint());
        ui.painter().rect_filled(rect, 2.0, if selected || resp.hovered() { theme::ROW_HOVER } else { theme::ROW });
        crate::icons::paint(ui, t.icon(), egui::Rect::from_min_size(rect.min, vec2(ROW, ROW)), 20.0, theme::TEXT);
        ui.painter().text(rect.left_center() + vec2(ROW + 4.0, 0.0), egui::Align2::LEFT_CENTER, t.name(), egui::FontId::proportional(13.0), theme::TEXT);
        if selected { ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.5_f32, theme::ACCENT), egui::StrokeKind::Middle); }
        if resp.clicked() { *m.tool = t; }
    }
}

fn brush_row(ui: &mut Ui, p: &Preset, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    if !ui.is_rect_visible(rect) { return resp; }
    let bg = if selected || resp.hovered() { theme::ROW_HOVER } else { theme::ROW };
    ui.painter().rect_filled(rect, 2.0, bg);
    // A dot of the brush's size and softness, then its name.
    let c = rect.left_center() + vec2(ROW / 2.0, 0.0);
    let r = (p.settings.size / 8.0).clamp(2.0, ROW / 2.0 - 4.0);
    let col = if p.settings.erase { theme::TEXT_WEAK } else { theme::TEXT };
    ui.painter().circle_filled(c, r, col.gamma_multiply(p.settings.opacity.max(0.3)));
    if p.settings.hardness < 0.5 { ui.painter().circle_filled(c, r * 1.4, col.gamma_multiply(0.25)); }
    ui.painter().text(rect.left_center() + vec2(ROW + 4.0, 0.0), egui::Align2::LEFT_CENTER, &p.name, egui::FontId::proportional(13.0), theme::TEXT);
    if selected { ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.5_f32, theme::ACCENT), egui::StrokeKind::Middle); }
    resp
}
