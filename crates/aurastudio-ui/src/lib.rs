//! AuraStudio user interface (egui): top bar, Procreate-like tool strip
//! on the left, CSP-like dock column on the right (tabs can be rearranged,
//! resized and dragged out as floating windows), canvas in the middle.
//!
//! The app owns the data (brush, layers...) and lends it through `Model`
//! each frame; the UI edits it directly and reports anything the app must
//! act on (recomposite, canvas resize) in `Actions`.

pub mod theme;
mod widgets;

use std::collections::{HashMap, HashSet};

use aurastudio_brush::Brush;
use aurastudio_layers::LayerStack;
use egui::{pos2, vec2, Align, Color32, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};

use widgets::*;

/// Brush size range, as a diameter in pixels.
pub const MAX_BRUSH_SIZE: u32 = 1000;

/// Everything the UI may edit this frame.
pub struct Model<'a> {
    pub brush:              &'a mut Brush,
    pub use_hokusai:        &'a mut bool,
    /// Pressure smoothing distance, as a fraction of the brush radius.
    pub pressure_smoothing: &'a mut f32,
    pub layers:             &'a mut LayerStack,
    /// Width / height typed in the Canvas dock, applied by "Resize".
    pub canvas_edit:        &'a mut (u32, u32),
    pub max_canvas:         u32,
}

/// What the app must do after the UI ran.
#[derive(Default)]
pub struct Actions {
    /// Layer visibility / opacity / order / content changed outside a
    /// stroke: recomposite the whole canvas.
    pub recomposite: bool,
    pub resize_to:   Option<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab { Color, Brush, Layers, Canvas }

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorMode { Rgb, Hsv }

/// UI state kept between frames.
pub struct UiState {
    dock:  DockState<Tab>,
    panel: PanelState,
}

struct PanelState {
    theme_applied: bool,
    color_mode:    ColorMode,
    /// Master color in HSV, so hue survives when saturation or value hit 0
    /// (converting back from RGB would lose it).
    hsv:           [f32; 3],
    hex:           String,
    thumbs:        HashMap<u64, egui::TextureHandle>,
    stale_thumbs:  HashSet<u64>,
    /// Layer being renamed: (id, text, focus not requested yet).
    renaming:      Option<(u64, String, bool)>,
    focus_tab:     Option<Tab>,
}

impl Default for UiState {
    fn default() -> Self { Self::new() }
}

impl UiState {
    pub fn new() -> Self {
        Self {
            dock:  default_dock(),
            panel: PanelState {
                theme_applied: false,
                color_mode:    ColorMode::Hsv,
                hsv:           [0.0, 0.0, 0.0],
                hex:           String::new(),
                thumbs:        HashMap::new(),
                stale_thumbs:  HashSet::new(),
                renaming:      None,
                focus_tab:     None,
            },
        }
    }

    /// The pixels of this layer changed: rebuild its thumbnail.
    pub fn invalidate_thumbnail(&mut self, layer_id: u64) {
        self.panel.stale_thumbs.insert(layer_id);
    }

    /// Every layer changed (canvas resize...).
    pub fn invalidate_all_thumbnails(&mut self) {
        self.panel.thumbs.clear();
    }
}

/// Right column: Color + Brush on top, Layers + Canvas below.
fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Color, Tab::Brush]);
    dock.main_surface_mut().split_below(NodeIndex::root(), 0.30, vec![Tab::Layers, Tab::Canvas]);
    dock
}

/// Build the whole UI for this frame.
pub fn show(ctx: &egui::Context, state: &mut UiState, model: &mut Model, actions: &mut Actions) {
    if !state.panel.theme_applied {
        theme::apply(ctx);
        state.panel.theme_applied = true;
    }
    refresh_thumbnails(ctx, &mut state.panel, model.layers);

    top_bar(ctx, state, model, actions);
    tool_strip(ctx, &mut state.panel, model);

    if let Some(tab) = state.panel.focus_tab.take() {
        if let Some(loc) = state.dock.find_tab(&tab) {
            state.dock.set_active_tab(loc);
        }
    }

    egui::SidePanel::right("dock")
        .resizable(true)
        .default_width(310.0)
        .width_range(250.0..=600.0)
        .frame(Frame::none().fill(theme::SURFACE))
        .show(ctx, |ui| {
            let style = theme::dock_style(ui.style());
            let mut viewer = Viewer { p: &mut state.panel, m: model, a: actions };
            DockArea::new(&mut state.dock)
                .style(style)
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .window_bounds(ctx.screen_rect())
                .show_inside(ui, &mut viewer);
        });
}

// --- Top bar ------------------------------------------------------------------

fn top_bar(ctx: &egui::Context, state: &mut UiState, m: &mut Model, a: &mut Actions) {
    egui::TopBottomPanel::top("top_bar")
        .exact_height(34.0)
        .frame(Frame::none().fill(theme::PANEL).inner_margin(Margin::symmetric(10.0, 0.0)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("AuraStudio").strong().color(theme::TEXT));
                ui.add_space(10.0);

                ui.menu_button("Layer", |ui| {
                    if ui.button("New layer").clicked() {
                        new_layer(m, a);
                        ui.close_menu();
                    }
                    if ui.button("Duplicate").clicked() {
                        m.layers.duplicate_active();
                        a.recomposite = true;
                        ui.close_menu();
                    }
                    if ui.add(egui::Button::new("Clear").shortcut_text("Del")).clicked() {
                        let id = m.layers.layers[m.layers.active].id;
                        m.layers.clear_active();
                        state.panel.stale_thumbs.insert(id);
                        a.recomposite = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(m.layers.layers.len() > 1, egui::Button::new("Delete")).clicked() {
                        m.layers.remove_layer(m.layers.active);
                        a.recomposite = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Canvas", |ui| {
                    if ui.button("Canvas size…").clicked() {
                        state.panel.focus_tab = Some(Tab::Canvas);
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Reset docks").clicked() {
                        state.dock = default_dock();
                        ui.close_menu();
                    }
                });

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let n = m.layers.layers.len();
                    ui.label(RichText::new(format!(
                        "{} × {} px  ·  {} layer{}",
                        m.layers.width, m.layers.height, n, if n > 1 { "s" } else { "" }
                    )).color(theme::TEXT_WEAK));
                });
            });
        });
}

fn new_layer(m: &mut Model, a: &mut Actions) {
    let n = m.layers.layers.len();
    m.layers.add_layer(&format!("Layer {n}"));
    a.recomposite = true;
}

// --- Tool strip (Procreate-like) --------------------------------------------

fn tool_strip(ctx: &egui::Context, p: &mut PanelState, m: &mut Model) {
    egui::SidePanel::left("tool_strip")
        .exact_width(56.0)
        .resizable(false)
        .frame(Frame::none().fill(theme::PANEL).inner_margin(Margin::symmetric(0.0, 14.0)))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                let color = Color32::from_rgb(m.brush.color[0], m.brush.color[1], m.brush.color[2]);
                if color_dot(ui, color, 34.0).on_hover_text("Color").clicked() {
                    p.focus_tab = Some(Tab::Color);
                }
                ui.add_space(22.0);

                // Size: logarithmic, so 1..1000 px stays precise at small sizes.
                let diameter = (m.brush.radius * 2).clamp(1, MAX_BRUSH_SIZE) as f32;
                let mut t = diameter.ln() / (MAX_BRUSH_SIZE as f32).ln();
                if vertical_slider(ui, &mut t, 170.0, &format!("{}", diameter as u32), "Brush size (px)") {
                    let d = (t * (MAX_BRUSH_SIZE as f32).ln()).exp().round() as u32;
                    m.brush.radius = (d / 2).max(1);
                }
                ui.add_space(18.0);

                let mut o = m.brush.opacity;
                let label = format!("{}%", (o * 100.0).round());
                if vertical_slider(ui, &mut o, 170.0, &label, "Brush opacity") {
                    m.brush.opacity = o;
                }
            });
        });
}

// --- Dock tabs ------------------------------------------------------------------

struct Viewer<'s, 'a> {
    p: &'s mut PanelState,
    m: &'s mut Model<'a>,
    a: &'s mut Actions,
}

impl TabViewer for Viewer<'_, '_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        match tab {
            Tab::Color  => "Color",
            Tab::Brush  => "Brush",
            Tab::Layers => "Layers",
            Tab::Canvas => "Canvas",
        }.into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::Color  => color_tab(ui, self.p, self.m),
            Tab::Brush  => brush_tab(ui, self.m),
            Tab::Layers => layers_tab(ui, self.p, self.m, self.a),
            Tab::Canvas => canvas_tab(ui, self.m, self.a),
        }
    }

    fn closeable(&mut self, _tab: &mut Tab) -> bool { false }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        // The layer list scrolls by itself.
        [false, *tab != Tab::Layers]
    }
}

// --- Color --------------------------------------------------------------------

fn color_tab(ui: &mut Ui, p: &mut PanelState, m: &mut Model) {
    let to_u8 = |rgb: [f32; 3]| rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let to_f  = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);

    // The brush color may have changed elsewhere: resync the HSV master,
    // keeping the hue (and saturation) when they become undefined.
    if to_u8(hsv_to_rgb(p.hsv)) != m.brush.color {
        let [h, s, v] = rgb_to_hsv(to_f(m.brush.color));
        let h = if s > 0.0 && v > 0.0 { h } else { p.hsv[0] };
        let s = if v > 0.0 { s } else { p.hsv[1] };
        p.hsv = [h, s, v];
    }

    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if vertical_tab(ui, "RGB", p.color_mode == ColorMode::Rgb).clicked() { p.color_mode = ColorMode::Rgb; }
            if vertical_tab(ui, "HSV", p.color_mode == ColorMode::Hsv).clicked() { p.color_mode = ColorMode::Hsv; }
        });

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            match p.color_mode {
                ColorMode::Hsv => {
                    let [h, s, v] = p.hsv;
                    let mut new = p.hsv;
                    // Hue bar at full saturation / value, like most apps;
                    // S and V bars show the real color along the range.
                    channel_row(ui, "H", &mut new[0], 360.0, &|t| hsv_to_color32([t, 1.0, 1.0]));
                    channel_row(ui, "S", &mut new[1], 100.0, &|t| hsv_to_color32([h, t, v]));
                    channel_row(ui, "V", &mut new[2], 100.0, &|t| hsv_to_color32([h, s, t]));
                    if new != p.hsv {
                        p.hsv = new;
                        m.brush.color = to_u8(hsv_to_rgb(new));
                    }
                }
                ColorMode::Rgb => {
                    let [r, g, b] = to_f(m.brush.color);
                    let mut new = [r, g, b];
                    channel_row(ui, "R", &mut new[0], 255.0, &|t| rgb_to_color32([t, g, b]));
                    channel_row(ui, "G", &mut new[1], 255.0, &|t| rgb_to_color32([r, t, b]));
                    channel_row(ui, "B", &mut new[2], 255.0, &|t| rgb_to_color32([r, g, t]));
                    if new != [r, g, b] {
                        m.brush.color = to_u8(new);
                    }
                }
            }
        });
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let [r, g, b] = m.brush.color;
        let color = Color32::from_rgb(r, g, b);
        let w = (ui.available_width() - 96.0).max(40.0);
        let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
        ui.painter().rect_filled(rect, 8.0, color);
        ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.0_f32, theme::BORDER));

        let edit = ui.add_sized(vec2(88.0, 26.0), egui::TextEdit::singleline(&mut p.hex).horizontal_align(Align::Center));
        if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) && edit.has_focus() {
            if let Some(c) = parse_hex(&p.hex) {
                m.brush.color = c;
            }
        }
        if !edit.has_focus() {
            p.hex = format!("#{r:02X}{g:02X}{b:02X}");
        }
    });
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 { return None; }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

// --- Brush --------------------------------------------------------------------

fn brush_tab(ui: &mut Ui, m: &mut Model) {
    egui::Grid::new("brush_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label("Size");
        let mut d = (m.brush.radius * 2).clamp(1, MAX_BRUSH_SIZE);
        if ui.add(egui::Slider::new(&mut d, 1..=MAX_BRUSH_SIZE).logarithmic(true).suffix(" px")).changed() {
            m.brush.radius = (d / 2).max(1);
        }
        ui.end_row();

        ui.label("Opacity");
        let mut o = (m.brush.opacity * 100.0).round() as u32;
        if ui.add(egui::Slider::new(&mut o, 0..=100).suffix(" %")).changed() {
            m.brush.opacity = o as f32 / 100.0;
        }
        ui.end_row();

        ui.label("Pressure smoothing");
        ui.add(egui::Slider::new(m.pressure_smoothing, 0.0..=1.5).fixed_decimals(2))
            .on_hover_text("Limits sudden pressure jumps over a distance relative to the brush size.\n0 = raw pen pressure.");
        ui.end_row();
    });

    ui.add_space(10.0);
    ui.label(RichText::new("Engine").color(theme::TEXT_WEAK));
    ui.radio_value(m.use_hokusai, false, "Round pen")
        .on_hover_text("Continuous stroke, crisp and fast. Opacity never builds up within a stroke.");
    ui.radio_value(m.use_hokusai, true, "hokusai (libmypaint)")
        .on_hover_text("Dab-based engine, for textured / painterly brushes.");
}

// --- Layers -------------------------------------------------------------------

const THUMB: f32 = 40.0;
const ROW_H: f32 = 50.0;

fn layers_tab(ui: &mut Ui, p: &mut PanelState, m: &mut Model, a: &mut Actions) {
    // Active layer settings, Krita / CSP style: blend mode + opacity.
    let active = m.layers.active;
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("blend_mode")
            .selected_text("Normal")
            .width(100.0)
            .show_ui(ui, |ui| { let _ = ui.selectable_label(true, "Normal"); });

        let mut pct = (m.layers.layers[active].opacity * 100.0).round() as u32;
        ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(40.0);
        if ui.add(egui::Slider::new(&mut pct, 0..=100).suffix(" %")).on_hover_text("Layer opacity").changed() {
            m.layers.layers[active].opacity = pct as f32 / 100.0;
            a.recomposite = true;
        }
    });
    ui.add_space(4.0);

    let toolbar_h = 30.0;
    let list_h = (ui.available_height() - toolbar_h - 8.0).max(ROW_H);

    let mut select = None;
    let mut toggle = None;
    Frame::none().fill(theme::SURFACE).rounding(6.0).show(ui, |ui| {
        ui.set_min_height(list_h);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for i in (0..m.layers.layers.len()).rev() {
                match layer_row(ui, p, m, i) {
                    RowEvent::Select => select = Some(i),
                    RowEvent::ToggleVisible => toggle = Some(i),
                    RowEvent::None => {}
                }
            }
        });
    });
    if let Some(i) = select { m.layers.set_active(i); }
    if let Some(i) = toggle {
        m.layers.layers[i].visible ^= true;
        a.recomposite = true;
    }

    // Bottom toolbar.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if icon_button(ui, "➕", "New layer", true) { new_layer(m, a); }
        if icon_button(ui, "🗐", "Duplicate layer", true) {
            m.layers.duplicate_active();
            a.recomposite = true;
        }
        let top = m.layers.active + 1 == m.layers.layers.len();
        let bottom = m.layers.active == 0;
        if icon_button(ui, "⏶", "Move up", !top) {
            m.layers.move_active(true);
            a.recomposite = true;
        }
        if icon_button(ui, "⏷", "Move down", !bottom) {
            m.layers.move_active(false);
            a.recomposite = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, "🗑", "Delete layer", m.layers.layers.len() > 1) {
                m.layers.remove_layer(m.layers.active);
                a.recomposite = true;
            }
        });
    });
}

enum RowEvent { None, Select, ToggleVisible }

fn layer_row(ui: &mut Ui, p: &mut PanelState, m: &mut Model, i: usize) -> RowEvent {
    let layer = &m.layers.layers[i];
    let (id, visible, opacity) = (layer.id, layer.visible, layer.opacity);
    let layer_name = layer.name.clone();
    let active = m.layers.active == i;
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
    let painter = ui.painter_at(rect);

    let bg = if active {
        theme::ACCENT_DIM
    } else if resp.hovered() {
        theme::WIDGET
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect.shrink2(vec2(2.0, 0.0)), 6.0, bg);

    // Eye.
    let eye = Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(24.0, 24.0));
    paint_eye(&painter, eye.center(), visible);

    // Thumbnail on a checkerboard (transparency).
    let thumb_box = Rect::from_center_size(pos2(eye.right() + 8.0 + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
    if let Some(tex) = p.thumbs.get(&id) {
        let s = tex.size_vec2();
        let k = (THUMB / s.x).min(THUMB / s.y);
        let r = Rect::from_center_size(thumb_box.center(), s * k);
        checker(&painter, r, 5.0);
        painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, if active { theme::ACCENT } else { theme::BORDER }));
    }

    // Name + "100 % · Normal" (CSP).
    let text_x = thumb_box.right() + 10.0;
    let name_rect = Rect::from_min_max(pos2(text_x, rect.top() + 6.0), pos2(rect.right() - 8.0, rect.center().y + 2.0));
    let renaming = p.renaming.as_ref().is_some_and(|(rid, _, _)| *rid == id);
    if renaming {
        let (_, buf, first) = p.renaming.as_mut().unwrap();
        let edit = ui.put(name_rect, egui::TextEdit::singleline(buf));
        // Focus once: requesting it every frame kept the field open after
        // Enter (focus came straight back).
        if *first {
            edit.request_focus();
            *first = false;
        }
        if edit.lost_focus() {
            let name = buf.trim().to_owned();
            if !name.is_empty() {
                m.layers.layers[i].name = name;
            }
            p.renaming = None;
        }
    } else {
        let name = RichText::new(&layer_name).color(theme::TEXT);
        let name = if active { name.strong() } else { name };
        painter.text(pos2(text_x, rect.center().y - 7.0), egui::Align2::LEFT_CENTER,
            name.text(), egui::FontId::proportional(13.0), if active { Color32::WHITE } else { theme::TEXT });
    }
    painter.text(
        pos2(text_x, rect.center().y + 10.0),
        egui::Align2::LEFT_CENTER,
        format!("{:.0} %  ·  Normal", opacity * 100.0),
        egui::FontId::proportional(10.5),
        theme::TEXT_WEAK,
    );

    if resp.double_clicked() && !renaming && resp.interact_pointer_pos().is_some_and(|pt| pt.x > text_x) {
        p.renaming = Some((id, layer_name, true));
        return RowEvent::None;
    }
    if resp.clicked() {
        if resp.interact_pointer_pos().is_some_and(|pt| eye.contains(pt)) {
            return RowEvent::ToggleVisible;
        }
        return RowEvent::Select;
    }
    RowEvent::None
}

/// Eye icon drawn with shapes (no dependency on font glyphs).
fn paint_eye(painter: &egui::Painter, c: egui::Pos2, open: bool) {
    let col = if open { theme::TEXT } else { theme::TEXT_WEAK.gamma_multiply(0.6) };
    let pts: Vec<_> = (0..=24)
        .map(|k| {
            let a = k as f32 / 24.0 * std::f32::consts::TAU;
            pos2(c.x + 7.5 * a.cos(), c.y + 4.5 * a.sin() * (1.0 - 0.25 * a.cos().abs()))
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, Stroke::new(1.3_f32, col)));
    if open {
        painter.circle_filled(c, 2.4, col);
    } else {
        painter.line_segment([pos2(c.x - 7.0, c.y + 6.0), pos2(c.x + 7.0, c.y - 6.0)], Stroke::new(1.3_f32, col));
    }
}

/// Keep one small texture per layer, rebuilt when its pixels changed.
fn refresh_thumbnails(ctx: &egui::Context, p: &mut PanelState, layers: &LayerStack) {
    let alive: HashSet<u64> = layers.layers.iter().map(|l| l.id).collect();
    p.thumbs.retain(|id, _| alive.contains(id));

    for layer in &layers.layers {
        let stale = p.stale_thumbs.remove(&layer.id);
        if p.thumbs.contains_key(&layer.id) && !stale { continue; }

        let (w, h) = (layers.width as usize, layers.height as usize);
        let k = (64.0 / w.max(h) as f32).min(1.0);
        let tw = ((w as f32 * k).round() as usize).max(1);
        let th = ((h as f32 * k).round() as usize).max(1);
        let mut rgba = Vec::with_capacity(tw * th * 4);
        for y in 0..th {
            let sy = (y * h) / th;
            for x in 0..tw {
                let sx = (x * w) / tw;
                let i = (sy * w + sx) * 4;
                rgba.extend_from_slice(&layer.pixels[i..i + 4]);
            }
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba);
        match p.thumbs.get_mut(&layer.id) {
            Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
            None => {
                let tex = ctx.load_texture(format!("layer-thumb-{}", layer.id), image, egui::TextureOptions::LINEAR);
                p.thumbs.insert(layer.id, tex);
            }
        }
    }
}

// --- Canvas -------------------------------------------------------------------

fn canvas_tab(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    ui.label(RichText::new(format!("Current: {} × {} px", m.layers.width, m.layers.height)).color(theme::TEXT_WEAK));
    ui.add_space(6.0);

    egui::Grid::new("canvas_size").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        ui.label("Width");
        ui.add(egui::DragValue::new(&mut m.canvas_edit.0).range(1..=m.max_canvas).suffix(" px"));
        ui.end_row();
        ui.label("Height");
        ui.add(egui::DragValue::new(&mut m.canvas_edit.1).range(1..=m.max_canvas).suffix(" px"));
        ui.end_row();
    });

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (name, w, h) in [("Square", 2048, 2048), ("A4 300 dpi", 2480, 3508), ("Full HD", 1920, 1080), ("4K", 3840, 2160)] {
            if ui.small_button(name).on_hover_text(format!("{w} × {h}")).clicked() {
                *m.canvas_edit = (w.min(m.max_canvas), h.min(m.max_canvas));
            }
        }
    });

    // RAM: 4 bytes/px per layer, plus composite (4), stroke snapshot (4)
    // and stroke mask (1).
    let px = m.canvas_edit.0 as f64 * m.canvas_edit.1 as f64;
    let n = m.layers.layers.len();
    let mb = px * (4.0 * n as f64 + 9.0) / (1024.0 * 1024.0);
    ui.add_space(6.0);
    ui.label(RichText::new(format!("≈ {mb:.0} MB RAM with {n} layer(s) · content stays centered"))
        .small().color(theme::TEXT_WEAK));
    ui.add_space(6.0);

    let unchanged = *m.canvas_edit == (m.layers.width, m.layers.height);
    let button = egui::Button::new(RichText::new("Resize canvas").color(Color32::WHITE)).fill(theme::ACCENT_DIM);
    if ui.add_enabled(!unchanged, button).clicked() {
        a.resize_to = Some(*m.canvas_edit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("0a0B0c"), Some([10, 11, 12]));
        assert_eq!(parse_hex("#FFF"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
    }
}
