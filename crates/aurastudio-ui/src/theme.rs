//! AuraStudio dark theme: neutral grays (R = G = B, give or take one
//! level), one saturated blue for everything selected or filled (sliders,
//! selected tab, layer and brush). Menu bar, tool options bar and docks are
//! one seamless surface; only the canvas is a rounded card set into it.

use egui::{Color32, FontFamily, FontId, Margin, Rounding, Stroke, TextStyle, Visuals};

/// The UI surface: menu bar, tool options bar, docks, and the thin frame
/// around the canvas.
pub const PANEL:        Color32 = Color32::from_rgb(0x1f, 0x1f, 0x1f);
/// Same surface (kept as a name for "behind everything").
pub const BACKDROP:     Color32 = PANEL;
/// Around the image, inside the canvas card (GPU clear color).
pub const DESK:         Color32 = Color32::from_rgb(0x18, 0x18, 0x18);
/// Inset areas inside a card (layer list).
pub const SURFACE:      Color32 = Color32::from_rgb(0x22, 0x22, 0x23);
/// Widgets and list rows.
pub const WIDGET:       Color32 = Color32::from_rgb(0x2e, 0x2e, 0x2f);
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x39, 0x39, 0x3b);
pub const BORDER:       Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
/// Selection and slider fill: #0070E7 with 10 % less saturation (HSB).
pub const ACCENT:       Color32 = Color32::from_rgb(0x17, 0x7c, 0xe7);
/// The accent toned down (hovered separators, drop zones).
pub const ACCENT_DIM:   Color32 = Color32::from_rgb(0x1f, 0x4a, 0x7a);
pub const TEXT:         Color32 = Color32::from_rgb(0xf2, 0xf2, 0xf2);
pub const TEXT_WEAK:    Color32 = Color32::from_rgb(0xaf, 0xaf, 0xb3);

/// Corner radius of the canvas.
pub const CANVAS_ROUNDING: f32 = 10.0;
/// Frame of UI surface left of and below the canvas (where nothing is
/// docked yet).
pub const CANVAS_FRAME: f32 = 3.0;

/// DESK as a 0..1 float color, for the GPU clear (the UI paints the
/// backdrop around the canvas card itself).
pub fn backdrop_rgb() -> [f64; 3] {
    [DESK.r() as f64 / 255.0, DESK.g() as f64 / 255.0, DESK.b() as f64 / 255.0]
}

pub fn apply(ctx: &egui::Context) {
    // SVG icons (toolbox).
    egui_extras::install_image_loaders(ctx);
    // Ctrl + / - / 0 belong to the canvas, not to egui's own UI zoom.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
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
    v.selection.bg_fill   = ACCENT;
    v.selection.stroke    = Stroke::new(1.0_f32, TEXT);
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
    w.hovered.bg_stroke     = Stroke::NONE;
    w.hovered.fg_stroke     = Stroke::new(1.5_f32, TEXT);
    w.hovered.rounding      = r;
    w.active.bg_fill        = ACCENT;
    w.active.weak_bg_fill   = ACCENT;
    w.active.bg_stroke      = Stroke::NONE;
    w.active.fg_stroke      = Stroke::new(2.0_f32, TEXT);
    w.active.rounding       = r;
    w.open.bg_fill          = WIDGET_HOVER;
    w.open.rounding         = r;

    style.visuals = v;
    ctx.set_style(style);
}

/// egui_dock look matching the theme: docks are seamless (tab bar, body
/// and separators all the surface color, no borders); the selected tab is
/// a lighter pill, and a separator lights up only when hovered.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let mut d = egui_dock::Style::from_egui(style);
    d.main_surface_border_stroke = Stroke::NONE;
    d.tab_bar.bg_fill = PANEL;
    d.tab_bar.height  = 30.0;
    d.tab_bar.hline_color = PANEL;
    d.tab_bar.rounding = Rounding::ZERO;
    d.separator.width = 2.0;
    // egui_dock's own limit (175 pt per side) is replaced by each
    // dock's minimum height, see dock_layout.
    d.separator.extra = 8.0;
    d.separator.color_idle    = BACKDROP;
    d.separator.color_hovered = ACCENT_DIM;
    d.separator.color_dragged = ACCENT;
    d.tab.tab_body.bg_fill = PANEL;
    d.tab.tab_body.stroke  = Stroke::NONE;
    d.tab.tab_body.rounding = Rounding::ZERO;
    d.tab.tab_body.inner_margin = Margin::same(8.0);
    d.tab.hline_below_active_tab_name = false;
    let pill = Rounding::same(6.0);
    for (t, bg, text) in [
        (&mut d.tab.active, WIDGET_HOVER, TEXT),
        (&mut d.tab.focused, WIDGET_HOVER, TEXT),
        (&mut d.tab.active_with_kb_focus, WIDGET_HOVER, TEXT),
        (&mut d.tab.focused_with_kb_focus, WIDGET_HOVER, TEXT),
        (&mut d.tab.inactive, PANEL, TEXT_WEAK.gamma_multiply(0.8)),
        (&mut d.tab.inactive_with_kb_focus, PANEL, TEXT_WEAK.gamma_multiply(0.8)),
        (&mut d.tab.hovered, WIDGET, TEXT),
    ] {
        t.bg_fill = bg;
        t.text_color = text;
        t.outline_color = bg;
        t.rounding = pill;
    }
    d.overlay.selection_color = ACCENT.linear_multiply(0.35);
    d
}
