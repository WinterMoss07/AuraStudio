//! Brushes docker: the brush presets, grouped (AuraStudio, then the
//! MyPaint collections and imported packs), as a list of strokes painted
//! by each brush (brush_preview.rs), its name in the corner. Click
//! selects; each brush keeps its own size / opacity / blend mode, and the
//! selected one's stroke follows its settings.

use aurastudio_brushes::{Engine, Preset};
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::brush_preview::{PreviewCache, Slot};
use crate::{theme, Actions, ImportBrushes, Model};

/// Height of a brush's row (points).
const ROW: f32 = 46.0;

#[derive(Default)]
pub struct BrushesState {
    filter:   String,
    /// Stroke previews, shared with the Tool options docker.
    pub previews: PreviewCache,
    /// Number of presets the previews were made for (an import changes
    /// the list: they are redone).
    count:    usize,
}

pub fn show(ui: &mut Ui, st: &mut BrushesState, m: &mut Model, a: &mut Actions) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let search_w = (ui.available_width() - 76.0).max(60.0);
        ui.add_sized(vec2(search_w, 22.0), egui::TextEdit::singleline(&mut st.filter).hint_text("Search brushes"));
        ui.menu_button("Import…", |ui| {
            if ui.button("Brush files (.myb)…").clicked() {
                a.import_brushes = Some(ImportBrushes::Files);
                ui.close_menu();
            }
            if ui.button("Brush folder…").clicked() {
                a.import_brushes = Some(ImportBrushes::Folder);
                ui.close_menu();
            }
        });
    });
    ui.add_space(2.0);

    let filter = st.filter.trim().to_lowercase();
    let presets = m.presets;
    if presets.len() != st.count {
        st.count = presets.len();
        st.previews.clear_list();
    }
    let current_group = presets.get(*m.preset).map(|p| p.group.clone());
    let mut groups: Vec<&str> = Vec::new();
    for p in presets {
        if !groups.contains(&p.group.as_str()) { groups.push(&p.group); }
    }

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for group in groups {
            let members: Vec<usize> = presets.iter().enumerate()
                .filter(|(_, p)| p.group == group)
                .filter(|(_, p)| filter.is_empty() || p.name.to_lowercase().contains(&filter))
                .map(|(i, _)| i)
                .collect();
            if members.is_empty() { continue; }
            // Open: the selected brush's group, or every group while
            // searching.
            let open = !filter.is_empty() || current_group.as_deref() == Some(group);
            egui::CollapsingHeader::new(RichText::new(format!("{group}  ({})", members.len())).strong())
                .id_salt(("brush_group", group))
                .default_open(open)
                .open(if filter.is_empty() { None } else { Some(true) })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for &i in &members {
                        let selected = i == *m.preset;
                        if brush_row(ui, st, &presets[i], i, selected, m.brush).clicked() {
                            *m.preset = i;
                        }
                    }
                });
        }
    });
}

fn brush_row(ui: &mut Ui, st: &mut BrushesState, p: &Preset, index: usize, selected: bool, current: &aurastudio_brush::Brush) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let resp = resp.on_hover_text(format!("{}\n{}", p.name, engine_label(&p.engine)));
    if !ui.is_rect_visible(rect) { return resp; }
    let bg = if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE };
    ui.painter().rect_filled(rect, 4.0, bg);
    // The selected brush is shown with its current settings.
    let mut settings = if selected { current.clone() } else { p.settings.clone() };
    settings.erase = false;
    st.previews.show(ui, rect.shrink(1.0), Slot::List(index), p, &settings, &p.name);
    if selected {
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.5_f32, theme::ACCENT));
    }
    resp
}

pub fn engine_label(e: &Engine) -> &'static str {
    match e {
        Engine::Native => "AuraStudio engine",
        Engine::MyPaint(_) => "MyPaint brush",
    }
}
