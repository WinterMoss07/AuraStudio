//! Filter menu and dialogs: before / after preview, controls, Apply
//! (undoable) or Cancel.

// Pixel code indexes channels side by side (p[c], table[c]...).
#![allow(clippy::needless_range_loop)]

mod draw;
mod editors;
mod source;
mod view;

use aurastudio_document::Edit;
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

/// What a dialog works on.
enum Target {
    /// The active layer's pixels (the paint thread's filter session).
    Layer,
    /// A filter layer's filter: changed in place, put back on Cancel (or
    /// the layer removed, if it was just made).
    FilterLayer { id: u64, before: Filter, created: bool },
}

/// What a filter layer's dialog asks of its layer (lib.rs does it).
pub enum LayerEdit {
    /// Show this filter on it.
    Set(Filter),
    /// Done: keep it.
    Keep(Filter),
    /// Cancelled: back to this one.
    Restore(Filter),
    /// Cancelled when just made: remove the layer.
    Remove,
}

/// The open filter dialog.
pub struct FilterDialog {
    target:  Target,
    /// For a filter layer: what to do to it this frame.
    pub(crate) edit: Option<(u64, LayerEdit)>,
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
    } else if !m.can_paint() {
        Some("The active layer is locked or a folder")
    } else if m.doc.filter_area(0).is_some_and(|(_, r)| r.is_empty()) {
        Some("Nothing is painted on the active layer")
    } else {
        None
    };
    if let Some(why) = why { ui.label(RichText::new(why).small().color(theme::TEXT_WEAK)); }
    for (k, (group, kinds)) in GROUPS.iter().enumerate() {
        if k > 0 { ui.separator(); }
        ui.label(RichText::new(*group).small().color(theme::TEXT_WEAK));
        for &kind in *kinds {
            if ui.add_enabled(why.is_none(), egui::Button::new(format!("{}…", kind.name()))).clicked() {
                *open = new_dialog(ui.ctx(), kind, m);
                ui.close();
            }
        }
    }
}

/// A new filter of `kind`, colored with the brush's color (Black & white
/// keeps its own tint).
pub(crate) fn fresh(kind: Kind, brush_color: [u8; 3]) -> Filter {
    let f = Filter::new(kind);
    if kind == Kind::BlackWhite { f } else { Filter { color: brush_color, ..f } }
}

/// A dialog for `kind` on the active layer (None: nothing to filter
/// there).
pub fn new_dialog(ctx: &egui::Context, kind: Kind, m: &Model) -> Option<FilterDialog> {
    let source = Source::capture(m.doc)?;
    Some(FilterDialog {
        target: Target::Layer,
        edit: None,
        filter: fresh(kind, m.brush.color),
        preview: true,
        shown: None,
        source: Arc::new(source),
        view: view::View::new(ctx),
        state: editors::State::default(),
        frames: 0,
    })
}

/// A dialog for filter layer `id` (`created`: just made; Cancel removes
/// it). None if it isn't a filter layer.
pub fn layer_dialog(ctx: &egui::Context, id: u64, created: bool, m: &Model) -> Option<FilterDialog> {
    let Some(aurastudio_layers::Node::Filter(fl)) = m.layers().node(id) else { return None };
    let source = Source::below(m.doc, id)?;
    Some(FilterDialog {
        target: Target::FilterLayer { id, before: fl.filter.clone(), created },
        edit: None,
        filter: fl.filter.clone(),
        preview: true,
        // The layer shows its filter already.
        shown: Some(Some(fl.filter.clone())),
        source: Arc::new(source),
        view: view::View::new(ctx),
        state: editors::State::default(),
        frames: 0,
    })
}

impl FilterDialog {
    /// Closed from outside (a document opened...): for the active layer,
    /// its session is cancelled (the app sends it); a filter layer keeps
    /// what it shows.
    pub fn targets_layer(&self) -> bool { matches!(self.target, Target::Layer) }
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
    let title = match d.target { Target::Layer => kind.name().to_string(), Target::FilterLayer { .. } => format!("{} (filter layer)", kind.name()) };
    egui::Window::new(title)
        .id(egui::Id::unique("filter_dialog"))
        .collapsible(false)
        .resizable(false)
        // (Placed by its left top corner: with a right top pivot, egui
        // moved the window at the first click in it.)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-24.0 - draw::WIDTH - 30.0, 64.0))
        .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin::same(14)).corner_radius(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(draw::WIDTH);
            ui.spacing_mut().item_spacing.y = 5.0;
            ui.label(RichText::new(kind.hint()).color(theme::TEXT_WEAK));
            ui.add_space(2.0);
            let src = Arc::clone(&d.source);
            // On a short screen (a laptop at 200 %) the preview shrinks and
            // the controls scroll.
            let screen = ctx.content_rect().height();
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
            match d.target {
                Target::Layer => draw::info(ui, &format!("{} {w} × {h} px, {count} painted pixels", if src.selection { "In the selection:" } else { "On the layer:" })),
                Target::FilterLayer { .. } => draw::info(ui, "Filter layer: double-click it to edit it again"),
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.preview, "Preview on canvas");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new("Apply").color(theme::TEXT)).fill(theme::ACCENT).min_size(egui::vec2(80.0, 26.0));
                    if ui.add(b).on_hover_text("Enter").clicked() { apply = true; }
                    if ui.button("Cancel").on_hover_text("Escape").clicked() { open = false; }
                    if ui.button("Reset").clicked() {
                        d.filter.values = kind.defaults();
                    }
                });
            });
        });
    // (Enter in a value being typed confirms the value, not the dialog.)
    if ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.key_pressed(egui::Key::Enter)) { apply = true; }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { open = false; }

    if let Target::FilterLayer { id, before, created } = &d.target {
        let id = *id;
        if apply {
            d.edit = Some((id, LayerEdit::Keep(d.filter.clone())));
            return false;
        }
        if !open {
            d.edit = Some((id, if *created { LayerEdit::Remove } else { LayerEdit::Restore(before.clone()) }));
            return false;
        }
        // "Preview on canvas" off: the layer shows its filter as it was.
        let wanted = if d.preview { d.filter.clone() } else { before.clone() };
        if d.shown.as_ref().and_then(Option::as_ref) != Some(&wanted) {
            d.shown = Some(Some(wanted.clone()));
            d.edit = Some((id, LayerEdit::Set(wanted)));
        }
        return true;
    }
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

/// Do a filter layer dialog's edit on the layer.
pub fn apply_layer_edit(id: u64, edit: LayerEdit, m: &mut Model) {
    if !matches!(m.layers().node(id), Some(aurastudio_layers::Node::Filter(_))) { return; }
    match edit {
        LayerEdit::Set(f) | LayerEdit::Restore(f) => { m.apply(Edit::SetFilter(id, f)); }
        LayerEdit::Keep(f) => { m.apply(Edit::SetFilter(id, f)); }
        LayerEdit::Remove => {
            m.apply(Edit::SetActive(id));
            m.apply(Edit::RemoveActive);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurastudio_document::Document;

    fn layers() -> Document {
        let mut d = Document::new(aurastudio_document::DocId(1), "t", 300, 200, aurastudio_core::DocType::Illustration);
        d.apply(aurastudio_document::Edit::AddLayer("ink".into()));
        let px: Vec<[u8; 4]> = (0..200 * 120).map(|i| { let (x, y) = (i % 200, i / 200); [(x + y) as u8, (x * 2) as u8, (y * 3) as u8, if x % 7 == 0 { 0 } else { 255 }] }).collect();
        d.stroke("t", |s| {
            use aurastudio_core::PaintSurface;
            let r = aurastudio_core::Rect::new(40, 30, 240, 150);
            s.prepare(r);
            for y in r.y0..r.y1 {
                let mut x = r.x0;
                while x < r.x1 {
                    let sp = s.span_mut(x, y, (r.x1 - x) as usize);
                    for i in 0..sp.len() { sp.pixels[i] = px[((y - 30) * 200 + x - 40) as usize + i]; sp.mask[i] = 255; }
                    x += sp.len() as i32;
                }
            }
        });
        d
    }

    #[test]
    fn source_follows_the_paint_threads_area() {
        let mut l = layers();
        let s = Source::capture(&l).unwrap();
        // What shows (the first column is transparent), and the margin
        // around it (the canvas is unbounded).
        assert_eq!((s.full.w, s.full.h), (199 + 2 * 512, 120 + 2 * 512));
        assert_eq!(s.area, (512, 512, 711, 632));
        assert!(s.transparent && s.k_for(1.0, 1.0) == 1);
        l.apply(aurastudio_document::Edit::Select(aurastudio_document::SelectOp::Shape { shape: aurastudio_document::Shape::Rect(aurastudio_core::Rect::new(50, 40, 60, 50)), mode: aurastudio_document::SelectMode::Replace, aa: false, feather: 0.0 }));
        let s = Source::capture(&l).unwrap();
        assert_eq!(s.hist.count, 100 - 10, "selected painted pixels: {}", s.hist.count);
        // Outside the selection the preview keeps the pixels.
        let f = Filter::new(Kind::Invert);
        let crop = source::Crop::new(&s.full, 1, (0, 0, s.full.w, s.full.h), 0);
        let out = source::filter_crop(&f, &s.full, &crop);
        let at = |v: &[u8], x: usize, y: usize| v[(y * s.full.w + x) * 4..][..4].to_vec();
        let o = |x: usize, y: usize| (x + 512 - 50, y + 512 - 40);
        let (ax, ay) = o(100, 100);
        assert_eq!(at(&out, ax, ay), at(&s.full.px, ax, ay));
        let (bx, by) = o(51, 41);
        assert_ne!(at(&out, bx, by), at(&s.full.px, bx, by));
        // Nothing painted: no source.
        let mut e = Document::new(aurastudio_document::DocId(2), "e", 64, 64, aurastudio_core::DocType::Illustration);
        e.apply(aurastudio_document::Edit::AddLayer("empty".into()));
        assert!(Source::capture(&e).is_none());
    }

    #[test]
    fn every_dialog_runs() {
        let l = layers();
        let ctx = egui::Context::default();
        for (_, kinds) in GROUPS {
            for &kind in *kinds {
                let src = Arc::new(Source::capture(&l).unwrap());
                let mut d = FilterDialog { target: Target::Layer, edit: None, filter: Filter::new(kind), preview: true, shown: None, source: src, view: view::View::new(&ctx), state: editors::State::default(), frames: 0 };
                let mut sent = Vec::new();
                for frame in 0..4 {
                    let mut a = Actions::default();
                    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0 - frame as f32 * 100.0))), ..Default::default() };
                    crate::test_pass(&ctx, input, |root| { assert!(dialog(root.ctx(), &mut d, &mut a), "{kind:?}"); });
                    sent.extend(a.filter);
                }
                assert_eq!(sent, vec![FilterAction::Preview(Some(Filter::new(kind)))], "{kind:?}: one preview, then nothing until a change");
            }
        }
    }
}
