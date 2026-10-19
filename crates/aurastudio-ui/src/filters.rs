//! Filter menu and dialog: a floating window (the canvas stays visible)
//! with the filter's values; the active layer shows the result while it is
//! open (Preview), Apply keeps it (undoable), Cancel or Escape puts the
//! layer back. The filters themselves: aurastudio_filters.

use aurastudio_filters::{Filter, Kind, Unit, GROUPS};
use egui::{RichText, Ui};

use crate::widgets::{fill_slider, Fill};
use crate::{theme, Actions, Model};

/// What the app sends to the paint thread for the open filter dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterAction {
    /// Show this filter on the active layer (None: the layer as it was).
    Preview(Option<Filter>),
    Apply(Filter),
    Cancel,
}

/// The open filter dialog.
pub struct FilterDialog {
    filter:  Filter,
    preview: bool,
    /// What the layer shows now (None: nothing sent yet).
    shown:   Option<Option<Filter>>,
}

impl FilterDialog {
    fn new(kind: Kind, color: [u8; 3]) -> Self {
        Self { filter: Filter { color, ..Filter::new(kind) }, preview: true, shown: None }
    }
}

/// The Filter menu's entries (disabled while a dialog is open).
pub fn menu(ui: &mut Ui, open: &mut Option<FilterDialog>, m: &Model) {
    let busy = open.is_some();
    for (k, (group, kinds)) in GROUPS.iter().enumerate() {
        if k > 0 { ui.separator(); }
        ui.label(RichText::new(*group).small().color(theme::TEXT_WEAK));
        for &kind in *kinds {
            if ui.add_enabled(!busy, egui::Button::new(format!("{}…", kind.name()))).on_hover_text(kind.hint()).clicked() {
                *open = Some(FilterDialog::new(kind, m.brush.color));
                ui.close_menu();
            }
        }
    }
}

/// The dialog, if open. Returns false once closed.
pub fn dialog(ctx: &egui::Context, d: &mut FilterDialog, a: &mut Actions) -> bool {
    let mut open = true;
    let mut apply = false;
    let kind = d.filter.kind;
    egui::Window::new(kind.name())
        .id(egui::Id::new("filter_dialog"))
        .collapsible(false)
        .resizable(false)
        .default_pos(ctx.screen_rect().center() - egui::vec2(160.0, 120.0))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(14.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(300.0);
            ui.label(RichText::new(kind.hint()).small().color(theme::TEXT_WEAK));
            ui.add_space(8.0);
            for (i, p) in kind.params().iter().enumerate() {
                let (scale, fmt): (f32, Box<dyn Fn(f32) -> String>) = match p.unit {
                    Unit::Percent => (100.0, Box::new(|v| format!("{v:.0} %"))),
                    Unit::Pixels => (1.0, Box::new(|v| format!("{v:.1} px"))),
                    Unit::Degrees => (1.0, Box::new(|v| format!("{v:.0}°"))),
                    Unit::Number(n) => (1.0, Box::new(move |v| format!("{v:.n$}"))),
                };
                let spec = Fill { label: p.name, range: p.min * scale..=p.max * scale, mid: None, format: &*fmt };
                if let Some(v) = fill_slider(ui, &format!("filter_{i}"), spec, d.filter.values[i] * scale, 300.0) {
                    d.filter.values[i] = v / scale;
                }
                ui.add_space(2.0);
            }
            ui.add_space(6.0);
            ui.checkbox(&mut d.preview, "Preview");
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.button("Reset").clicked() { d.filter.values = kind.defaults(); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new("Apply").color(theme::TEXT)).fill(theme::ACCENT).min_size(egui::vec2(88.0, 26.0));
                    if ui.add(b).clicked() { apply = true; }
                    if ui.button("Cancel").clicked() { open = false; }
                });
            });
        });
    // (Enter in a value being typed confirms the value, not the dialog.)
    if ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.key_pressed(egui::Key::Enter)) { apply = true; }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { open = false; }

    if apply {
        a.filter = Some(FilterAction::Apply(d.filter.clone()));
        return false;
    }
    if !open {
        a.filter = Some(FilterAction::Cancel);
        return false;
    }
    let wanted = d.preview.then(|| d.filter.clone());
    if d.shown.as_ref() != Some(&wanted) {
        d.shown = Some(wanted.clone());
        a.filter = Some(FilterAction::Preview(wanted));
    }
    true
}
