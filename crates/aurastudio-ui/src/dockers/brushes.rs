//! Brushes docker: a row of icons, one per medium (pencil, ink...), then
//! the brushes of the chosen medium as a list of strokes painted by each
//! brush (brush_preview.rs), its name in the corner. Searching looks in
//! every medium. Click selects; each brush keeps its own size / opacity /
//! blend mode, and the selected one's stroke follows its settings.

use aurastudio_brushes::{Engine, Preset, MEDIUMS};
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::brush_preview::{PreviewCache, Slot};
use crate::icons::Icon;
use crate::{theme, Actions, ImportBrushes, Model};

/// Height of a brush's row (points).
const ROW: f32 = 46.0;

#[derive(Default)]
pub struct BrushesState {
    filter:   String,
    /// The medium shown (None: the selected brush's).
    medium:   Option<String>,
    /// Stroke previews, shared with the Tool options docker.
    pub previews: PreviewCache,
    /// Number of presets the previews were made for (an import changes
    /// the list: they are redone).
    count:    usize,
}

fn medium_icon(medium: &str) -> Icon {
    match medium {
        "Pencil" => Icon::Pencil,
        "Ink" => Icon::Ink,
        "Charcoal & pastel" => Icon::Charcoal,
        "Paint" => Icon::Paint,
        "Watercolor" => Icon::Watercolor,
        "Airbrush" => Icon::Airbrush,
        "Blend & smudge" => Icon::Blend,
        "Eraser" => Icon::Eraser,
        "Texture & FX" => Icon::Effects,
        _ => Icon::Misc,
    }
}

pub fn show(ui: &mut Ui, st: &mut BrushesState, m: &mut Model, a: &mut Actions) {
    let presets = m.presets;
    if presets.len() != st.count {
        st.count = presets.len();
        st.previews.clear_list();
    }
    let medium = st.medium.clone()
        .or_else(|| presets.get(*m.preset).map(|p| p.group.clone()))
        .unwrap_or_else(|| MEDIUMS[0].to_string());

    // One icon per medium.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for &md in MEDIUMS {
            let on = md == medium && st.filter.trim().is_empty();
            let (rect, resp) = ui.allocate_exact_size(vec2(25.0, 26.0), Sense::click());
            let resp = resp.on_hover_text(md);
            let bg = if on { Some(theme::ACCENT) } else if resp.hovered() { Some(theme::WIDGET_HOVER) } else { None };
            if let Some(bg) = bg { ui.painter().rect_filled(rect, 5.0, bg); }
            crate::icons::paint(ui, medium_icon(md), rect, 17.0, if on { theme::TEXT } else { theme::TEXT_WEAK });
            if resp.clicked() {
                st.medium = Some(md.to_string());
                st.filter.clear();
            }
        }
    });
    ui.add_space(3.0);
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
    ui.add_space(4.0);

    let filter = st.filter.trim().to_lowercase();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        if filter.is_empty() {
            ui.label(RichText::new(&medium).strong());
            for (i, p) in presets.iter().enumerate().filter(|(_, p)| p.group == medium) {
                if brush_row(ui, st, p, i, i == *m.preset, m.brush).clicked() { *m.preset = i; }
            }
            return;
        }
        // Searching: every medium, under its name.
        let mut any = false;
        for &md in MEDIUMS {
            let hits: Vec<usize> = presets.iter().enumerate()
                .filter(|(_, p)| p.group == md && p.name.to_lowercase().contains(&filter))
                .map(|(i, _)| i).collect();
            if hits.is_empty() { continue; }
            any = true;
            ui.label(RichText::new(md).strong());
            for i in hits {
                if brush_row(ui, st, &presets[i], i, i == *m.preset, m.brush).clicked() {
                    *m.preset = i;
                    st.medium = Some(md.to_string());
                }
            }
            ui.add_space(4.0);
        }
        if !any { ui.label(RichText::new("No brush found").color(theme::TEXT_WEAK)); }
    });
}

fn brush_row(ui: &mut Ui, st: &mut BrushesState, p: &Preset, index: usize, selected: bool, current: &aurastudio_brush::Brush) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let resp = resp.on_hover_text(format!("{}\n{}  ·  {}", p.name, p.author, engine_label(&p.engine)));
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
