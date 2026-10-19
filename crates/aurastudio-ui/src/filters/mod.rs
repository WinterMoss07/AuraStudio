//! Filter menu and dialogs. A dialog floats beside the canvas: the layer
//! before and after side by side in its own preview (zoom, pan, a line to
//! drag), the filter's controls with pictures of what it does, what it
//! will change. The active layer shows the result too (Preview on
//! canvas), Apply keeps it (undoable), Cancel or Escape puts the layer
//! back. The filters themselves: aurastudio_filters.

// Pixel code indexes channels side by side (p[c], table[c]...).
#![allow(clippy::needless_range_loop)]

mod draw;
mod editors;
mod source;
mod view;

use std::sync::Arc;

use aurastudio_filters::{Filter, Kind, GROUPS};
use egui::{RichText, Ui};

use crate::{theme, Actions, Model};
use source::Source;

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
    source:  Arc<Source>,
    view:    view::View,
    state:   editors::State,
    /// Frames shown (egui sizes a new window over its first frames).
    frames:  u32,
}

/// The Filter menu's entries (disabled while a dialog is open, or when
/// the active layer can't take one).
pub fn menu(ui: &mut Ui, open: &mut Option<FilterDialog>, m: &Model) {
    let why = if open.is_some() {
        Some("A filter dialog is open")
    } else if !m.layers.can_paint() {
        Some("The active layer is locked or a folder")
    } else if m.layers.filter_area(m.selection, 0).is_some_and(|(_, r)| r.2 <= r.0) {
        Some("Nothing is painted on the active layer")
    } else {
        None
    };
    if let Some(why) = why { ui.label(RichText::new(why).small().color(theme::TEXT_WEAK)); }
    for (k, (group, kinds)) in GROUPS.iter().enumerate() {
        if k > 0 { ui.separator(); }
        ui.label(RichText::new(*group).small().color(theme::TEXT_WEAK));
        for &kind in *kinds {
            if ui.add_enabled(why.is_none(), egui::Button::new(format!("{}…", kind.name()))).on_hover_text(kind.hint()).clicked() {
                if let Some(source) = Source::capture(m.layers, m.selection) {
                    let color = match kind { Kind::BlackWhite => Filter::new(kind).color, _ => m.brush.color };
                    *open = Some(FilterDialog {
                        filter: Filter { color, ..Filter::new(kind) },
                        preview: true,
                        shown: None,
                        source: Arc::new(source),
                        view: view::View::new(ui.ctx()),
                        state: editors::State::default(),
                        frames: 0,
                    });
                }
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
    if d.frames < 3 {
        d.frames += 1;
        ctx.request_repaint();
    }
    egui::Window::new(kind.name())
        .id(egui::Id::new("filter_dialog"))
        .collapsible(false)
        .resizable(false)
        // (Placed by its left top corner: with a right top pivot, egui
        // moved the window at the first click in it.)
        .default_pos(ctx.screen_rect().right_top() + egui::vec2(-24.0 - draw::WIDTH - 30.0, 64.0))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(14.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(draw::WIDTH);
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.label(RichText::new(kind.hint()).color(theme::TEXT_WEAK));
            ui.add_space(2.0);
            let src = Arc::clone(&d.source);
            // On a short screen (a laptop at 200 %) the preview shrinks and
            // the controls scroll.
            let screen = ctx.screen_rect().height();
            let view_h = (screen - 680.0).clamp(130.0, 230.0);
            d.view.show(ui, &src, &d.filter, view_h);
            ui.separator();
            let controls_h = (screen - view_h - 330.0).max(160.0);
            let info = egui::ScrollArea::vertical().max_height(controls_h).auto_shrink([false, true]).show(ui, |ui| {
                ui.set_width(draw::WIDTH);
                editors::show(ui, &mut d.filter, &src, &mut d.state)
            }).inner;
            ui.separator();
            for line in info { draw::info(ui, &line); }
            let (w, h) = (src.area.2 - src.area.0, src.area.3 - src.area.1);
            let px = src.painted();
            let count = if px >= 1_000_000 { format!("{:.1} M", px as f64 / 1e6) } else { format!("{px}") };
            draw::info(ui, &format!("{} {w} × {h} px, {count} painted pixels", if src.selection { "In the selection:" } else { "On the layer:" }));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.preview, "Preview on canvas").on_hover_text("The layer shows the result while the dialog is open");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new("Apply").color(theme::TEXT)).fill(theme::ACCENT).min_size(egui::vec2(80.0, 26.0));
                    if ui.add(b).on_hover_text("Enter").clicked() { apply = true; }
                    if ui.button("Cancel").on_hover_text("Escape").clicked() { open = false; }
                    if ui.button("Reset").on_hover_text("Every value back to its default").clicked() {
                        d.filter.values = kind.defaults();
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_layers::{LayerStack, Selection};

    fn layers() -> LayerStack {
        let mut l = LayerStack::new_document(300, 200);
        let id = l.add_layer("ink");
        l.set_active(id);
        let px: Vec<u8> = (0..200 * 120).flat_map(|i| { let (x, y) = (i % 200, i / 200); [(x + y) as u8, (x * 2) as u8, (y * 3) as u8, if x % 7 == 0 { 0 } else { 255 }] }).collect();
        assert!(l.set_layer_rect(id, (40, 30, 240, 150), &px));
        l
    }

    #[test]
    fn source_follows_the_paint_threads_area() {
        let l = layers();
        let s = Source::capture(&l, None).unwrap();
        // The painted area, the margin around it inside the canvas.
        assert_eq!((s.full.w, s.full.h), (300, 200));
        // (Whole tiles of 64 px.)
        assert_eq!(s.area, (0, 0, 256, 192));
        assert!(s.transparent && s.proxy.is_none());
        let sel = Selection::rect(300, 200, (50, 40, 60, 50)).unwrap();
        let s = Source::capture(&l, Some(&sel)).unwrap();
        assert_eq!(s.hist.count, 100 - 10, "selected painted pixels: {}", s.hist.count);
        // Outside the selection the preview keeps the pixels.
        let f = Filter::new(Kind::Invert);
        let crop = source::Crop::new(&s.full, 1, (0, 0, s.full.w, s.full.h), 0);
        let out = source::filter_crop(&f, &s.full, &crop);
        let at = |v: &[u8], x: usize, y: usize| v[(y * s.full.w + x) * 4..][..4].to_vec();
        assert_eq!(at(&out, 100, 100), at(&s.full.px, 100, 100));
        assert_ne!(at(&out, 51, 41), at(&s.full.px, 51, 41));
        // Nothing painted: no source.
        let mut e = LayerStack::new_document(64, 64);
        let id = e.add_layer("empty");
        e.set_active(id);
        assert!(Source::capture(&e, None).is_none());
    }

    #[test]
    fn every_dialog_runs() {
        let l = layers();
        let ctx = egui::Context::default();
        for (_, kinds) in GROUPS {
            for &kind in *kinds {
                let src = Arc::new(Source::capture(&l, None).unwrap());
                let mut d = FilterDialog { filter: Filter::new(kind), preview: true, shown: None, source: src, view: view::View::new(&ctx), state: editors::State::default(), frames: 0 };
                let mut sent = Vec::new();
                for frame in 0..4 {
                    let mut a = Actions::default();
                    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0 - frame as f32 * 100.0))), ..Default::default() };
                    let _ = ctx.run(input, |ctx| { assert!(dialog(ctx, &mut d, &mut a), "{kind:?}"); });
                    sent.extend(a.filter);
                }
                assert_eq!(sent, vec![FilterAction::Preview(Some(Filter::new(kind)))], "{kind:?}: one preview, then nothing until a change");
            }
        }
    }
}
