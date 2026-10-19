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

/// A size the New dialog offers.
pub struct Preset {
    pub name: &'static str,
    pub w:    f32,
    pub h:    f32,
    unit:     Unit,
    pub dpi:  f32,
    /// What it is for, shown under the details.
    pub note: &'static str,
}

const fn mm(name: &'static str, w: f32, h: f32, dpi: f32, note: &'static str) -> Preset { Preset { name, w, h, unit: Unit::Mm, dpi, note } }
const fn inch(name: &'static str, w: f32, h: f32, dpi: f32, note: &'static str) -> Preset { Preset { name, w, h, unit: Unit::In, dpi, note } }
const fn px(name: &'static str, w: f32, h: f32, note: &'static str) -> Preset { Preset { name, w, h, unit: Unit::Px, dpi: 72.0, note } }

/// The dialog's tabs and their presets. Comics: black-and-white pages at
/// 600 dpi (manga manuscripts), color ones at 400; animation and screens
/// in pixels (the DCI sizes: Digital Cinema Initiatives).
pub const CATEGORIES: &[(&str, &[Preset])] = &[
    ("Painting", &[
        mm("A5", 148.0, 210.0, 300.0, "Sketchbook page"),
        mm("A4", 210.0, 297.0, 300.0, "Print, 300 dpi"),
        mm("A3", 297.0, 420.0, 300.0, "Print, 300 dpi"),
        inch("US Letter", 8.5, 11.0, 300.0, "Print, 300 dpi"),
        inch("Tabloid", 11.0, 17.0, 300.0, "Print, 300 dpi"),
        inch("Poster 18 × 24", 18.0, 24.0, 300.0, "Print, 300 dpi"),
        px("Square 2K", 2048.0, 2048.0, "Studies, speed paintings"),
        px("Square 4K", 4096.0, 4096.0, "Large illustration"),
        px("Portrait 3:4", 3000.0, 4000.0, "Illustration for screens"),
        px("Wallpaper 4K", 3840.0, 2160.0, "16:9 screen, keyframes"),
        px("Instagram portrait", 1080.0, 1350.0, "4:5 post"),
        px("Instagram square", 1080.0, 1080.0, "1:1 post"),
    ]),
    ("Animation", &[
        px("HD 720p", 1280.0, 720.0, "16:9, light: animatics and tests"),
        px("Full HD 1080p", 1920.0, 1080.0, "16:9, TV and web standard"),
        px("4K UHD", 3840.0, 2160.0, "16:9, UHD TV and streaming"),
        px("8K UHD", 7680.0, 4320.0, "16:9"),
        px("DCI 2K", 2048.0, 1080.0, "1.90:1, cinema full container"),
        px("DCI 2K Flat", 1998.0, 1080.0, "1.85:1, cinema"),
        px("DCI 2K Scope", 2048.0, 858.0, "2.39:1, cinema widescreen"),
        px("DCI 4K", 4096.0, 2160.0, "1.90:1, cinema full container"),
        px("DCI 4K Flat", 3996.0, 2160.0, "1.85:1, cinema"),
        px("DCI 4K Scope", 4096.0, 1716.0, "2.39:1, cinema widescreen"),
        px("1.66:1", 1800.0, 1080.0, "European widescreen"),
        px("4:3", 1440.0, 1080.0, "Classic TV, 1080 lines"),
        px("Vertical 9:16", 1080.0, 1920.0, "Shorts, Reels, TikTok"),
        px("Square 1:1", 1080.0, 1080.0, "Social posts"),
    ]),
    ("Comics", &[
        mm("Manga B4 manuscript", 257.0, 364.0, 600.0, "Japanese manuscript paper; inner frame 180 × 270 mm, printed at B5"),
        mm("Manga A4 manuscript", 210.0, 297.0, 600.0, "Doujinshi manuscript, printed at B5 or A5"),
        mm("Manga B5", 182.0, 257.0, 600.0, "Magazine / B5 doujinshi page"),
        mm("Manga A5", 148.0, 210.0, 600.0, "A5 doujinshi page"),
        mm("Manga B6 tankōbon", 128.0, 182.0, 600.0, "Collected volume page"),
        inch("US comic, bleed", 6.875, 10.4375, 600.0, "Trim 6.625 × 10.25 in, 0.125 in bleed"),
        inch("US comic art board", 11.0, 17.0, 400.0, "Original art, printed reduced"),
        inch("Graphic novel 6 × 9", 6.0, 9.0, 400.0, "Trade paperback"),
        mm("BD album", 240.0, 320.0, 400.0, "European album page (franco-belge)"),
        mm("A4 comic", 210.0, 297.0, 400.0, "European / print-on-demand"),
        px("Webtoon", 800.0, 1280.0, "Vertical scroll, 800 px wide"),
        px("Webtoon 2×", 1600.0, 2560.0, "Drawn at twice the web width"),
        px("Instagram comic", 1080.0, 1350.0, "4:5 carousel page"),
    ]),
];

/// State of the open New dialog.
pub struct NewForm {
    name:     String,
    category: usize,
    /// (category, preset) chosen; None once a value was changed by hand.
    preset:   Option<(usize, usize)>,
    w:        f32,
    h:        f32,
    unit:     Unit,
    dpi:      f32,
}

impl NewForm {
    /// The dialog on `category` (the last one used), its second preset.
    pub fn new(category: usize) -> Self {
        let category = category.min(CATEGORIES.len() - 1);
        let mut f = Self { name: "Untitled".into(), category, preset: None, w: 0.0, h: 0.0, unit: Unit::Px, dpi: 0.0 };
        f.pick(category, 1);
        f
    }

    pub fn category(&self) -> usize { self.category }

    fn pick(&mut self, c: usize, i: usize) {
        let p = &CATEGORIES[c].1[i];
        (self.preset, self.w, self.h, self.unit, self.dpi) = (Some((c, i)), p.w, p.h, p.unit, p.dpi);
    }

    fn pixels(&self) -> (u32, u32) {
        (self.unit.to_px(self.w, self.dpi), self.unit.to_px(self.h, self.dpi))
    }
}

/// Size line of a preset card ("210 × 297 mm · 300 dpi", "1920 × 1080 px").
fn size_line(p: &Preset) -> String {
    let n = |v: f32| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
    match p.unit {
        Unit::Px => format!("{} × {} px", n(p.w), n(p.h)),
        u => format!("{} × {} {} · {:.0} dpi", n(p.w), n(p.h), u.name(), p.dpi),
    }
}

/// A page outline with the size's proportions, in `r`.
fn page_icon(ui: &Ui, r: Rect, w: f32, h: f32, color: Color32) {
    let s = r.width().min(r.height()) / w.max(h);
    let page = Rect::from_center_size(r.center(), vec2(w * s, h * s));
    ui.painter().rect_stroke(page, 2.0, Stroke::new(1.3_f32, color));
}

/// The New document dialog (laid out like PhotoCraft's: tabs, preset
/// cards, the details on the right). Returns false once closed.
pub fn new_dialog(ctx: &egui::Context, form: &mut NewForm, max_side: u32, a: &mut Actions) -> bool {
    let mut open = true;
    let mut create = false;
    let _ = egui::Modal::new(egui::Id::new("new_document"))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(18.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(820.0);
            ui.label(RichText::new("New document").heading());
            ui.add_space(8.0);
            // Tabs, underlined when chosen.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 20.0;
                for (c, (name, _)) in CATEGORIES.iter().enumerate() {
                    let on = form.category == c;
                    let r = ui.add(egui::Label::new(RichText::new(*name).size(14.0).color(if on { theme::TEXT } else { theme::TEXT_WEAK }))
                        .selectable(false).sense(Sense::click()));
                    if on {
                        ui.painter().line_segment([r.rect.left_bottom() + vec2(0.0, 3.0), r.rect.right_bottom() + vec2(0.0, 3.0)], Stroke::new(2.0_f32, theme::ACCENT));
                    }
                    if r.clicked() && !on {
                        form.category = c;
                        form.pick(c, 1);
                    }
                }
            });
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                // Left: the presets as cards.
                ui.vertical(|ui| {
                    ui.set_width(500.0);
                    let presets = CATEGORIES[form.category].1;
                    let card = vec2(160.0, 104.0);
                    egui::ScrollArea::vertical().id_salt(("new_presets", form.category)).max_height(3.0 * (card.y + 8.0) + 4.0).show(ui, |ui| {
                        for (row, chunk) in presets.chunks(3).enumerate() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for (k, p) in chunk.iter().enumerate() {
                                    let i = row * 3 + k;
                                    let on = form.preset == Some((form.category, i));
                                    let (r, resp) = ui.allocate_exact_size(card, Sense::click());
                                    let resp = resp.on_hover_text(p.note);
                                    let bg = if on { theme::ACCENT_DIM } else if resp.hovered() { theme::WIDGET_HOVER } else { theme::WIDGET };
                                    ui.painter().rect_filled(r, 6.0, bg);
                                    if on { ui.painter().rect_stroke(r, 6.0, Stroke::new(1.5_f32, theme::ACCENT)); }
                                    page_icon(ui, Rect::from_center_size(pos2(r.center().x, r.top() + 30.0), vec2(38.0, 38.0)), p.w, p.h, theme::TEXT_WEAK);
                                    ui.painter().text(pos2(r.center().x, r.top() + 64.0), Align2::CENTER_CENTER, p.name, FontId::proportional(12.5), theme::TEXT);
                                    ui.painter().text(pos2(r.center().x, r.top() + 84.0), Align2::CENTER_CENTER, size_line(p), FontId::proportional(10.5), theme::TEXT_WEAK);
                                    if resp.clicked() { form.pick(form.category, i); }
                                    if resp.double_clicked() { create = true; }
                                }
                            });
                            ui.add_space(8.0);
                        }
                    });
                });
                ui.add_space(14.0);
                // Right: the details.
                ui.vertical(|ui| {
                    ui.set_width(280.0);
                    let label = |ui: &mut Ui, text: &str| { ui.label(RichText::new(text).small().color(theme::TEXT_WEAK)); };
                    label(ui, "Name");
                    ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    let before = (form.w, form.h, form.dpi);
                    let speed = if form.unit == Unit::Px { 1.0 } else { 0.1 };
                    let digits = if form.unit == Unit::Px { 0 } else { 2 };
                    egui::Grid::new("new_size").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                        ui.label("Width");
                        ui.add(egui::DragValue::new(&mut form.w).range(0.01..=100_000.0).speed(speed).max_decimals(digits));
                        unit_combo(ui, form);
                        ui.end_row();
                        ui.label("Height");
                        ui.add(egui::DragValue::new(&mut form.h).range(0.01..=100_000.0).speed(speed).max_decimals(digits));
                        ui.end_row();
                        ui.label("Resolution");
                        ui.add(egui::DragValue::new(&mut form.dpi).range(1.0..=2400.0).speed(1.0).suffix(" dpi"));
                        ui.end_row();
                    });
                    ui.add_space(4.0);
                    // Orientation: a portrait and a landscape page.
                    ui.horizontal(|ui| {
                        label(ui, "Orientation");
                        for portrait in [true, false] {
                            let on = (form.h >= form.w) == portrait;
                            let (r, resp) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::click());
                            let resp = resp.on_hover_text(if portrait { "Portrait" } else { "Landscape" });
                            if on { ui.painter().rect_filled(r, 4.0, theme::ACCENT_DIM); } else if resp.hovered() { ui.painter().rect_filled(r, 4.0, theme::WIDGET_HOVER); }
                            let (w, h) = if portrait { (3.0, 4.0) } else { (4.0, 3.0) };
                            page_icon(ui, r.shrink(5.0), w, h, if on { theme::TEXT } else { theme::TEXT_WEAK });
                            if resp.clicked() && !on { std::mem::swap(&mut form.w, &mut form.h); }
                        }
                    });
                    if (form.w, form.h, form.dpi) != before && (form.w, form.h) != (before.1, before.0) { form.preset = None; }

                    let (pw, ph) = form.pixels();
                    ui.add_space(10.0);
                    ui.label(RichText::new(format!("{pw} × {ph} px")).strong());
                    if let Some((c, i)) = form.preset {
                        ui.label(RichText::new(CATEGORIES[c].1[i].note).small().color(theme::TEXT_WEAK));
                    }
                    let mb = pw as f64 * ph as f64 * 4.0 / (1024.0 * 1024.0);
                    ui.label(RichText::new(format!("≈ {mb:.0} MB for the image shown; each layer only takes memory where it is painted."))
                        .small().color(theme::TEXT_WEAK));
                    let too_big = pw > max_side || ph > max_side;
                    if too_big {
                        ui.label(RichText::new(format!("Too big for this GPU: at most {max_side} px per side."))
                            .color(Color32::from_rgb(0xff, 0x8a, 0x80)));
                    }
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() { open = false; }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let b = egui::Button::new(RichText::new("Create").color(theme::TEXT)).fill(theme::ACCENT).min_size(vec2(96.0, 28.0));
                            if ui.add_enabled(!too_big, b).clicked() { create = true; }
                        });
                    });
                    if create && too_big { create = false; }
                });
            });
        });
    if ctx.memory(|m| m.focused().is_none()) && ctx.input(|i| i.key_pressed(egui::Key::Enter)) { create = true; }
    if create {
        let (pw, ph) = form.pixels();
        if pw <= max_side && ph <= max_side {
            let name = form.name.trim();
            a.new_doc = Some(NewDoc { name: if name.is_empty() { "Untitled".into() } else { name.into() }, width: pw, height: ph, dpi: form.dpi });
            open = false;
        }
    }
    // Escape closes it; a click beside it doesn't (a slip would lose
    // what was typed).
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
                    if ui.add(clear).on_hover_text("The files stay on disk").clicked() {
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
        let px = |c: &str, name: &str| {
            let ci = CATEGORIES.iter().position(|x| x.0 == c).unwrap();
            let mut f = NewForm::new(ci);
            f.pick(ci, CATEGORIES[ci].1.iter().position(|p| p.name == name).unwrap());
            f.pixels()
        };
        assert_eq!(px("Painting", "A4"), (2480, 3508));
        assert_eq!(px("Comics", "Manga B4 manuscript"), (6071, 8598));
        assert_eq!(px("Comics", "US comic, bleed"), (4125, 6263));
        assert_eq!(px("Animation", "DCI 4K Scope"), (4096, 1716));
        // Every category opens on a preset.
        for c in 0..CATEGORIES.len() { assert!(NewForm::new(c).preset.is_some()); }
    }

    #[test]
    fn unit_change_keeps_the_size() {
        for u in Unit::ALL {
            let v = u.of_px(2480, 300.0);
            assert_eq!(u.to_px(v, 300.0), 2480, "{u:?}");
        }
    }
}
