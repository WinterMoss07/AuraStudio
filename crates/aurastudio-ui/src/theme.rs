//! AuraStudio dark theme: neutral graphite grays (no blue cast), one soft
//! gray-cyan accent, rounded corners, compact spacing. Minimal on purpose:
//! the UI stays quiet and leaves the room to the painting.

use egui::{Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Visuals};

/// Behind the canvas (also used as the GPU clear color).
pub const BACKDROP:     Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1f);
/// Docks, toolbar, menu bar.
pub const PANEL:        Color32 = Color32::from_rgb(0x2c, 0x2c, 0x2e);
/// Inset areas inside a dock (layer list, tab bars).
pub const SURFACE:      Color32 = Color32::from_rgb(0x25, 0x25, 0x27);
pub const WIDGET:       Color32 = Color32::from_rgb(0x39, 0x39, 0x3c);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x44, 0x44, 0x48);
pub const BORDER:       Color32 = Color32::from_rgb(0x3b, 0x3b, 0x3e);
/// Selection accent: a gray-leaning cyan blue.
pub const ACCENT:       Color32 = Color32::from_rgb(0x6c, 0xa4, 0xb6);
pub const ACCENT_DIM:   Color32 = Color32::from_rgb(0x3c, 0x56, 0x60);
pub const TEXT:         Color32 = Color32::from_rgb(0xe2, 0xe2, 0xe3);
pub const TEXT_WEAK:    Color32 = Color32::from_rgb(0x98, 0x98, 0x9c);

/// BACKDROP as a 0..1 float color, for the GPU clear.
pub fn backdrop_rgb() -> [f64; 3] {
    [BACKDROP.r() as f64 / 255.0, BACKDROP.g() as f64 / 255.0, BACKDROP.b() as f64 / 255.0]
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Small,     FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Body,      FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button,    FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Heading,   FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ].into();

    let s = &mut style.spacing;
    s.item_spacing    = egui::vec2(6.0, 5.0);
    s.button_padding  = egui::vec2(7.0, 3.0);
    s.interact_size.y = 22.0;
    s.slider_width    = 140.0;
    s.window_margin   = Margin::same(8.0);
    s.menu_margin     = Margin::same(6.0);

    let mut v = Visuals::dark();
    v.panel_fill        = PANEL;
    v.window_fill       = PANEL;
    v.extreme_bg_color  = SURFACE;
    v.faint_bg_color    = WIDGET;
    v.window_stroke     = Stroke::new(1.0_f32, BORDER);
    v.window_rounding   = Rounding::same(10.0);
    v.menu_rounding     = Rounding::same(8.0);
    v.window_shadow.color = Color32::from_black_alpha(90);
    v.popup_shadow.color  = Color32::from_black_alpha(90);
    v.selection.bg_fill   = ACCENT_DIM;
    v.selection.stroke    = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color     = ACCENT;
    v.override_text_color = Some(TEXT);
    v.slider_trailing_fill = true;

    let r = Rounding::same(6.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill   = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_WEAK);
    w.noninteractive.rounding  = r;
    w.inactive.bg_fill      = WIDGET;
    w.inactive.weak_bg_fill = WIDGET;
    w.inactive.bg_stroke    = Stroke::NONE;
    w.inactive.fg_stroke    = Stroke::new(1.0_f32, TEXT);
    w.inactive.rounding     = r;
    w.hovered.bg_fill       = WIDGET_HOVER;
    w.hovered.weak_bg_fill  = WIDGET_HOVER;
    w.hovered.bg_stroke     = Stroke::new(1.0_f32, BORDER);
    w.hovered.fg_stroke     = Stroke::new(1.5_f32, TEXT);
    w.hovered.rounding      = r;
    w.active.bg_fill        = ACCENT_DIM;
    w.active.weak_bg_fill   = ACCENT_DIM;
    w.active.bg_stroke      = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke      = Stroke::new(2.0_f32, TEXT);
    w.active.rounding       = r;
    w.open.bg_fill          = WIDGET_HOVER;
    w.open.rounding         = r;

    style.visuals = v;
    ctx.set_style(style);
}

/// egui_dock look matching the theme: flat tab bars, the active tab lifted
/// to the panel color, thin separators that light up when dragged.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut d = egui_dock::Style::from_egui(style);
    d.main_surface_border_stroke = Stroke::NONE;
    d.tab_bar.bg_fill = BACKDROP;
    d.tab_bar.height  = 28.0;
    d.tab_bar.hline_color = BORDER;
    d.separator.width = 2.0;
    d.separator.color_idle    = BACKDROP;
    d.separator.color_hovered = ACCENT_DIM;
    d.separator.color_dragged = ACCENT;
    d.tab.tab_body.bg_fill = PANEL;
    d.tab.tab_body.stroke  = Stroke::NONE;
    d.tab.tab_body.inner_margin = Margin::same(8.0);
    d.tab.active.bg_fill    = PANEL;
    d.tab.active.text_color = TEXT;
    d.tab.active.outline_color = PANEL;
    d.tab.focused.bg_fill    = PANEL;
    d.tab.focused.text_color = TEXT;
    d.tab.focused.outline_color = PANEL;
    d.tab.inactive.bg_fill    = BACKDROP;
    d.tab.inactive.text_color = TEXT_WEAK;
    d.tab.inactive.outline_color = BACKDROP;
    d.tab.hovered.bg_fill    = WIDGET;
    d.tab.hovered.text_color = TEXT;
    d.tab.hovered.outline_color = WIDGET;
    d.overlay.selection_color = ACCENT.linear_multiply(0.35);
    d
}
