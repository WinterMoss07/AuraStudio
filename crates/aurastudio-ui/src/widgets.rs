//! Shared widgets: color channel sliders, fill sliders (size, opacity...),
//! icon buttons, elided text.

pub use crate::icons::Icon;
use egui::{epaint, pos2, vec2, Color32, Pos2, Rect, Sense, Shape, Stroke, Ui};

use std::sync::{Arc, RwLock};

use crate::theme;

// --- Color math ----------------------------------------------------------
//
// HSV computed on sRGB-encoded values, like Krita, CSP and Photoshop show
// it (egui's own Hsva works on linear values: mid gray would read V=21%).

pub fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    [h, s, max]
}

pub fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h6 % 2.0) - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// Color shown on screen for an sRGB color of the image: goes through the
/// display transform (monitor profile) when color management is on, so
/// the color docker shows exactly what the canvas will show.
pub fn rgb_to_color32([r, g, b]: [f32; 3]) -> Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    display_color([q(r), q(g), q(b)])
}

/// sRGB -> monitor conversion for image colors (None = no conversion).
type DisplayFn = Arc<dyn Fn([u8; 3]) -> [u8; 3] + Send + Sync>;
static DISPLAY: RwLock<Option<DisplayFn>> = RwLock::new(None);

/// Set by the app from the monitor profile (aurastudio-color).
pub fn set_display_transform(f: Option<DisplayFn>) {
    *DISPLAY.write().unwrap() = f;
}

/// An image color (sRGB) as it must be drawn on this monitor.
pub fn display_color(rgb: [u8; 3]) -> Color32 {
    let [r, g, b] = match DISPLAY.read().unwrap().as_ref() {
        Some(f) => f(rgb),
        None => rgb,
    };
    Color32::from_rgb(r, g, b)
}

pub fn hsv_to_color32(hsv: [f32; 3]) -> Color32 {
    rgb_to_color32(hsv_to_rgb(hsv))
}

// --- Drawing helpers -----------------------------------------------------

/// Light / dark checkerboard showing transparency (layer thumbnails,
/// opacity sliders).
pub fn checker(painter: &egui::Painter, rect: Rect, cell: f32) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(0x9a));
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 { continue; }
            let min = rect.min + vec2(x as f32 * cell, y as f32 * cell);
            let r = Rect::from_min_max(min, (min + vec2(cell, cell)).min(rect.max));
            painter.rect_filled(r, 0.0, Color32::from_gray(0x6a));
        }
    }
}

/// Horizontal gradient, sampled `steps` times through `color_at(t)`.
fn paint_gradient(painter: &egui::Painter, rect: Rect, color_at: &dyn Fn(f32) -> Color32) {
    const STEPS: usize = 48;
    let mut mesh = epaint::Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let x = rect.left() + t * rect.width();
        let c = color_at(t);
        mesh.colored_vertex(pos2(x, rect.top()), c);
        mesh.colored_vertex(pos2(x, rect.bottom()), c);
        if i > 0 {
            let k = (i * 2) as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(Shape::mesh(mesh));
}

// --- Gradient channel slider ----------------------------------------------

/// Snap a 0..1 slider position to the step its number box shows (an S of
/// 0.4 % shown as 0 gave tinted "grays").
pub fn snap(t: f32, display_max: f32) -> f32 {
    (t * display_max).round() / display_max
}

/// One color channel: label, gradient bar, marker, value (0..1 shown as
/// 0..`display_max`). True when changed.
pub fn channel_row(
    ui: &mut Ui,
    width: f32,
    label: &str,
    value: &mut f32,
    display_max: f32,
    color_at: &dyn Fn(f32) -> Color32,
) -> bool {
    // The row is allocated whole and split by hand, `width` measured once
    // for all rows: egui sizing the box made the bars end at different x.
    let gap = 7.0;
    let (label_w, box_w, h, bar_h) = (12.0, 38.0, 24.0, 14.0);
    let (row, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    // Label and number box are centered on the bar, not on the row (the
    // marker triangle hangs under the bar).
    let bar_cy = row.top() + bar_h / 2.0;
    let label_rect = Rect::from_center_size(pos2(row.left() + label_w / 2.0, bar_cy), vec2(label_w, 16.0));
    let box_rect = Rect::from_center_size(pos2(row.right() - box_w / 2.0, bar_cy), vec2(box_w, 20.0));
    let rect = Rect::from_min_max(pos2(label_rect.right() + gap, row.top()), pos2(box_rect.left() - gap, row.bottom()));
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), bar_h));

    ui.painter().text(label_rect.center(), egui::Align2::CENTER_CENTER, label,
        egui::FontId::proportional(12.0), theme::TEXT_WEAK);

    let mut changed = false;
    let resp = ui.interact(rect, ui.id().with(("channel", label)), Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos() {
        let t = snap(((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0), display_max);
        if (t - *value).abs() > f32::EPSILON {
            *value = t;
            changed = true;
        }
    }

    let painter = ui.painter_at(rect.expand(2.0));
    paint_gradient(&painter, bar, color_at);
    painter.rect_stroke(bar, 2.0, Stroke::new(1.0_f32, theme::BORDER));

    // Marker: a thin line on the bar plus a small triangle under it.
    let x = bar.left() + *value * bar.width();
    painter.line_segment([pos2(x, bar.top()), pos2(x, bar.bottom())], Stroke::new(2.0_f32, Color32::WHITE));
    painter.line_segment([pos2(x, bar.top()), pos2(x, bar.bottom())], Stroke::new(0.8_f32, Color32::BLACK));
    let tip = pos2(x, bar.bottom() + 1.0);
    let col = if resp.hovered() || resp.dragged() { theme::TEXT } else { theme::TEXT_WEAK };
    painter.add(Shape::convex_polygon(
        vec![tip, pos2(x + 4.5, tip.y + 6.0), pos2(x - 4.5, tip.y + 6.0)],
        col,
        Stroke::NONE,
    ));

    let mut shown = (*value * display_max).round() as i32;
    let dv = egui::DragValue::new(&mut shown).range(0..=display_max as i32).speed(1.0);
    // egui's default minimum widget width (40) is wider than the box.
    let min_w = std::mem::replace(&mut ui.spacing_mut().interact_size.x, box_w);
    let changed_box = ui.put(box_rect, dv).changed();
    ui.spacing_mut().interact_size.x = min_w;
    if changed_box {
        *value = shown as f32 / display_max;
        changed = true;
    }
    changed
}

// --- Text ------------------------------------------------------------------

/// One line of text from `left_center`, cut with "…" to fit `max_width`.
pub fn elided_text(p: &egui::Painter, left_center: Pos2, text: &str, font: egui::FontId, color: Color32, max_width: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let pos = pos2(left_center.x, left_center.y - galley.size().y / 2.0);
    p.galley(pos, galley, color);
}

/// What is being dragged, written next to the pointer.
pub fn drag_ghost(ctx: &egui::Context, text: &str) {
    // The hand closes on what it carries (as on the canvas when panning).
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let Some(p) = ctx.pointer_interact_pos() else { return };
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag_ghost")));
    let galley = painter.layout_no_wrap(text.to_owned(), egui::FontId::proportional(12.0), theme::TEXT);
    let r = Rect::from_min_size(p + vec2(14.0, 6.0), galley.size() + vec2(12.0, 6.0));
    painter.rect_filled(r, 2.0, theme::ACCENT_DIM);
    painter.galley(r.min + vec2(6.0, 3.0), galley, theme::TEXT);
}

/// Inside a scroll area's content: while something is dragged near (or
/// past) its top or bottom edge, scroll that way, faster the farther out
/// (Krita, CSP), to drop it on a row out of sight.
pub fn drag_autoscroll(ui: &Ui) {
    let ctx = ui.ctx();
    if !egui::DragAndDrop::has_any_payload(ctx) { return; }
    let Some(p) = ctx.pointer_latest_pos() else { return };
    let r = ui.clip_rect();
    if p.x < r.left() - 60.0 || p.x > r.right() + 60.0 { return; }
    const EDGE: f32 = 36.0;
    let dt = ctx.input(|i| i.stable_dt).clamp(0.001, 0.05);
    // Points per second: up to ~1500 well past the edge.
    let speed = |d: f32| (d / EDGE).clamp(0.0, 3.0) * 500.0 * dt;
    let dy = if p.y < r.top() + EDGE {
        speed(r.top() + EDGE - p.y)
    } else if p.y > r.bottom() - EDGE {
        -speed(p.y - (r.bottom() - EDGE))
    } else {
        return;
    };
    ui.scroll_with_delta_animation(vec2(0.0, dy), egui::style::ScrollAnimation::none());
    ctx.request_repaint();
}

// --- Icons ---------------------------------------------------------------------

/// Frameless toolbar button with an icon.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(tooltip);
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 2.5, theme::WIDGET_HOVER);
    }
    let col = if enabled { theme::TEXT } else { theme::TEXT_WEAK.gamma_multiply(0.45) };
    crate::icons::paint(ui, icon, rect, 14.0, col);
    enabled && resp.clicked()
}

/// An icon button that stays lit while `on` (a layer mark, a filter).
/// Returns its response (clicked, or a popup under it).
pub fn icon_toggle(ui: &mut Ui, icon: Icon, tooltip: &str, enabled: bool, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let resp = resp.on_hover_text(tooltip);
    if on {
        ui.painter().rect_filled(rect, 2.5, theme::ACCENT_DIM);
    } else if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, 2.5, theme::WIDGET_HOVER);
    }
    let col = if !enabled { theme::TEXT_WEAK.gamma_multiply(0.45) } else if on { theme::TEXT } else { Color32::from_gray(0xc8) };
    crate::icons::paint(ui, icon, rect, 14.0, col);
    resp
}

// --- Brush controls (options bar + Tool options dock) ---------------------

/// What a fill slider shows and edits.
pub struct Fill<'a> {
    pub label:  &'a str,
    pub range:  std::ops::RangeInclusive<f32>,
    /// The value at the middle of the bar (brush size: 100 of 1..1000);
    /// None: linear.
    pub mid:    Option<f32>,
    pub format: &'a dyn Fn(f32) -> String,
}

/// Krita-style slider: a bar filled up to the value, label and value
/// inside. Click or drag sets it, right-click types one in. Returns the
/// new value when changed.
pub fn fill_slider(ui: &mut Ui, id_salt: &str, spec: Fill, value: f32, width: f32) -> Option<f32> {
    let Fill { label, range, mid, format } = spec;
    let (lo, hi) = (*range.start(), *range.end());
    let to_t = |v: f32| match mid { Some(m) => size_curve::to_t(v, lo, m, hi), None => (v - lo) / (hi - lo) };
    let from_t = |t: f32| match mid { Some(m) => size_curve::from_t(t, lo, m, hi), None => lo + t * (hi - lo) };

    let id = ui.id().with(("fill_slider", id_salt));
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let editing_id = id.with("editing");
    // (text typed so far, focus already given)
    let editing: Option<(String, bool)> = ui.data(|d| d.get_temp(editing_id));

    if let Some((mut text, focused)) = editing {
        ui.painter().rect(rect, 2.5, theme::WIDGET, Stroke::new(1.0_f32, theme::ACCENT));
        let edit = ui.put(rect, egui::TextEdit::singleline(&mut text)
            .frame(false)
            .hint_text(format(value))
            .horizontal_align(egui::Align::Center)
            .vertical_align(egui::Align::Center));
        // Focus once: asked every frame, it came straight back after a
        // click elsewhere and only Escape could close the field.
        if !focused { edit.request_focus(); }
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let clicked_away = ui.input(|i| i.pointer.any_pressed()) && !edit.hovered();
        let left = focused && !edit.has_focus();
        if left || clicked_away || enter || escape {
            ui.data_mut(|d| d.remove::<(String, bool)>(editing_id));
            if escape || clicked_away && text.is_empty() { return None; }
            let digits: String = text.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
            return digits.parse::<f32>().ok().map(|v| v.clamp(lo, hi));
        }
        ui.data_mut(|d| d.insert_temp(editing_id, (text, true)));
        return None;
    }

    if resp.secondary_clicked() {
        // Right-click types a value (left click and drag only ever set
        // it): empty, the current value as a hint, typing replaces it.
        ui.data_mut(|d| d.insert_temp(editing_id, (String::new(), false)));
    }

    let mut new = None;
    // (clicked: press and release can land in the same frame.)
    // Left button only: the right one opens the text field.
    let primary_down = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
    if primary_down || resp.dragged_by(egui::PointerButton::Primary) || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            let v = from_t(t).round().clamp(lo, hi);
            if (v - value).abs() >= 0.5 { new = Some(v); }
        }
    }

    let shown = new.unwrap_or(value);
    let t = to_t(shown).clamp(0.0, 1.0);
    let painter = ui.painter_at(rect);
    let r = 2.5;
    let bg = if resp.hovered() || resp.dragged() { theme::WIDGET_HOVER } else { theme::WIDGET };
    painter.rect_filled(rect, r, bg);
    if t > 0.0 {
        // The fill keeps the bar's rounded left end; its right end is
        // square while the bar isn't full.
        let fill = Rect::from_min_max(rect.min, pos2(rect.left() + (rect.width() * t).max(2.0 * r), rect.bottom()));
        let right = if t >= 0.999 { r } else { 0.0 };
        painter.rect_filled(fill, egui::Rounding { nw: r, sw: r, ne: right, se: right }, theme::ACCENT);
    }
    let font = egui::FontId::proportional(11.5);
    let value = painter.layout_no_wrap(format(shown), font.clone(), theme::TEXT);
    let name = painter.layout_no_wrap(label.to_owned(), font, Color32::from_white_alpha(215));
    // Too narrow for both: the value alone (the label shows on hover).
    let both = name.size().x + value.size().x + 7.0 * 2.0 + 8.0 <= rect.width();
    let y = rect.center().y;
    if both {
        painter.galley(pos2(rect.left() + 7.0, y - name.size().y / 2.0), name, Color32::WHITE);
        painter.galley(pos2(rect.right() - 7.0 - value.size().x, y - value.size().y / 2.0), value, Color32::WHITE);
    } else {
        painter.galley(pos2(rect.center().x - value.size().x / 2.0, y - value.size().y / 2.0), value, Color32::WHITE);
        resp.on_hover_text(label);
    }
    new
}

/// A 0..100 % slider.
pub fn percent(label: &str) -> Fill<'_> {
    Fill { label, range: 0.0..=100.0, mid: None, format: &|v| format!("{v:.0} %") }
}

/// Slider position -> value for a slider with `mid` at its middle:
/// `lo × (hi / lo)^(t^k)`, k chosen so t = 0.5 gives `mid`. Exponential
/// (each step a ratio, precise for small brushes), smooth all along.
mod size_curve {
    fn k(lo: f32, mid: f32, hi: f32) -> f32 {
        ((mid / lo).ln() / (hi / lo).ln()).ln() / 0.5f32.ln()
    }
    pub fn from_t(t: f32, lo: f32, mid: f32, hi: f32) -> f32 {
        lo * (hi / lo).powf(t.clamp(0.0, 1.0).powf(k(lo, mid, hi)))
    }
    pub fn to_t(v: f32, lo: f32, mid: f32, hi: f32) -> f32 {
        ((v.clamp(lo, hi) / lo).ln() / (hi / lo).ln()).powf(1.0 / k(lo, mid, hi))
    }
}

/// Brush size as a diameter: 1..1000 px, 100 at the middle.
pub fn size_slider(ui: &mut Ui, id: &str, brush: &mut aurastudio_brush::Brush, width: f32) {
    let max = crate::MAX_BRUSH_SIZE as f32;
    let d = (brush.radius * 2.0).round().clamp(1.0, max);
    if let Some(v) = fill_slider(ui, id, Fill { label: "Size", range: 1.0..=max, mid: Some(100.0), format: &|v| format!("{v:.0} px") }, d, width) {
        // Whole diameters, odd ones included (1, 3, 5 px...).
        brush.radius = v.round().max(1.0) / 2.0;
    }
}

pub fn opacity_slider(ui: &mut Ui, id: &str, brush: &mut aurastudio_brush::Brush, width: f32) {
    let o = (brush.opacity * 100.0).round();
    if let Some(v) = fill_slider(ui, id, percent("Opacity"), o, width) {
        brush.opacity = v / 100.0;
    }
}

pub fn hardness_slider(ui: &mut Ui, id: &str, brush: &mut aurastudio_brush::Brush, width: f32) {
    let h = (brush.hardness * 100.0).round();
    if let Some(v) = fill_slider(ui, id, percent("Hardness"), h, width) {
        brush.hardness = v / 100.0;
    }
}

pub fn blend_combo(ui: &mut Ui, id: &str, mode: &mut crate::BlendMode, width: f32) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(mode.name())
        .width(width)
        .show_ui(ui, |ui| {
            for m in crate::BlendMode::ALL {
                changed |= ui.selectable_value(mode, m, m.name()).changed();
            }
        });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_zero_saturation_is_a_true_gray() {
        // Pointer lands at S = 0.4 %: shown as 0, must give R = G = B.
        let s = snap(0.004, 100.0);
        let rgb = hsv_to_rgb([0.0, s, 0.8]);
        let q = rgb.map(|v| (v * 255.0).round() as u8);
        assert_eq!(q[0], q[1]);
        assert_eq!(q[1], q[2]);
        // Without snapping it was tinted: (204, 203, 203).
        let raw = hsv_to_rgb([0.0, 0.004, 0.8]).map(|v| (v * 255.0).round() as u8);
        assert_ne!(raw[0], raw[1]);
    }

    #[test]
    fn size_slider_has_100_px_in_the_middle() {
        let (lo, mid, hi) = (1.0, 100.0, 1000.0);
        assert!((size_curve::from_t(0.5, lo, mid, hi) - 100.0).abs() < 0.01);
        assert!((size_curve::from_t(0.0, lo, mid, hi) - 1.0).abs() < 1e-4);
        assert!((size_curve::from_t(1.0, lo, mid, hi) - 1000.0).abs() < 0.01);
        for v in [1.0, 7.0, 50.0, 100.0, 333.0, 1000.0] {
            assert!((size_curve::from_t(size_curve::to_t(v, lo, mid, hi), lo, mid, hi) - v).abs() < 0.01 * v);
        }
    }

    #[test]
    fn hsv_round_trip() {
        for &rgb in &[[1.0, 0.0, 0.0], [0.2, 0.6, 0.9], [0.5, 0.5, 0.5], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]] {
            let back = hsv_to_rgb(rgb_to_hsv(rgb));
            for c in 0..3 {
                assert!((back[c] - rgb[c]).abs() < 1e-5, "{rgb:?} -> {back:?}");
            }
        }
        // Mid gray is V = 50% here (sRGB values), as in Krita / CSP.
        assert!((rgb_to_hsv([0.5, 0.5, 0.5])[2] - 0.5).abs() < 1e-6);
    }
}
