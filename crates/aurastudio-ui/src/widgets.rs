//! Custom widgets: gradient channel sliders (Krita / CSP color sliders),
//! vertical tabs (CSP), vertical sliders (Procreate side bar).

use egui::{epaint, pos2, vec2, Color32, Pos2, Rect, Response, Sense, Shape, Stroke, Ui};

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

pub fn rgb_to_color32([r, g, b]: [f32; 3]) -> Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(q(r), q(g), q(b))
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

/// One color channel: label, gradient bar showing what the color becomes
/// across the whole range (recomputed every frame from the current color),
/// a marker under the bar, and an editable value. `value` is 0..1; the
/// number box shows it as 0..`display_max` (255, 360 or 100).
/// Returns true when the user changed the value.
pub fn channel_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    display_max: f32,
    color_at: &dyn Fn(f32) -> Color32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(vec2(14.0, 18.0), egui::Label::new(egui::RichText::new(label).color(theme::TEXT_WEAK)));

        let box_w = 46.0;
        let bar_w = (ui.available_width() - box_w - ui.spacing().item_spacing.x).max(40.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(bar_w, 22.0), Sense::click_and_drag());
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 14.0));

        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            if (t - *value).abs() > f32::EPSILON {
                *value = t;
                changed = true;
            }
        }

        let painter = ui.painter_at(rect.expand(2.0));
        paint_gradient(&painter, bar, color_at);
        painter.rect_stroke(bar, 3.0, Stroke::new(1.0_f32, theme::BORDER));

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
        if ui.add_sized(vec2(box_w, 20.0), dv).changed() {
            *value = shown as f32 / display_max;
            changed = true;
        }
    });
    changed
}

// --- Vertical tab (CSP color slider) ---------------------------------------

pub fn vertical_tab(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(22.0, 44.0), Sense::click());
    let painter = ui.painter_at(rect);
    let bg = if selected {
        theme::WIDGET
    } else if resp.hovered() {
        theme::WIDGET_HOVER
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 5.0, bg);
    if selected {
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 8.0), vec2(2.0, rect.height() - 16.0));
        painter.rect_filled(bar, 1.0, theme::ACCENT);
    }

    let color = if selected { theme::TEXT } else { theme::TEXT_WEAK };
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(11.0),
        color,
    );
    // Rotated -90°: reads bottom to top, like CSP.
    let pos = pos2(
        rect.center().x - galley.size().y / 2.0,
        rect.center().y + galley.size().x / 2.0,
    );
    painter.add(epaint::TextShape::new(pos, galley, color).with_angle(-std::f32::consts::FRAC_PI_2));
    resp
}

// --- Vertical slider (Procreate side bar) -----------------------------------

/// Tall rounded slider; `t` is 0 (bottom) .. 1 (top). Returns true if
/// changed. `label` is drawn under it (current value).
pub fn vertical_slider(ui: &mut Ui, t: &mut f32, height: f32, label: &str, tooltip: &str) -> bool {
    let width = 26.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let resp = resp.on_hover_text(tooltip);
    let mut changed = false;

    if let Some(p) = resp.interact_pointer_pos() {
        let v = (1.0 - (p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        if (v - *t).abs() > f32::EPSILON {
            *t = v;
            changed = true;
        }
    }

    let painter = ui.painter_at(rect.expand(4.0));
    let track = Rect::from_center_size(rect.center(), vec2(8.0, rect.height()));
    painter.rect_filled(track, 4.0, theme::SURFACE);
    let fill = Rect::from_min_max(pos2(track.left(), track.bottom() - *t * track.height()), track.max);
    painter.rect_filled(fill, 4.0, theme::ACCENT_DIM);

    let knob_y = track.bottom() - *t * track.height();
    let knob = Rect::from_center_size(pos2(rect.center().x, knob_y.clamp(rect.top() + 5.0, rect.bottom() - 5.0)), vec2(width, 10.0));
    let knob_col = if resp.hovered() || resp.dragged() { theme::TEXT } else { Color32::from_gray(0xb8) };
    painter.rect_filled(knob, 5.0, knob_col);

    ui.add_sized(
        vec2(width + 8.0, 14.0),
        egui::Label::new(egui::RichText::new(label).size(10.5).color(theme::TEXT_WEAK)),
    );
    changed
}

/// Small frameless icon button for toolbars.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, enabled: bool) -> bool {
    let btn = egui::Button::new(egui::RichText::new(icon).size(15.0)).frame(false).min_size(vec2(26.0, 24.0));
    ui.add_enabled(enabled, btn).on_hover_text(tooltip).clicked()
}

/// Circle filled with `color`, ringed like Procreate's color button.
pub fn color_dot(ui: &mut Ui, color: Color32, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let c: Pos2 = rect.center();
    let p = ui.painter();
    p.circle_filled(c, size / 2.0, theme::BORDER);
    p.circle_filled(c, size / 2.0 - 2.0, color);
    p.circle_stroke(c, size / 2.0 - 2.0, Stroke::new(1.0_f32, Color32::from_white_alpha(40)));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

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
