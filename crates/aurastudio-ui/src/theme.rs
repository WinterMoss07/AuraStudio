//! AuraStudio dark theme: calm charcoal surfaces, one soft blue accent,
//! rounded corners, compact spacing. Between Procreate (minimal, the
//! canvas first) and Clip Studio Paint (dense, dockable panels).

use egui::{Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Visuals};

/// Behind the canvas (also used as the GPU clear color).
pub const BACKDROP:     Color32 = Color32::from_rgb(0x16, 0x17, 0x1a);
/// Docks, tool strip, top bar.
pub const PANEL:        Color32 = Color32::from_rgb(0x1e, 0x1f, 0x23);
/// Inset areas inside a dock (layer list, tab bars).
pub const SURFACE:      Color32 = Color32::from_rgb(0x19, 0x1a, 0x1e);
pub const WIDGET:       Color32 = Color32::from_rgb(0x2a, 0x2c, 0x32);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x34, 0x37, 0x3f);
pub const BORDER:       Color32 = Color32::from_rgb(0x2c, 0x2e, 0x34);
pub const ACCENT:       Color32 = Color32::from_rgb(0x6e, 0x8e, 0xff);
pub const ACCENT_DIM:   Color32 = Color32::from_rgb(0x2b, 0x35, 0x5c);
pub const TEXT:         Color32 = Color32::from_rgb(0xdc, 0xdd, 0xe2);
pub const TEXT_WEAK:    Color32 = Color32::from_rgb(0x8b, 0x8e, 0x98);

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
    s.item_spacing    = egui::vec2(6.0, 6.0);
    s.button_padding  = egui::vec2(8.0, 4.0);
    s.interact_size.y = 22.0;
    s.slider_width    = 140.0;
    s.window_margin   = Margin::same(10.0);
    s.menu_margin     = Margin::same(6.0);

    let mut v = Visuals::dark();
    v.panel_fill        = PANEL;
    v.window_fill       = PANEL;
    v.extreme_bg_color  = SURFACE;
    v.faint_bg_color    = WIDGET;
    v.window_stroke     = Stroke::new(1.0_f32, BORDER);
    v.window_rounding = Rounding::same(10.0);
    v.menu_rounding   = Rounding::same(8.0);
    v.window_shadow.color  = Color32::from_black_alpha(90);
    v.popup_shadow.color   = Color32::from_black_alpha(90);
    v.selection.bg_fill    = ACCENT_DIM;
    v.selection.stroke     = Stroke::new(1.0_f32, ACCENT);
    v.hyperlink_color      = ACCENT;
    v.override_text_color  = Some(TEXT);

    let r = Rounding::same(6.0);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill   = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_WEAK);
    w.noninteractive.rounding = r;
    w.inactive.bg_fill      = WIDGET;
    w.inactive.weak_bg_fill = WIDGET;
    w.inactive.bg_stroke    = Stroke::NONE;
    w.inactive.fg_stroke    = Stroke::new(1.0_f32, TEXT);
    w.inactive.rounding = r;
    w.hovered.bg_fill       = WIDGET_HOVER;
    w.hovered.weak_bg_fill  = WIDGET_HOVER;
    w.hovered.bg_stroke     = Stroke::new(1.0_f32, BORDER);
    w.hovered.fg_stroke     = Stroke::new(1.5_f32, TEXT);
    w.hovered.rounding = r;
    w.active.bg_fill        = ACCENT_DIM;
    w.active.weak_bg_fill   = ACCENT_DIM;
    w.active.bg_stroke      = Stroke::new(1.0_f32, ACCENT);
    w.active.fg_stroke      = Stroke::new(2.0_f32, TEXT);
    w.active.rounding  = r;
    w.open.bg_fill          = WIDGET_HOVER;
    w.open.rounding    = r;

    style.visuals = v;
    ctx.set_style(style);
}

/// egui_dock look matching the theme: flat tab bars, the active tab lifted
/// to the panel color, thin separators that light up when dragged.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut d = egui_dock::Style::from_egui(style);
    d.main_surface_border_stroke = Stroke::NONE;
    d.tab_bar.bg_fill = BACKDROP;
    d.tab_bar.height  = 30.0;
    d.tab_bar.hline_color = BORDER;
    d.separator.width = 2.0;
    d.separator.color_idle    = BACKDROP;
    d.separator.color_hovered = ACCENT_DIM;
    d.separator.color_dragged = ACCENT;
    d.tab.tab_body.bg_fill = PANEL;
    d.tab.tab_body.stroke  = Stroke::NONE;
    d.tab.tab_body.inner_margin = Margin::same(10.0);
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
