//! Color docker: HSV / RGB gradient sliders (tabs),
//! a preview swatch and a hex field.

use egui::{pos2, vec2, Rect, Sense, Stroke, Ui};

use crate::widgets::*;
use crate::{theme, Model};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode { Rgb, Hsv }

pub struct ColorState {
    mode: Mode,
    /// Master color in HSV, so hue survives when saturation or value hit 0
    /// (converting back from RGB would lose it).
    hsv:  [f32; 3],
    hex:  String,
}

impl Default for ColorState {
    fn default() -> Self {
        Self { mode: Mode::Hsv, hsv: [0.0, 0.0, 0.0], hex: String::new() }
    }
}

pub fn show(ui: &mut Ui, st: &mut ColorState, m: &mut Model) {
    let to_u8 = |rgb: [f32; 3]| rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8);
    let to_f  = |c: [u8; 3]| c.map(|v| v as f32 / 255.0);

    // The brush color may have changed elsewhere: resync the HSV master,
    // keeping the hue (and saturation) when they become undefined.
    if to_u8(hsv_to_rgb(st.hsv)) != m.brush.color {
        let [h, s, v] = rgb_to_hsv(to_f(m.brush.color));
        let h = if s > 0.0 && v > 0.0 { h } else { st.hsv[0] };
        let s = if v > 0.0 { s } else { st.hsv[1] };
        st.hsv = [h, s, v];
    }

    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        let w = ui.available_width();
        match st.mode {
            Mode::Hsv => {
                let [h, s, v] = st.hsv;
                let mut new = st.hsv;
                // Hue bar at full saturation / value, like most apps;
                // S and V bars show the real color along the range.
                channel_row(ui, w, "H", &mut new[0], 360.0, &|t| hsv_to_color32([t, 1.0, 1.0]));
                channel_row(ui, w, "S", &mut new[1], 100.0, &|t| hsv_to_color32([h, t, v]));
                channel_row(ui, w, "V", &mut new[2], 100.0, &|t| hsv_to_color32([h, s, t]));
                if new != st.hsv {
                    st.hsv = new;
                    m.brush.color = to_u8(hsv_to_rgb(new));
                }
            }
            Mode::Rgb => {
                let [r, g, b] = to_f(m.brush.color);
                let mut new = [r, g, b];
                channel_row(ui, w, "R", &mut new[0], 255.0, &|t| rgb_to_color32([t, g, b]));
                channel_row(ui, w, "G", &mut new[1], 255.0, &|t| rgb_to_color32([r, t, b]));
                channel_row(ui, w, "B", &mut new[2], 255.0, &|t| rgb_to_color32([r, g, t]));
                if new != [r, g, b] {
                    m.brush.color = to_u8(new);
                }
            }
        }
    });

    ui.add_space(6.0);
    {
        // Swatch + hex field, split by hand so the field never overflows.
        let [r, g, b] = m.brush.color;
        let hex_w = 74.0;
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let swatch = Rect::from_min_max(row.min, pos2(row.right() - hex_w - 4.0, row.bottom()));
        let field = Rect::from_min_max(pos2(row.right() - hex_w, row.top() + 1.0), pos2(row.right(), row.bottom() - 1.0));
        ui.painter().rect_filled(swatch, 3.0, display_color([r, g, b]));
        ui.painter().rect_stroke(swatch, 3.0, Stroke::new(1.0_f32, theme::BORDER), egui::StrokeKind::Middle);

        let edit = ui.put(field, egui::TextEdit::singleline(&mut st.hex).align(egui::Align2::CENTER_TOP));
        if edit.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) && edit.has_focus() {
            if let Some(c) = parse_hex(&st.hex) {
                m.brush.color = c;
            }
        }
        if !edit.has_focus() {
            st.hex = format!("#{r:02X}{g:02X}{b:02X}");
        }
    }

    // Slider set, under the color preview (CSP keeps them as tabs too).
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (mode, label) in [(Mode::Hsv, "HSV"), (Mode::Rgb, "RGB")] {
            if mode_tab(ui, label, st.mode == mode).clicked() { st.mode = mode; }
        }
    });
}

/// A small tab: accent underline when selected.
fn mode_tab(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(44.0, 22.0), Sense::click());
    let p = ui.painter();
    if selected || resp.hovered() {
        p.rect_filled(rect, 2.5, if selected { theme::WIDGET } else { theme::WIDGET_HOVER.gamma_multiply(0.6) });
    }
    let col = if selected { theme::TEXT } else { theme::TEXT_WEAK };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.0), col);
    if selected {
        let line = Rect::from_min_max(pos2(rect.left() + 12.0, rect.bottom() - 2.0), pos2(rect.right() - 12.0, rect.bottom()));
        p.rect_filled(line, 1.0, theme::ACCENT);
    }
    resp
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 { return None; }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
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
