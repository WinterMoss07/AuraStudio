//! Preferences (Edit > Preferences…, Ctrl+K): what the user can turn on
//! and off, kept in settings.txt (key=value lines; unknown or missing keys
//! keep their default), and their dialog.

use egui::{RichText, Ui};

use crate::theme;

/// The graphics API the canvas is drawn with (taken at startup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Renderer { Vulkan, DirectX12, OpenGl, Metal }

impl Renderer {
    /// Those this system may have, in order.
    pub fn offered() -> &'static [Renderer] {
        if cfg!(target_os = "macos") { &[Renderer::Metal, Renderer::OpenGl] }
        else if cfg!(windows) { &[Renderer::Vulkan, Renderer::DirectX12, Renderer::OpenGl] }
        else { &[Renderer::Vulkan, Renderer::OpenGl] }
    }

    pub fn name(self) -> &'static str {
        match self { Renderer::Vulkan => "Vulkan", Renderer::DirectX12 => "DirectX 12", Renderer::OpenGl => "OpenGL", Renderer::Metal => "Metal" }
    }

    fn key(self) -> &'static str {
        match self { Renderer::Vulkan => "vulkan", Renderer::DirectX12 => "dx12", Renderer::OpenGl => "gl", Renderer::Metal => "metal" }
    }

    fn from_key(k: &str) -> Option<Self> {
        [Renderer::Vulkan, Renderer::DirectX12, Renderer::OpenGl, Renderer::Metal].into_iter().find(|r| r.key() == k)
    }
}

impl Default for Renderer {
    fn default() -> Self { Renderer::offered()[0] }
}

/// How the canvas is drawn on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CanvasQuality {
    /// The exact average of the canvas pixels under each screen pixel.
    #[default]
    High,
    /// Mipmaps (lighter on old graphics chips, a third more memory).
    Standard,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    // Interface.
    /// Size of the interface (1.0 = the system's scaling).
    pub ui_scale:         f32,
    /// The Tools docker shows each brush's stroke (off: names only).
    pub brush_previews:   bool,
    /// The window opens maximized.
    pub open_maximized:   bool,
    // Canvas and display.
    /// 100 % zoom follows the display scaling (off: one image pixel per
    /// physical screen pixel).
    pub hidpi_scaling:    bool,
    /// The canvas is color-managed to the monitor profile.
    pub color_managed:    bool,
    /// Perceptual intent (off: relative colorimetric).
    pub perceptual:       bool,
    /// Graphics API wanted (the next start uses it; another one if it
    /// can't run).
    pub renderer:         Renderer,
    /// How the canvas is drawn.
    pub canvas_quality:   CanvasQuality,
    /// Above 100 %: canvas pixels blended instead of sharp squares.
    pub smooth_zoom_in:   bool,
    /// Frames shown in step with the screen (no tearing, a little more
    /// latency); off: as soon as drawn.
    pub vsync:            bool,
    // Brushes and input.
    /// The brush outline replaces the cursor over the canvas (off: a
    /// crosshair).
    pub brush_outline:    bool,
    /// Mouse click debounce, ms (0: off).
    pub debounce_ms:      u32,
    // Files.
    /// Unsaved work goes to the recovery file this long after it changed
    /// (0: never).
    pub autosave_minutes: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            ui_scale: 1.0, brush_previews: true, open_maximized: true,
            hidpi_scaling: true, color_managed: true, perceptual: true, renderer: Renderer::default(),
            canvas_quality: CanvasQuality::High, smooth_zoom_in: false, vsync: false,
            brush_outline: true, debounce_ms: 20,
            autosave_minutes: 5,
        }
    }
}

/// Interface sizes offered.
pub const UI_SCALES: [f32; 6] = [0.75, 0.85, 1.0, 1.1, 1.25, 1.5];
const DEBOUNCES: [u32; 7] = [0, 10, 20, 30, 40, 60, 80];
const AUTOSAVES: [u32; 7] = [0, 1, 2, 5, 10, 15, 30];

impl Prefs {
    pub fn from_text(text: &str) -> Self {
        let mut p = Self::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            let b = v != "false";
            match k.trim() {
                "ui_scale" => if let Ok(v) = v.parse::<f32>() { p.ui_scale = v.clamp(0.5, 2.0) },
                "brush_previews" => p.brush_previews = b,
                "open_maximized" => p.open_maximized = b,
                "hidpi_image_scaling" => p.hidpi_scaling = b,
                "color_managed" => p.color_managed = b,
                "perceptual" => p.perceptual = b,
                "renderer" => if let Some(r) = Renderer::from_key(v).filter(|r| Renderer::offered().contains(r)) { p.renderer = r },
                "canvas_quality" => p.canvas_quality = if v == "standard" { CanvasQuality::Standard } else { CanvasQuality::High },
                "smooth_zoom_in" => p.smooth_zoom_in = b,
                "vsync" => p.vsync = v == "true",
                "brush_outline" => p.brush_outline = b,
                "click_debounce_ms" => if let Ok(v) = v.parse::<u32>() { p.debounce_ms = v.min(200) },
                "autosave_minutes" => if let Ok(v) = v.parse::<u32>() { p.autosave_minutes = v.min(120) },
                _ => {}
            }
        }
        p
    }

    pub fn to_text(&self) -> String {
        format!(
            "ui_scale={}\nbrush_previews={}\nopen_maximized={}\nhidpi_image_scaling={}\ncolor_managed={}\nperceptual={}\nrenderer={}\ncanvas_quality={}\nsmooth_zoom_in={}\nvsync={}\nbrush_outline={}\nclick_debounce_ms={}\nautosave_minutes={}\n",
            self.ui_scale, self.brush_previews, self.open_maximized, self.hidpi_scaling, self.color_managed,
            self.perceptual, self.renderer.key(),
            if self.canvas_quality == CanvasQuality::Standard { "standard" } else { "high" }, self.smooth_zoom_in, self.vsync,
            self.brush_outline, self.debounce_ms, self.autosave_minutes,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Section { #[default] Interface, Display, Brushes, Files }

impl Section {
    const ALL: [Section; 4] = [Section::Interface, Section::Display, Section::Brushes, Section::Files];
    fn name(self) -> &'static str {
        match self {
            Section::Interface => "Interface",
            Section::Display => "Canvas & display",
            Section::Brushes => "Brushes & input",
            Section::Files => "Files",
        }
    }
}

/// The Preferences dialog. Changes apply at once. Returns false once
/// closed. `profile`: the monitor profile's name, if one was found.
pub fn dialog(ctx: &egui::Context, section: &mut Section, p: &mut Prefs, profile: Option<&str>, in_use: &str, reset_docks: &mut bool) -> bool {
    let mut open = true;
    let modal = egui::Modal::new(egui::Id::new("preferences"))
        .frame(egui::Frame::popup(&ctx.style()).inner_margin(egui::Margin::same(18.0)).rounding(theme::CANVAS_ROUNDING))
        .show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(RichText::new("Preferences").heading());
            ui.add_space(10.0);
            // A fixed height: a separator in a horizontal row takes all the
            // height it is offered, and a modal offers what it had last
            // frame, so the window grew a little more every frame.
            const H: f32 = 330.0;
            ui.horizontal_top(|ui| {
                ui.set_height(H);
                ui.vertical(|ui| {
                    ui.set_width(150.0);
                    for s in Section::ALL {
                        if ui.selectable_label(*section == s, s.name()).clicked() { *section = s; }
                    }
                });
                let (line, _) = ui.allocate_exact_size(egui::vec2(9.0, H), egui::Sense::hover());
                ui.painter().vline(line.center().x, line.y_range(), egui::Stroke::new(1.0_f32, theme::BORDER));
                ui.vertical(|ui| {
                    ui.set_height(H);
                    match section {
                        Section::Interface => interface(ui, p, reset_docks),
                        Section::Display => display(ui, p, profile, in_use),
                        Section::Brushes => brushes(ui, p),
                        Section::Files => files(ui, p),
                    }
                });
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Restore defaults").clicked() {
                    *p = Prefs::default();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new("Close").color(theme::TEXT)).fill(theme::ACCENT).min_size(egui::vec2(88.0, 26.0));
                    if ui.add(b).clicked() { open = false; }
                });
            });
        });
    if modal.should_close() { open = false; }
    open
}

fn title(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong());
}

fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::TEXT_WEAK));
}

fn interface(ui: &mut Ui, p: &mut Prefs, reset_docks: &mut bool) {
    title(ui, "Interface size");
    ui.horizontal_wrapped(|ui| {
        for s in UI_SCALES {
            ui.radio_value(&mut p.ui_scale, s, format!("{:.0} %", s * 100.0));
        }
    });
    note(ui, "On top of the system's display scaling.");
    title(ui, "Tools docker");
    ui.checkbox(&mut p.brush_previews, "Show brush strokes");
    title(ui, "Window");
    ui.checkbox(&mut p.open_maximized, "Open maximized");
    ui.add_space(6.0);
    if ui.button("Reset docks").clicked() { *reset_docks = true; }
}

fn display(ui: &mut Ui, p: &mut Prefs, profile: Option<&str>, in_use: &str) {
    title(ui, "Renderer");
    ui.horizontal_wrapped(|ui| {
        for &r in Renderer::offered() { ui.radio_value(&mut p.renderer, r, r.name()); }
    });
    if !in_use.is_empty() && p.renderer.name() != in_use {
        ui.label(RichText::new(format!("In use: {in_use}. A restart is required.")).small().color(theme::ACCENT));
    } else {
        note(ui, "Another one is used if it can't run.");
    }
    title(ui, "Canvas quality");
    ui.horizontal_wrapped(|ui| {
        ui.radio_value(&mut p.canvas_quality, CanvasQuality::High, "High");
        ui.radio_value(&mut p.canvas_quality, CanvasQuality::Standard, "Standard");
    });
    note(ui, match p.canvas_quality {
        CanvasQuality::High => "Every screen pixel shows the exact average of the canvas under it.",
        CanvasQuality::Standard => "Mipmaps: lighter on old graphics chips, a third more memory.",
    });
    title(ui, "Zoom");
    ui.checkbox(&mut p.hidpi_scaling, "HiDPI image scaling");
    note(ui, "Off: 100 % is one image pixel per screen pixel.");
    ui.checkbox(&mut p.smooth_zoom_in, "Smooth zoom above 100 %");
    note(ui, "Off: canvas pixels stay sharp squares close up.");
    title(ui, "Screen sync");
    ui.checkbox(&mut p.vsync, "Show frames in step with the screen (V-Sync)");
    note(ui, "Off: lowest pen latency, a tear may show while panning.");
    title(ui, "Color management");
    note(ui, &format!("Monitor profile: {}", profile.unwrap_or("none found (sRGB)")));
    ui.add_enabled_ui(profile.is_some(), |ui| {
        ui.checkbox(&mut p.color_managed, "Color-manage the canvas");
        ui.add_enabled_ui(p.color_managed, |ui| {
            ui.radio_value(&mut p.perceptual, true, "Perceptual");
            ui.radio_value(&mut p.perceptual, false, "Relative colorimetric");
        });
    });
}

fn brushes(ui: &mut Ui, p: &mut Prefs) {
    title(ui, "Cursor");
    ui.checkbox(&mut p.brush_outline, "Brush outline over the canvas");
    title(ui, "Mouse click debounce");
    ui.horizontal_wrapped(|ui| {
        for ms in DEBOUNCES {
            ui.radio_value(&mut p.debounce_ms, ms, if ms == 0 { "Off".to_string() } else { format!("{ms} ms") });
        }
    });
    note(ui, "Ignores a worn mouse button's double release.");
}

fn files(ui: &mut Ui, p: &mut Prefs) {
    title(ui, "Autosave");
    ui.horizontal_wrapped(|ui| {
        for m in AUTOSAVES {
            ui.radio_value(&mut p.autosave_minutes, m, if m == 0 { "Never".to_string() } else { format!("{m} min") });
        }
    });
    note(ui, "Unsaved work is kept for recovery after this delay.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_round_trip_and_old_files_read() {
        let p = Prefs { ui_scale: 1.25, brush_previews: false, perceptual: false, debounce_ms: 40, autosave_minutes: 0, renderer: Renderer::OpenGl,
            canvas_quality: CanvasQuality::Standard, smooth_zoom_in: true, vsync: true, ..Prefs::default() };
        assert_eq!(Prefs::from_text(&p.to_text()), p);
        // A 0.7 settings file.
        let old = Prefs::from_text("hidpi_image_scaling=false\nui_scale=0.85\nclick_debounce_ms=30\n");
        assert_eq!((old.hidpi_scaling, old.ui_scale, old.debounce_ms, old.autosave_minutes), (false, 0.85, 30, 5));
        assert_eq!((old.canvas_quality, old.smooth_zoom_in, old.vsync), (CanvasQuality::High, false, false));
    }

    #[test]
    fn the_dialog_keeps_its_size() {
        let ctx = egui::Context::default();
        let (mut section, mut p, mut reset) = (Section::Interface, Prefs::default(), false);
        let mut heights = Vec::new();
        for frame in 0..40 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0))),
                events: vec![egui::Event::PointerMoved(egui::pos2(600.0 + frame as f32, 400.0))],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| { dialog(ctx, &mut section, &mut p, None, "Vulkan", &mut reset); });
            heights.push(ctx.memory(|m| m.area_rect(egui::Id::new("preferences"))).map_or(0.0, |r| r.height()));
        }
        assert!(heights[39] > 100.0 && heights[39] < 600.0, "{heights:?}");
        assert_eq!(heights[10], heights[39], "grew: {heights:?}");
    }
}
