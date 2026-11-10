//! Tool options docker: the stroke preview with the brush's name, then
//! the settings it shows (chosen with the eyes of the Advanced tool
//! options window, kept per brush), each with its pen inputs' button
//! (CSP's Tool property).

use egui::{vec2, RichText, Sense, Ui};

use crate::brush_settings::{by_id, row, shown};
use crate::widgets::{icon_button, Icon};
use crate::{theme, Model};

pub fn show(ui: &mut Ui, m: &mut Model, a: &mut crate::Actions) {
    if !m.tool.paints() {
        crate::tools::options(ui, m, a);
        return;
    }
    let Some(p) = m.presets.get(*m.preset) else { return };
    let name = if m.tool.is_shape() { format!("{} · {}", m.tool.name(), p.name) } else { p.name.clone() };
    // The banner: the stroke, the name, the way to every setting.
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, crate::stroke_preview::TOOL_OPTIONS.1 as f32), Sense::hover());
    crate::stroke_preview::current(ui.painter(), r, crate::stroke_preview::Slot::ToolOptions, &previewed(m), m.tool.preview_shape(), &name, a);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, Icon::FilterLayer, "Advanced tool options: every setting of the brush (F5)", true) { a.advanced = true; }
            ui.label(RichText::new("Advanced tool options").small().color(theme::TEXT_WEAK));
        });
    });
    let categories = crate::advanced::show_category(ui.ctx());
    let mut last = None;
    for id in shown(m.brush) {
        // (An eraser has no blend mode.)
        if id == "blend" && m.brush.erase { continue; }
        let Some(st) = by_id(id) else { continue };
        if categories && last != Some(st.cat) {
            last = Some(st.cat);
            ui.add_space(2.0);
            ui.label(RichText::new(st.cat.name()).small().color(theme::TEXT_WEAK));
        }
        row(ui, st, m.brush, w, None);
    }
    if m.tool.is_shape() { crate::tools::shape_options(ui, m); }
    crate::tools::stay_inside_option(ui, m);
}

/// The brush its preview shows: the Eraser tool erases with any brush.
pub(crate) fn previewed<'m>(m: &'m Model<'_>) -> std::borrow::Cow<'m, aurastudio_forge::Brush> {
    if *m.tool == crate::Tool::Eraser && !m.brush.erase {
        std::borrow::Cow::Owned(aurastudio_forge::Brush { erase: true, ..m.brush.clone() })
    } else {
        std::borrow::Cow::Borrowed(&*m.brush)
    }
}
