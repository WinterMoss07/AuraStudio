//! AuraStudio dark theme: strictly neutral grays (R = G = B, so no gray is
//! warmer or cooler than another), one soft
//! gray-cyan accent, rounded corners, compact spacing. Minimal on purpose:
//! the UI stays quiet and leaves the room to the painting.

use egui::{Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Visuals};

/// Behind the canvas (also used as the GPU clear color).
pub const BACKDROP:     Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
/// Docks, toolbar, menu bar.
pub const PANEL:        Color32 = Color32::from_rgb(0x2d, 0x2d, 0x2d);
/// Inset areas inside a dock (layer list, tab bars).
pub const SURFACE:      Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
pub const WIDGET:       Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x46, 0x46, 0x46);
pub const BORDER:       Color32 = Color32::from_rgb(0x3c, 0x3c, 0x3c);
/// Selection accent: a soft, slightly grayed blue (halfway between the
/// first blue and a cyan).
pub const ACCENT:       Color32 = Color32::from_rgb(0x6d, 0x99, 0xda);
pub const ACCENT_DIM:   Color32 = Color32::from_rgb(0x33, 0x45, 0x5e);
pub const TEXT:         Color32 = Color32::from_rgb(0xe2, 0xe2, 0xe2);
pub const TEXT_WEAK:    Color32 = Color32::from_rgb(0x9a, 0x9a, 0x9a);

/// BACKDROP as a 0..1 float color, for the GPU clear.
pub fn backdrop_rgb() -> [f64; 3] {
    [BACKDROP.r() as f64 / 255.0, BACKDROP.g() as f64 / 255.0, BACKDROP.b() as f64 / 255.0]
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    // egui waits for the pointer to be still before showing a tooltip, and
    // asks for a new frame right away on every frame until then. A pen
    // hovering over the tablet is never still (it jitters by a pixel): over
    // any widget with a tooltip, that was hundreds of frames per second
    // (Immediate present mode) and the idle CPU use. Tooltips now show
    // after their usual delay, from a timer.
    style.interaction.show_tooltips_only_when_still = false;

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
    // egui_dock's own limit (175 pt per side) is replaced by each
    // dock's minimum height, see dock_layout.
    d.separator.extra = 8.0;
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
