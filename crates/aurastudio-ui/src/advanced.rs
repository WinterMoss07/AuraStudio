//! The Advanced tool options window (CSP's Sub Tool Detail): the stroke
//! preview on top, the categories on the left, the chosen one's settings
//! on the right with the About box of the row under the pointer, and the
//! buttons to go back to the brush's default or to make this its default.
//! It floats over the canvas (F5, the toolbar, Tool options, Window).
//! Also the Brushes window (F6): the brush list, out of the dockers.

use aurastudio_forge::Brush;
use egui::{vec2, Color32, RichText, ScrollArea, Sense, Ui};

use crate::brush_settings::{by_id, of, paint_preview, row, set_shown, shown, Category};
use crate::{theme, Actions, Model};

/// The preview's picture, in pixels (it fits the cache's first page).
pub const PREVIEW: (u32, u32) = (480, 96);

/// What the app is asked to do with the brush's default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushDefault {
    /// Back to the default.
    Reset,
    /// These settings become the default.
    Save,
}

/// The stroke preview: the brush it shows, being made or not, its picture.
#[derive(Default)]
pub struct Preview {
    /// The brush last asked for.
    of:      Option<Brush>,
    busy:    bool,
    pic:     Option<crate::atlas::Pic>,
    pending: Option<egui::ColorImage>,
}

impl Preview {
    /// Ask for a new picture when the brush changed (one at a time).
    pub fn ask(&mut self, brush: &Brush, a: &mut Actions) {
        if self.busy || self.of.as_ref() == Some(brush) { return; }
        self.of = Some(brush.clone());
        self.busy = true;
        a.preview = Some(brush.clone());
    }

    /// The picture made (straight RGBA).
    pub fn set(&mut self, w: u32, h: u32, rgba: &[u8]) {
        self.busy = false;
        if rgba.len() == (w * h * 4) as usize {
            self.pending = Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba));
        }
    }

    /// Into the cache (retried while it has no room).
    fn upload(&mut self, ctx: &egui::Context) {
        let Some(img) = self.pending.take() else { return };
        let [w, h] = img.size;
        match crate::atlas::ui(|a| a.alloc(ctx, w, h)) {
            Some(at) => { crate::atlas::ui(|a| a.write(at, img)); self.pic = Some(crate::atlas::Pic(at)); }
            None => self.pending = Some(img),
        }
    }

    pub fn region(&self) -> Option<crate::atlas::Region> { self.pic.as_ref().map(|p| p.0) }
}

pub struct AdvancedState {
    pub open:   bool,
    cat:        Category,
    /// The row under the pointer (its About box).
    hovered:    Option<&'static str>,
    /// The Brushes window (F6).
    pub brushes_open: bool,
    brushes:    crate::dockers::tool_list::ToolListState,
    pub preview: Preview,
}

impl Default for AdvancedState {
    fn default() -> Self {
        Self { open: false, cat: Category::BrushSize, hovered: None, brushes_open: false, brushes: Default::default(), preview: Default::default() }
    }
}

/// Tool options shows the categories' names over its rows.
pub fn show_category(ctx: &egui::Context) -> bool { ctx.data(|d| d.get_temp(egui::Id::unique("show_category"))).unwrap_or(false) }

fn set_show_category(ctx: &egui::Context, on: bool) { ctx.data_mut(|d| d.insert_temp(egui::Id::unique("show_category"), on)); }

/// F5 and F6, and both windows.
pub fn windows(ctx: &egui::Context, st: &mut AdvancedState, m: &mut Model, a: &mut Actions) {
    if !m.home {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5)) { st.open = !st.open; }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F6)) { st.brushes_open = !st.brushes_open; }
    }
    st.preview.upload(ctx);
    if m.home { return; }
    if st.open || m.tool.paints() { st.preview.ask(m.brush, a); }
    if st.open { advanced(ctx, st, m, a); }
    if st.brushes_open {
        let mut open = true;
        egui::Window::new("Brushes").open(&mut open).default_size(vec2(300.0, 460.0)).resizable(true).show(ctx, |ui| {
            crate::dockers::tool_list::show(ui, &mut st.brushes, m, a);
        });
        st.brushes_open = open;
    }
}

fn advanced(ctx: &egui::Context, st: &mut AdvancedState, m: &mut Model, a: &mut Actions) {
    let mut open = true;
    let name = m.presets.get(*m.preset).map(|p| p.name.clone()).unwrap_or_default();
    egui::Window::new("Advanced tool options").id(egui::Id::unique("advanced_tool_options")).open(&mut open)
        .default_size(vec2(640.0, 560.0)).min_width(480.0).min_height(320.0).resizable(true).show(ctx, |ui| {
        // The stroke preview.
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 84.0), Sense::hover());
        paint_preview(ui, r, &name, st.preview.region());
        ui.add_space(6.0);
        let bottom = 34.0;
        let h = (ui.available_height() - bottom).max(120.0);
        ui.horizontal_top(|ui| {
            // The categories.
            let left = 150.0;
            ui.allocate_ui_with_layout(vec2(left, h), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(left);
                ui.set_min_height(h);
                egui::Frame::NONE.fill(theme::DESK).inner_margin(4).show(ui, |ui| {
                    ui.set_width(left - 8.0);
                    ui.set_min_height(h - 40.0);
                    for c in Category::ALL {
                        ui.horizontal(|ui| {
                            if c.is_sub() { ui.add_space(14.0); }
                            let on = c.switch().map(|(get, _)| get(m.brush));
                            let text = RichText::new(c.name()).color(if on == Some(false) { theme::TEXT_WEAK } else { theme::TEXT });
                            if ui.add_sized(vec2(ui.available_width(), 18.0), egui::Button::selectable(st.cat == c, text)).clicked() { st.cat = c; }
                        });
                    }
                });
                ui.add_space(4.0);
                let mut sc = show_category(ui.ctx());
                if ui.checkbox(&mut sc, "Show category").on_hover_text("Tool options shows the categories' names").changed() { set_show_category(ui.ctx(), sc); }
            });
            ui.separator();
            // Its settings.
            ui.vertical(|ui| {
                let w = ui.available_width().max(200.0);
                ui.set_width(w);
                settings(ui, st, m, w, h);
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Reset all settings to default").on_hover_text("The brush as it was saved as its default (or before it was first changed)").clicked() { a.brush_default = Some(BrushDefault::Reset); }
            if ui.button("Save all settings as default").on_hover_text("These settings become what Reset goes back to").clicked() { a.brush_default = Some(BrushDefault::Save); }
        });
    });
    st.open = open;
}

/// The chosen category: its sentence and switch, its rows, the About box.
fn settings(ui: &mut Ui, st: &mut AdvancedState, m: &mut Model, w: f32, h: f32) {
    let cat = st.cat;
    ui.horizontal(|ui| {
        ui.label(RichText::new(cat.name()).strong());
        if let Some((get, set)) = cat.switch() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut on = get(m.brush);
                if ui.checkbox(&mut on, "On").changed() { set(m.brush, on); }
            });
        }
    });
    ui.label(RichText::new(cat.about()).small().color(theme::TEXT_WEAK));
    ui.add_space(4.0);
    let about = 64.0;
    let list = shown(m.brush);
    let mut hovered = None;
    let off = cat.switch().is_some_and(|(get, _)| !get(m.brush));
    ScrollArea::vertical().id_salt("advanced_rows").max_height((h - about - 50.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
        if off { ui.disable(); }
        let mut last = None;
        for s in of(cat) {
            // Brush shape lists its parts' names.
            if cat == Category::BrushShape && last != Some(s.cat) {
                last = Some(s.cat);
                ui.add_space(4.0);
                ui.label(RichText::new(s.cat.name()).small().color(theme::TEXT_WEAK));
            }
            let was = list.contains(&s.id);
            let mut on = was;
            if let Some(r) = row(ui, s, m.brush, w - 14.0, Some(&mut on)) {
                if r.hovered() { hovered = Some(s.id); }
            }
            if on != was { set_shown(m.brush, s.id, on); }
        }
    });
    if hovered.is_some() { st.hovered = hovered; }
    // About the setting.
    let help = st.hovered.and_then(by_id).filter(|s| of(cat).any(|o| o.id == s.id));
    egui::Frame::NONE.fill(theme::DESK).inner_margin(6).corner_radius(2).show(ui, |ui| {
        ui.set_width(w - 12.0);
        ui.set_min_height(about - 12.0);
        match help {
            Some(s) => {
                ui.label(RichText::new(format!("About {}", s.label)).small().strong());
                ui.label(RichText::new(s.help).small().color(theme::TEXT_WEAK));
            }
            None => { ui.label(RichText::new("Point at a setting to read what it does.").small().color(Color32::from_gray(0x90))); }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A brush is drawn once until it changes; the answer frees the next.
    #[test]
    fn the_preview_is_asked_once_per_brush() {
        let mut p = Preview::default();
        let mut a = Actions::default();
        let b = Brush::default();
        p.ask(&b, &mut a);
        assert!(a.preview.take().is_some());
        p.ask(&b, &mut a);
        assert!(a.preview.is_none());
        p.set(2, 1, &[0; 8]);
        p.ask(&b, &mut a);
        assert!(a.preview.is_none(), "same brush");
        let other = Brush { size: 99.0, ..Brush::default() };
        p.ask(&other, &mut a);
        assert!(a.preview.is_some());
    }
}
