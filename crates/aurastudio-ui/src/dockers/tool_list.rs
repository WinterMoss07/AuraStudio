//! Tools docker: the tools of the group chosen in the toolbox, each as a
//! row with its stroke preview (stroke_preview.rs) and its name in the
//! corner.
//!
//! - Brush groups (Draw, Paint, Eraser): a row of icons, one per medium
//!   (pencil, ink...), then that medium's brushes. Search looks in every
//!   brush; picking one from another group switches to it.
//! - Geometry: the shape tools, drawn with the current brush (1 to 15 px thick).
//! - Fill, Picker, Selection: their tools by icon and name.
//!
//! Click selects; each brush keeps its own size / opacity / blend mode,
//! and the selected one's stroke follows its settings. Brushes drag and
//! drop to another place in the list, or onto another medium's icon;
//! medium icons drag and drop among themselves. The + after them makes a
//! medium of one's own (right click: rename, remove when empty).

use aurastudio_brushes::stroke_preview::{Mode, Shape};
use aurastudio_brushes::{mediums_of, Library, Preset};
use egui::{vec2, RichText, Sense, Stroke, Ui};

use crate::icons::Icon;
use crate::stroke_preview::{Slot, Stroke as StrokeSpec, StrokePreviews};
use crate::tools::{Group, Tool};
use crate::{theme, Actions, ImportBrushes, Model};

/// Height of a row (points).
const ROW: f32 = 46.0;

/// A brush row being dragged (egui drag and drop payload).
struct BrushDrag(usize);

/// A medium icon being dragged.
struct MediumDrag(String);

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
    /// A medium being renamed: (medium, name typed so far, focus asked).
    renaming: Option<(String, String, bool)>,
}

impl ToolListState {
    /// Show medium `name` and edit its name.
    pub fn rename(&mut self, name: &str) {
        self.renaming = Some((name.to_owned(), name.to_owned(), false));
    }
}

fn medium_icon(medium: &str) -> Icon {
    // (Mediums of one's own: a folder.)
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
        "Misc" => Icon::Misc,
        _ => Icon::Folder,
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
    let Some(kind) = group.kind() else { return };
    let mediums = mediums_of(m.mediums, kind);
    if mediums.is_empty() { return; }
    // A medium just made (or renamed): show it.
    if let Some((name, ..)) = &st.renaming {
        if mediums.contains(&name.as_str()) { st.medium.insert(group, name.clone()); }
    }
    let selected_medium = presets.get(*m.preset).map(|p| p.medium.clone()).filter(|md| mediums.contains(&md.as_str()));
    let medium = st.medium.get(&group).cloned().filter(|md| mediums.contains(&md.as_str()))
        .or(selected_medium).unwrap_or_else(|| mediums[0].to_string());

    medium_icons(ui, st, group, &mediums, &medium, m, a);
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

    if let Some(d) = egui::DragAndDrop::payload::<BrushDrag>(ui.ctx()) {
        if let Some(p) = presets.get(d.0) { crate::widgets::drag_ghost(ui.ctx(), &p.name); }
    }
    let filter = st.filter.trim().to_lowercase();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let mut pick = |st: &mut ToolListState, ui: &mut Ui, i: usize| {
            let resp = brush_row(ui, st, &presets[i], i, i == *m.preset, m.brush, m.prefs.brush_previews);
            if resp.clicked() {
                // A brush of another group (search): go there.
                let g = Group::of_kind(presets[i].kind);
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
            for md in mediums_of(m.mediums, g.kind().unwrap()) {
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

/// One icon per medium, then +. Click shows a medium; a brush dropped on
/// one moves there; icons drag and drop among themselves (a red line where
/// one would go); right click: rename / remove a medium of one's own.
fn medium_icons(ui: &mut Ui, st: &mut ToolListState, group: Group, mediums: &[&str], medium: &str, m: &Model, a: &mut Actions) {
    let size = vec2(25.0, 26.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for (k, &md) in mediums.iter().enumerate() {
            let on = md == medium && st.filter.trim().is_empty();
            let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
            if resp.drag_started() { resp.dnd_set_drag_payload(MediumDrag(md.to_string())); }
            let resp = resp.on_hover_text(md);
            let bg = if on { Some(theme::ACCENT) } else if resp.hovered() { Some(theme::WIDGET_HOVER) } else { None };
            if let Some(bg) = bg { ui.painter().rect_filled(rect, 5.0, bg); }
            crate::icons::paint(ui, medium_icon(md), rect, 17.0, if on { theme::TEXT } else { theme::TEXT_WEAK });
            // A brush dropped here moves to this medium, at the end.
            if resp.dnd_hover_payload::<BrushDrag>().is_some() {
                ui.painter().rect_stroke(rect.shrink(1.0), 5.0, Stroke::new(2.0_f32, theme::DROP));
            }
            if let Some(d) = resp.dnd_release_payload::<BrushDrag>() {
                a.move_brush = Some((d.0, md.to_string(), None));
            }
            // A medium dropped here goes before or after it.
            if let Some(d) = resp.dnd_hover_payload::<MediumDrag>().filter(|d| d.0 != md) {
                let after = ui.ctx().pointer_interact_pos().is_some_and(|p| p.x > rect.center().x);
                let x = if after { rect.right() + 1.0 } else { rect.left() - 1.0 };
                ui.painter().vline(x, rect.y_range(), Stroke::new(2.0_f32, theme::DROP));
                if resp.dnd_release_payload::<MediumDrag>().is_some() {
                    let before = if after { mediums.get(k + 1).map(|s| s.to_string()) } else { Some(md.to_string()) };
                    if before.as_deref() != Some(d.0.as_str()) { a.move_medium = Some((d.0.clone(), before)); }
                }
            }
            if resp.clicked() {
                st.medium.insert(group, md.to_string());
                st.filter.clear();
            }
            let custom = m.mediums.iter().any(|x| x.name == md && x.custom);
            if custom {
                resp.context_menu(|ui| {
                    if ui.button("Rename…").clicked() {
                        st.rename(md);
                        ui.close_menu();
                    }
                    let empty = !m.presets.iter().any(|p| p.medium == md);
                    if ui.add_enabled(empty, egui::Button::new("Remove")).on_disabled_hover_text("Move its brushes out first").clicked() {
                        a.remove_medium = Some(md.to_string());
                        ui.close_menu();
                    }
                });
            }
        }
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let resp = resp.on_hover_text("New medium");
        if resp.hovered() { ui.painter().rect_filled(rect, 5.0, theme::WIDGET_HOVER); }
        crate::icons::paint(ui, Icon::Plus, rect, 15.0, theme::TEXT_WEAK);
        if resp.clicked() { a.add_medium = group.kind(); }
    });
    if let Some(d) = egui::DragAndDrop::payload::<MediumDrag>(ui.ctx()) { crate::widgets::drag_ghost(ui.ctx(), &d.0); }

    // Naming a medium: a text field under the icons (Enter keeps, Escape
    // cancels).
    let mut done = None;
    if let Some((old, text, focused)) = &mut st.renaming {
        if mediums.contains(&old.as_str()) {
            ui.add_space(3.0);
            let resp = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width()).hint_text("Medium name"));
            if !*focused { resp.request_focus(); *focused = true; }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(None);
            } else if resp.lost_focus() {
                done = Some((text.trim() != old.as_str()).then(|| (old.clone(), text.trim().to_owned())));
            }
        }
    }
    if let Some(rename) = done {
        if let Some((_, new)) = &rename { st.medium.insert(group, new.clone()); }
        a.rename_medium = rename;
        st.renaming = None;
    }
}

/// A brush dragged over row `i`: a line where it would go (above or
/// below), and the move once released.
fn drop_on_row(ui: &Ui, resp: &egui::Response, i: usize, presets: &[Preset]) -> Option<(usize, String, Option<usize>)> {
    let dragged = resp.dnd_hover_payload::<BrushDrag>()?;
    if dragged.0 == i { return None; }
    let below = ui.ctx().pointer_interact_pos()?.y > resp.rect.center().y;
    let y = if below { resp.rect.bottom() + 1.5 } else { resp.rect.top() - 1.5 };
    ui.painter().hline(resp.rect.x_range(), y, Stroke::new(2.0_f32, theme::DROP));
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
fn tools(ui: &mut Ui, st: &mut ToolListState, group: Group, m: &mut Model) {
    ui.label(RichText::new(group.name()).strong());
    ui.add_space(2.0);
    ui.spacing_mut().item_spacing.y = 3.0;
    for &t in group.tools() {
        let selected = t == *m.tool;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
        let resp = resp.on_hover_text(t.hint());
        ui.painter().rect_filled(rect, 4.0, if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE });
        if t.is_shape() {
            if let Some(p) = m.presets.get(*m.preset) {
                let settings = aurastudio_brush::Brush { erase: false, ..m.brush.clone() };
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

fn brush_row(ui: &mut Ui, st: &mut ToolListState, p: &Preset, index: usize, selected: bool, current: &aurastudio_brush::Brush, strokes: bool) -> egui::Response {
    let height = if strokes { ROW } else { 26.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click_and_drag());
    if resp.drag_started() { resp.dnd_set_drag_payload(BrushDrag(index)); }
    let resp = resp.on_hover_text(format!("{}\n{}  ·  {}", p.name, p.author, p.origin.label()));
    if !ui.is_rect_visible(rect) { return resp; }
    let bg = if selected || resp.hovered() { theme::WIDGET } else { theme::SURFACE };
    ui.painter().rect_filled(rect, 4.0, bg);
    // The selected brush is shown with its current settings.
    let settings = if selected { current } else { &p.settings };
    if strokes {
        let spec = StrokeSpec { preset: p, settings, shape: Shape::S, mode: Mode::List };
        st.previews.show(ui, rect.shrink(1.0), Slot::Brush(index), spec, Some(&p.name));
    } else {
        // Preferences: names only.
        ui.painter().text(rect.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, &p.name, egui::FontId::proportional(13.0), theme::TEXT);
    }
    if selected {
        ui.painter().rect_stroke(rect, 4.0, Stroke::new(1.5_f32, theme::ACCENT));
    }
    resp
}
