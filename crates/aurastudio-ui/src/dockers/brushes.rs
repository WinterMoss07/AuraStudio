//! Brushes docker: the brush presets, grouped (AuraStudio, then the
//! MyPaint collections and imported packs), as a grid of previews.
//! Click selects; each brush keeps its own size / opacity / blend mode.

use std::collections::HashMap;

use aurastudio_brushes::{Engine, Preset};
use egui::{pos2, vec2, Color32, Rect, RichText, Sense, Stroke, Ui};

use crate::{theme, Actions, ImportBrushes, Model};

/// Tile size of a brush in the grid.
const TILE: f32 = 58.0;
/// Previews are decoded at this size (memory: ~10 KB each).
const PREVIEW_PX: u32 = 64;

#[derive(Default)]
pub struct BrushesState {
    filter:   String,
    /// Preview textures by preset index, decoded the first time shown.
    previews: HashMap<usize, Option<egui::TextureHandle>>,
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
                    let cols = ((ui.available_width() + 4.0) / (TILE + 4.0)).floor().max(1.0) as usize;
                    for row in members.chunks(cols) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for &i in row {
                                if brush_tile(ui, st, &presets[i], i, i == *m.preset).clicked() {
                                    *m.preset = i;
                                }
                            }
                        });
                    }
                });
        }
    });
}

fn brush_tile(ui: &mut Ui, st: &mut BrushesState, p: &Preset, index: usize, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(TILE, TILE + 14.0), Sense::click());
    let resp = resp.on_hover_text(format!("{}\n{}", p.name, engine_label(&p.engine)));
    if !ui.is_rect_visible(rect) { return resp; }
    let painter = ui.painter_at(rect);
    let img = Rect::from_min_size(rect.min, vec2(TILE, TILE));

    let bg = if selected { theme::ACCENT } else if resp.hovered() { theme::WIDGET } else { theme::SURFACE };
    painter.rect_filled(img, 5.0, bg);

    let tex = st.previews.entry(index).or_insert_with(|| {
        p.decode_preview(PREVIEW_PX).map(|(w, h, rgba)| {
            let image = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
            ui.ctx().load_texture(format!("brush-preview-{index}"), image, egui::TextureOptions::LINEAR)
        })
    });
    match tex {
        Some(tex) => {
            let r = img.shrink(3.0);
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        // Built-in brushes: a drawn dab (hard or soft).
        None => {
            let c = img.center();
            let soft = p.settings.hardness < 0.5;
            let rings = if soft { 8 } else { 1 };
            for k in 0..rings {
                let t = k as f32 / rings as f32;
                let r = 18.0 * (1.0 - t * 0.9);
                let a = if soft { (40.0 + 215.0 * t) as u8 } else { 230 };
                painter.circle_filled(c, r, Color32::from_white_alpha(a).gamma_multiply(if soft { 0.35 } else { 1.0 }));
            }
        }
    }
    if selected {
        painter.rect_stroke(img, 5.0, Stroke::new(1.5_f32, theme::ACCENT));
    }

    // Name under the preview, cut to fit.
    let mut name = p.name.clone();
    while name.chars().count() > 3 && ui.fonts(|f| f.layout_no_wrap(name.clone(), egui::FontId::proportional(10.0), Color32::WHITE).size().x) > TILE {
        name.pop();
        name.pop();
        name.push('…');
    }
    painter.text(pos2(img.center().x, img.bottom() + 7.0), egui::Align2::CENTER_CENTER, name,
        egui::FontId::proportional(10.0), if selected { theme::TEXT } else { theme::TEXT_WEAK });
    resp
}

pub fn engine_label(e: &Engine) -> &'static str {
    match e {
        Engine::Native => "AuraStudio engine",
        Engine::MyPaint(_) => "MyPaint brush",
    }
}
