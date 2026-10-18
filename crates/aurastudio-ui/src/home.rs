//! Start screen (in place of the canvas while no document is open, like
//! Procreate / PhotoCraft: the docks stay) and the New document dialog.

use std::path::PathBuf;

use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui};

use crate::{theme, Actions};

/// A recently used file, as the start screen shows it.
pub struct RecentFile {
    pub path:   PathBuf,
    pub name:   String,
    /// "PNG · 2 hours ago"...
    pub detail: String,
    thumb:      Option<egui::TextureHandle>,
    /// Thumbnail decoded but not uploaded yet (textures need the context).
    pending:    Option<egui::ColorImage>,
}

impl RecentFile {
    pub fn new(path: PathBuf, name: String, detail: String) -> Self {
        Self { path, name, detail, thumb: None, pending: None }
    }
}

/// What the New dialog asks the app to create.
#[derive(Clone, Debug, PartialEq)]
pub struct NewDoc {
    pub name:   String,
    pub width:  u32,
    pub height: u32,
    pub dpi:    f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit { Px, Mm, Cm, In }

impl Unit {
    const ALL: [Unit; 4] = [Unit::Px, Unit::Mm, Unit::Cm, Unit::In];
    fn name(self) -> &'static str {
        match self { Unit::Px => "px", Unit::Mm => "mm", Unit::Cm => "cm", Unit::In => "in" }
    }
    /// Size in this unit -> pixels at `dpi`.
    fn to_px(self, v: f32, dpi: f32) -> u32 {
        let inches = match self { Unit::Px => return v.round().max(1.0) as u32, Unit::Mm => v / 25.4, Unit::Cm => v / 2.54, Unit::In => v };
        (inches * dpi).round().max(1.0) as u32
    }
    fn of_px(self, px: u32, dpi: f32) -> f32 {
        let inches = px as f32 / dpi;
        match self { Unit::Px => px as f32, Unit::Mm => inches * 25.4, Unit::Cm => inches * 2.54, Unit::In => inches }
    }
}

struct Preset { group: &'static str, name: &'static str, w: f32, h: f32, unit: Unit, dpi: f32 }

const PRESETS: &[Preset] = &[
    Preset { group: "Paper", name: "A5", w: 148.0, h: 210.0, unit: Unit::Mm, dpi: 300.0 },
    Preset { group: "Paper", name: "A4", w: 210.0, h: 297.0, unit: Unit::Mm, dpi: 300.0 },
    Preset { group: "Paper", name: "A3", w: 297.0, h: 420.0, unit: Unit::Mm, dpi: 300.0 },
    Preset { group: "Paper", name: "US Letter", w: 8.5, h: 11.0, unit: Unit::In, dpi: 300.0 },
    // Black and white manga manuscripts are drawn at 600 dpi (CSP presets).
    Preset { group: "Manga", name: "B4 manuscript", w: 257.0, h: 364.0, unit: Unit::Mm, dpi: 600.0 },
    Preset { group: "Manga", name: "B5", w: 182.0, h: 257.0, unit: Unit::Mm, dpi: 600.0 },
    Preset { group: "Manga", name: "A4", w: 210.0, h: 297.0, unit: Unit::Mm, dpi: 600.0 },
    Preset { group: "Comics", name: "US comic, bleed", w: 6.875, h: 10.4375, unit: Unit::In, dpi: 600.0 },
    Preset { group: "Comics", name: "US art board", w: 11.0, h: 17.0, unit: Unit::In, dpi: 400.0 },
    Preset { group: "Comics", name: "BD album page", w: 240.0, h: 320.0, unit: Unit::Mm, dpi: 600.0 },
    Preset { group: "Screen", name: "Square", w: 2048.0, h: 2048.0, unit: Unit::Px, dpi: 72.0 },
    Preset { group: "Screen", name: "Full HD", w: 1920.0, h: 1080.0, unit: Unit::Px, dpi: 72.0 },
    Preset { group: "Screen", name: "4K UHD", w: 3840.0, h: 2160.0, unit: Unit::Px, dpi: 72.0 },
    Preset { group: "Screen", name: "Webtoon", w: 800.0, h: 1280.0, unit: Unit::Px, dpi: 72.0 },
    Preset { group: "Screen", name: "Instagram portrait", w: 1080.0, h: 1350.0, unit: Unit::Px, dpi: 72.0 },
];

/// State of the open New dialog.
pub struct NewForm {
    name:   String,
    preset: Option<usize>,
    w:      f32,
    h:      f32,
    unit:   Unit,
    dpi:    f32,
}

impl NewForm {
    pub fn new() -> Self {
        let mut f = Self { name: "Untitled".into(), preset: None, w: 0.0, h: 0.0, unit: Unit::Px, dpi: 0.0 };
        f.pick(1); // A4 300 dpi
        f
    }

    fn pick(&mut self, i: usize) {
        let p = &PRESETS[i];
        (self.preset, self.w, self.h, self.unit, self.dpi) = (Some(i), p.w, p.h, p.unit, p.dpi);
    }

    fn pixels(&self) -> (u32, u32) {
        (self.unit.to_px(self.w, self.dpi), self.unit.to_px(self.h, self.dpi))
    }
}

impl Default for NewForm {
    fn default() -> Self { Self::new() }
}

/// The New document dialog. Returns false once closed.
pub fn new_dialog(ctx: &egui::Context, form: &mut NewForm, max_side: u32, a: &mut Actions) -> bool {
    let mut open = true;
    let modal = egui::Modal::new(egui::Id::new("new_document"))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(18.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("New document").heading());
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                // Presets, by group.
                ui.vertical(|ui| {
                    ui.set_width(210.0);
                    egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                        let mut group = "";
                        for (i, p) in PRESETS.iter().enumerate() {
                            if p.group != group {
                                group = p.group;
                                ui.add_space(4.0);
                                ui.label(RichText::new(group).small().color(theme::TEXT_WEAK));
                            }
                            let text = format!("{}  ·  {:.0} dpi", p.name, p.dpi);
                            let text = if p.unit == Unit::Px { p.name.to_string() } else { text };
                            if ui.selectable_label(form.preset == Some(i), text).clicked() { form.pick(i); }
                        }
                    });
                });
                ui.add_space(12.0);
                // Size.
                ui.vertical(|ui| {
                    let field = |ui: &mut Ui, label: &str| { ui.label(RichText::new(label).small().color(theme::TEXT_WEAK)); };
                    field(ui, "Name");
                    ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    let (before_w, before_h, before_dpi) = (form.w, form.h, form.dpi);
                    egui::Grid::new("new_size").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                        let speed = if form.unit == Unit::Px { 1.0 } else { 0.1 };
                        let digits = if form.unit == Unit::Px { 0 } else { 2 };
                        ui.label("Width");
                        ui.add(egui::DragValue::new(&mut form.w).range(0.01..=100_000.0).speed(speed).max_decimals(digits));
                        unit_combo(ui, form);
                        ui.end_row();
                        ui.label("Height");
                        ui.add(egui::DragValue::new(&mut form.h).range(0.01..=100_000.0).speed(speed).max_decimals(digits));
                        if ui.button("Swap").on_hover_text("Swap width and height (portrait / landscape)").clicked() {
                            std::mem::swap(&mut form.w, &mut form.h);
                            form.preset = None;
                        }
                        ui.end_row();
                        ui.label("Resolution");
                        ui.add(egui::DragValue::new(&mut form.dpi).range(1.0..=2400.0).speed(1.0).suffix(" dpi"));
                        ui.end_row();
                    });
                    if (form.w, form.h, form.dpi) != (before_w, before_h, before_dpi) { form.preset = None; }

                    let (pw, ph) = form.pixels();
                    ui.add_space(10.0);
                    ui.label(RichText::new(format!("{pw} × {ph} px")).strong());
                    let mb = pw as f64 * ph as f64 * 4.0 / (1024.0 * 1024.0);
                    ui.label(RichText::new(format!(
                        "≈ {mb:.0} MB for the image shown; each layer only takes memory where it is painted."
                    )).small().color(theme::TEXT_WEAK));
                    let too_big = pw > max_side || ph > max_side;
                    if too_big {
                        ui.label(RichText::new(format!("Too big for this GPU: at most {max_side} px per side."))
                            .color(Color32::from_rgb(0xff, 0x8a, 0x80)));
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() { open = false; }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let create = egui::Button::new(RichText::new("Create").color(theme::TEXT))
                                .fill(theme::ACCENT).min_size(vec2(96.0, 28.0));
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.add_enabled(!too_big, create).clicked() || (enter && !too_big) {
                                let name = form.name.trim();
                                a.new_doc = Some(NewDoc {
                                    name: if name.is_empty() { "Untitled".into() } else { name.into() },
                                    width: pw, height: ph, dpi: form.dpi,
                                });
                                open = false;
                            }
                        });
                    });
                });
            });
        });
    // Escape closes it; a click beside it doesn't (a slip would lose
    // what was typed).
    let _ = modal;
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) { open = false; }
    open
}

fn unit_combo(ui: &mut Ui, form: &mut NewForm) {
    egui::ComboBox::from_id_salt("new_unit").width(56.0).selected_text(form.unit.name()).show_ui(ui, |ui| {
        for u in Unit::ALL {
            if ui.selectable_label(form.unit == u, u.name()).clicked() && form.unit != u {
                // Same size, other unit.
                let (pw, ph) = form.pixels();
                form.w = u.of_px(pw, form.dpi);
                form.h = u.of_px(ph, form.dpi);
                form.unit = u;
            }
        }
    });
}

/// The start screen, over the canvas area.
pub fn start_screen(ctx: &egui::Context, area: Rect, recent: &mut [RecentFile], a: &mut Actions) {
    for r in recent.iter_mut() {
        if let Some(img) = r.pending.take() {
            r.thumb = Some(ctx.load_texture(format!("recent-{}", r.path.display()), img, egui::TextureOptions::LINEAR));
        }
    }
    egui::Area::new(egui::Id::new("start_screen"))
        .fixed_pos(area.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            ui.set_min_size(area.size());
            ui.set_max_size(area.size());
            ui.painter().rect_filled(area, theme::CANVAS_ROUNDING, theme::DESK);
            let col_w = (area.width() - 64.0).clamp(200.0, 760.0);
            let col = Rect::from_min_size(pos2(area.center().x - col_w / 2.0, area.top() + (area.height() * 0.14).min(120.0)), vec2(col_w, area.height()));
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(col), |ui| {
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new(egui::include_image!("../../../assets/logo/aurastudio-logo.svg"))
                        .fit_to_exact_size(vec2(84.0, 84.0)));
                    ui.add_space(10.0);
                    ui.label(RichText::new("AuraStudio").size(38.0).strong().color(theme::TEXT));
                    ui.label(RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).small().color(theme::TEXT_WEAK));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Create a new document or open a file.").color(theme::TEXT_WEAK));
                    ui.add_space(22.0);
                    // Two buttons, centered as a pair.
                    let (bw, gap) = (180.0, 12.0);
                    let (row, _) = ui.allocate_exact_size(vec2(bw * 2.0 + gap, 34.0), Sense::hover());
                    let new_r = Rect::from_min_size(row.min, vec2(bw, 34.0));
                    let open_r = Rect::from_min_size(row.min + vec2(bw + gap, 0.0), vec2(bw, 34.0));
                    if big_button(ui, new_r, "New…", "Ctrl+N", true) { a.file = Some(crate::FileCmd::New); }
                    if big_button(ui, open_r, "Open…", "Ctrl+O", false) { a.file = Some(crate::FileCmd::Open); }
                    ui.add_space(14.0);
                    ui.label(RichText::new("Drop an image or an .aurs file anywhere to open it.").small().color(theme::TEXT_WEAK));
                    if !recent.is_empty() {
                        ui.add_space(30.0);
                        recent_grid(ui, recent, a);
                    }
                });
            });
        });
}

fn big_button(ui: &mut Ui, rect: Rect, text: &str, key: &str, primary: bool) -> bool {
    let resp = ui.interact(rect, ui.id().with(text), Sense::click());
    let p = ui.painter();
    let hover = resp.hovered();
    if primary {
        p.rect_filled(rect, 17.0, if hover { theme::ACCENT.gamma_multiply(1.15) } else { theme::ACCENT });
    } else {
        p.rect(rect, 17.0, if hover { theme::WIDGET } else { Color32::TRANSPARENT }, Stroke::new(1.0_f32, theme::TEXT_WEAK));
    }
    p.text(rect.center() - vec2(22.0, 0.0), Align2::CENTER_CENTER, text, FontId::proportional(15.0), theme::TEXT);
    p.text(rect.right_center() - vec2(18.0, 0.0), Align2::RIGHT_CENTER, key, FontId::proportional(12.0),
        if primary { Color32::from_white_alpha(200) } else { theme::TEXT_WEAK });
    resp.clicked()
}

fn recent_grid(ui: &mut Ui, recent: &mut [RecentFile], a: &mut Actions) {
    const CARD: f32 = 148.0;
    const GAP: f32 = 12.0;
    let cols = ((ui.available_width() + GAP) / (CARD + GAP)).floor().max(1.0) as usize;
    // The grid as one block, centered like the rest, its rows left-aligned.
    let n = cols.min(recent.len());
    let width = n as f32 * (CARD + GAP) - GAP;
    ui.allocate_ui(vec2(width, 0.0), |ui| {
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Recent").strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let clear = egui::Button::new(RichText::new("Clear list").small().color(theme::TEXT_WEAK)).frame(false);
                    if ui.add(clear).on_hover_text("Forget the recent files (the files stay on disk)").clicked() {
                        a.clear_recent = true;
                    }
                });
            });
            ui.add_space(6.0);
            for row in recent.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for r in row { recent_card(ui, r, CARD, a); }
                });
                ui.add_space(8.0);
            }
        });
    });
}

fn recent_card(ui: &mut Ui, r: &RecentFile, card: f32, a: &mut Actions) {
    let (rect, resp) = ui.allocate_exact_size(vec2(card, card + 38.0), Sense::click());
    let resp = resp.on_hover_text(r.path.display().to_string());
    let p = ui.painter_at(rect);
    if resp.hovered() { p.rect_filled(rect, 8.0, theme::WIDGET); }
    let img = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(card - 12.0, card - 12.0));
    p.rect_filled(img, 6.0, theme::PANEL);
    if let Some(t) = &r.thumb {
        let s = t.size_vec2();
        let k = (img.width() / s.x).min(img.height() / s.y);
        let fit = Rect::from_center_size(img.center(), s * k);
        p.image(t.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    // Long names end with "…" inside the card.
    crate::widgets::elided_text(&p, pos2(rect.left() + 8.0, img.bottom() + 12.0), &r.name,
        FontId::proportional(13.0), theme::TEXT, card - 16.0);
    crate::widgets::elided_text(&p, pos2(rect.left() + 8.0, img.bottom() + 28.0), &r.detail,
        FontId::proportional(11.0), theme::TEXT_WEAK, card - 16.0);
    if resp.clicked() { a.open_path = Some(r.path.clone()); }
    resp.context_menu(|ui| {
        if ui.button("Remove from list").clicked() {
            a.forget_recent = Some(r.path.clone());
            ui.close_menu();
        }
    });
}

/// Give a recent file its thumbnail (RGBA, decoded by the app).
pub fn set_thumbnail(recent: &mut [RecentFile], path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    if let Some(r) = recent.iter_mut().find(|r| r.path == path) {
        r.pending = Some(egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_give_the_expected_pixels() {
        let px = |name: &str, dpi: f32| {
            let mut f = NewForm::new();
            f.pick(PRESETS.iter().position(|p| p.name == name && p.dpi == dpi).unwrap());
            f.pixels()
        };
        assert_eq!(px("A4", 300.0), (2480, 3508));
        assert_eq!(px("B4 manuscript", 600.0), (6071, 8598));
        assert_eq!(px("Full HD", 72.0), (1920, 1080));
    }

    #[test]
    fn unit_change_keeps_the_size() {
        for u in Unit::ALL {
            let v = u.of_px(2480, 300.0);
            assert_eq!(u.to_px(v, 300.0), 2480, "{u:?}");
        }
    }
}
