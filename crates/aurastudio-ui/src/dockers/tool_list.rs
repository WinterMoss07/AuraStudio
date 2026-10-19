//! Tools docker: the tools of the group chosen in the toolbox, each as a
//! row with its stroke preview (stroke_preview.rs) and its name in the
//! corner.
//!
//! - Brush groups (Draw, Paint, Eraser): a row of icons, one per medium
//!   (pencil, ink...), then that medium's brushes. Search looks in every
//!   brush; picking one from another group switches to it.
//! - Geometry: the shape tools, as thin outlines.
//! - Fill, Picker, Selection: their tools by icon and name.
//!
//! Click selects; each brush keeps its own size / opacity / blend mode,
//! and the selected one's stroke follows its settings. Brushes drag and
//! drop to another place in the list, or onto another medium's icon.

use aurastudio_brushes::stroke_preview::Mode;
use aurastudio_brushes::{Engine, Library, Preset};
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::icons::Icon;
use crate::stroke_preview::{Slot, Stroke as StrokeSpec, StrokePreviews};
use crate::tools::{Group, Tool};
use crate::{theme, Actions, ImportBrushes, Model};

/// Height of a row (points).
const ROW: f32 = 46.0;

/// A brush row being dragged (egui drag and drop payload).
struct BrushDrag(usize);

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
        tools(ui, group, m);
    }
}

fn brushes(ui: &mut Ui, st: &mut ToolListState, group: Group, m: &mut Model, a: &mut Actions) {
    let presets = m.presets;
    let mediums = group.mediums();
    let selected_medium = presets.get(*m.preset).map(|p| p.medium.clone()).filter(|md| mediums.contains(&md.as_str()));
    let medium = st.medium.get(&group).cloned().or(selected_medium).unwrap_or_else(|| mediums[0].to_string());

    // One icon per medium (none for a single one).
    if mediums.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for &md in mediums {
                let on = md == medium && st.filter.trim().is_empty();
                let (rect, resp) = ui.allocate_exact_size(vec2(25.0, 26.0), Sense::click());
                let resp = resp.on_hover_text(md);
                // A brush dropped here moves to this medium, at the end.
                let drop_here = resp.dnd_hover_payload::<BrushDrag>().is_some();
                if let Some(d) = resp.dnd_release_payload::<BrushDrag>() {
                    a.move_brush = Some((d.0, md.to_string(), None));
                }
                let bg = if on || drop_here { Some(theme::ACCENT) } else if resp.hovered() { Some(theme::WIDGET_HOVER) } else { None };
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

    if let Some(d) = egui::DragAndDrop::payload::<BrushDrag>(ui.ctx()) {
        if let Some(p) = presets.get(d.0) { crate::widgets::drag_ghost(ui.ctx(), &p.name); }
    }
    let filter = st.filter.trim().to_lowercase();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let mut pick = |st: &mut ToolListState, ui: &mut Ui, i: usize| {
            let resp = brush_row(ui, st, &presets[i], i, i == *m.preset, m.brush);
            if resp.clicked() {
                // A brush of another group (search): go there.
                let g = Group::of_medium(&presets[i].medium);
                *m.tool = if g == Group::Eraser { Tool::Eraser } else { Tool::Brush };
                *m.preset = i;
            }
            if let Some(moved) = drop_on_row(ui, &resp, i, presets) { a.move_brush = Some(moved); }
        };
        if filter.is_empty() {
            ui.label(RichText::new(&medium).strong());
            for i in Library::in_medium(presets, &medium) { pick(st, ui, i); }
            return;
        }
        // Searching: every brush, by medium.
        let mut any = false;
        for g in [Group::Draw, Group::Paint, Group::Eraser] {
            for &md in g.mediums() {
                let hits: Vec<usize> = Library::in_medium(presets, md).into_iter()
                    .filter(|&i| presets[i].name.to_lowercase().contains(&filter)).collect();
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

/// A brush dragged over row `i`: a line where it would go (above or
/// below), and the move once released.
fn drop_on_row(ui: &Ui, resp: &egui::Response, i: usize, presets: &[Preset]) -> Option<(usize, String, Option<usize>)> {
    let dragged = resp.dnd_hover_payload::<BrushDrag>()?;
    if dragged.0 == i { return None; }
    let below = ui.ctx().pointer_interact_pos()?.y > resp.rect.center().y;
    let y = if below { resp.rect.bottom() + 1.5 } else { resp.rect.top() - 1.5 };
    ui.painter().hline(resp.rect.x_range(), y, Stroke::new(2.0_f32, theme::ACCENT));
    let moved = resp.dnd_release_payload::<BrushDrag>()?;
    let medium = &presets[i].medium;
    let before = if below {
        // The brush after this one in its medium (None: the end).
        let list = Library::in_medium(presets, medium);
        list.iter().position(|&k| k == i).and_then(|p| list.get(p + 1).copied())
    } else {
        Some(i)
    };
    Some((moved.0, medium.clone(), before))
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
        ui.painter().rect_filled(rect, 4.0, if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE });
        if t.is_shape() {
            // A fixed thin outline: the brush shows in Tool options.
            crate::tools::draw_shape(&ui.painter_at(rect), t, rect);
            ui.painter().text(rect.right_bottom() - vec2(6.0, 4.0), egui::Align2::RIGHT_BOTTOM, t.name(),
                egui::FontId::proportional(11.0), theme::TEXT_WEAK);
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
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click_and_drag());
    if resp.drag_started() { resp.dnd_set_drag_payload(BrushDrag(index)); }
    let resp = resp.on_hover_text(format!("{}\n{}  ·  {}", p.name, p.author, engine_label(&p.engine)));
    if !ui.is_rect_visible(rect) { return resp; }
    let bg = if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE };
    ui.painter().rect_filled(rect, 4.0, bg);
    // The selected brush is shown with its current settings.
    let settings = if selected { current } else { &p.settings };
    let spec = StrokeSpec { preset: p, settings, mode: Mode::List };
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
