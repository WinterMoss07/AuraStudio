//! Tools docker: the tools of the group chosen in the toolbox, each as a
//! row with its stroke preview (stroke_preview.rs) and its name in the
//! corner.
//!
//! - Brush groups (Draw, Paint, Eraser): a row of icons, one per medium
//!   (pencil, ink...), then that medium's brushes. Search looks in every
//!   brush; picking one from another group switches to it.
//! - Geometry: the shape tools, drawn with the current brush.
//! - Fill, Picker, Selection: their tools by icon and name.
//!
//! Click selects; each brush keeps its own size / opacity / blend mode,
//! and the selected one's stroke follows its settings.

use aurastudio_brushes::stroke_preview::{Mode, Shape};
use aurastudio_brushes::{Engine, Preset};
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::icons::Icon;
use crate::stroke_preview::{Slot, Stroke as StrokeSpec, StrokePreviews};
use crate::tools::{Group, Tool};
use crate::{theme, Actions, ImportBrushes, Model};

/// Height of a row (points).
const ROW: f32 = 46.0;

#[derive(Default)]
pub struct ToolListState {
    filter:   String,
    /// The medium shown per brush group (None: the selected brush's).
    medium:   std::collections::HashMap<Group, String>,
    /// Stroke previews, shared with the Tool options docker.
    pub previews: StrokePreviews,
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

pub fn show(ui: &mut Ui, st: &mut ToolListState, m: &mut Model, a: &mut Actions) {
    if m.presets.len() != st.count {
        st.count = m.presets.len();
        st.previews.clear_brushes();
    }
    let group = Group::current(*m.tool, *m.preset, m.presets);
    if group.is_brushes() {
        brushes(ui, st, group, m, a);
    } else {
        tools(ui, st, group, m);
    }
}

fn brushes(ui: &mut Ui, st: &mut ToolListState, group: Group, m: &mut Model, a: &mut Actions) {
    let presets = m.presets;
    let mediums = group.mediums();
    let selected_medium = presets.get(*m.preset).map(|p| p.group.clone()).filter(|md| mediums.contains(&md.as_str()));
    let medium = st.medium.get(&group).cloned().or(selected_medium).unwrap_or_else(|| mediums[0].to_string());

    // One icon per medium (none for a single one).
    if mediums.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for &md in mediums {
                let on = md == medium && st.filter.trim().is_empty();
                let (rect, resp) = ui.allocate_exact_size(vec2(25.0, 26.0), Sense::click());
                let resp = resp.on_hover_text(md);
                let bg = if on { Some(theme::ACCENT) } else if resp.hovered() { Some(theme::WIDGET_HOVER) } else { None };
                if let Some(bg) = bg { ui.painter().rect_filled(rect, 5.0, bg); }
                crate::icons::paint(ui, medium_icon(md), rect, 17.0, if on { theme::TEXT } else { theme::TEXT_WEAK });
                if resp.clicked() {
                    st.medium.insert(group, md.to_string());
                    st.filter.clear();
                }
            }
        });
        ui.add_space(3.0);
    }
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
        let mut pick = |st: &mut ToolListState, ui: &mut Ui, i: usize| {
            if brush_row(ui, st, &presets[i], i, i == *m.preset, m.brush).clicked() {
                // A brush of another group (search): go there.
                let g = Group::of_medium(&presets[i].group);
                *m.tool = if g == Group::Eraser { Tool::Eraser } else { Tool::Brush };
                *m.preset = i;
            }
        };
        if filter.is_empty() {
            ui.label(RichText::new(&medium).strong());
            for i in (0..presets.len()).filter(|&i| presets[i].group == medium) { pick(st, ui, i); }
            return;
        }
        // Searching: every brush, by medium.
        let mut any = false;
        for g in [Group::Draw, Group::Paint, Group::Eraser] {
            for &md in g.mediums() {
                let hits: Vec<usize> = (0..presets.len())
                    .filter(|&i| presets[i].group == md && presets[i].name.to_lowercase().contains(&filter)).collect();
                if hits.is_empty() { continue; }
                any = true;
                ui.label(RichText::new(md).strong());
                for i in hits { pick(st, ui, i); }
                ui.add_space(4.0);
            }
        }
        if !any { ui.label(RichText::new("No brush found").color(theme::TEXT_WEAK)); }
    });
}

/// The tools of a group that isn't brushes.
fn tools(ui: &mut Ui, st: &mut ToolListState, group: Group, m: &mut Model) {
    ui.label(RichText::new(group.name()).strong());
    ui.add_space(2.0);
    ui.spacing_mut().item_spacing.y = 3.0;
    for &t in group.tools() {
        let selected = t == *m.tool;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        let resp = resp.on_hover_text(t.name());
        ui.painter().rect_filled(rect, 4.0, if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE });
        if t.is_shape() {
            // Drawn with the current brush.
            if let Some(p) = m.presets.get(*m.preset) {
                let mut settings = m.brush.clone();
                settings.erase = false;
                let spec = StrokeSpec { preset: p, settings: &settings, shape: t.stroke_shape(), mode: Mode::List };
                st.previews.show(ui, rect.shrink(1.0), Slot::Shape(t.stroke_shape()), spec, Some(t.name()));
            }
        } else {
            crate::icons::paint(ui, t.icon(), egui::Rect::from_min_size(rect.min, vec2(ROW, ROW)), 22.0, theme::TEXT);
            ui.painter().text(rect.left_center() + vec2(ROW + 4.0, 0.0), egui::Align2::LEFT_CENTER, t.name(),
                egui::FontId::proportional(13.0), theme::TEXT);
        }
        if selected { ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.5_f32, theme::ACCENT)); }
        if resp.clicked() { *m.tool = t; }
    }
}

fn brush_row(ui: &mut Ui, st: &mut ToolListState, p: &Preset, index: usize, selected: bool, current: &aurastudio_brush::Brush) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let resp = resp.on_hover_text(format!("{}\n{}  ·  {}", p.name, p.author, engine_label(&p.engine)));
    if !ui.is_rect_visible(rect) { return resp; }
    let bg = if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE };
    ui.painter().rect_filled(rect, 4.0, bg);
    // The selected brush is shown with its current settings.
    let settings = if selected { current } else { &p.settings };
    let spec = StrokeSpec { preset: p, settings, shape: Shape::S, mode: Mode::List };
    st.previews.show(ui, rect.shrink(1.0), Slot::Brush(index), spec, Some(&p.name));
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
