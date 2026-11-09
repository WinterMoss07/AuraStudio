//! Every brush setting the editor shows (Photoshop's Brush Settings with
//! CSP's tool property): its section, label, slider or switch, and the pen
//! inputs that drive it. The Advanced tool options docker lists them by
//! section, each with an eye: lit, it shows in the Tool options docker
//! too (per brush: `Brush::tool_options`). A setting with pen inputs has
//! CSP's icon at the right of its slider: lit while an input drives it;
//! its popup picks the inputs (several at once), each one's minimum and
//! curve, and the random part.

use aurastudio_forge::{Brush, Control, Dynamic, Link, ResponseCurve, TipShape, INPUTS};
use egui::{vec2, Ui};

use crate::curve_edit::curve_editor;
use crate::widgets::{fill_slider, icon_toggle, paint_mode_combo, popup_below, Fill, Icon};
use crate::theme;

/// Photoshop's Brush Settings sections, and ours for the stroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section { Brush, Shape, Scattering, Texture, Dual, Color, Transfer, Stroke, Mixer }

impl Section {
    pub const ALL: [Section; 9] = [Section::Brush, Section::Shape, Section::Scattering, Section::Texture, Section::Dual, Section::Color, Section::Transfer, Section::Stroke, Section::Mixer];

    pub fn name(self) -> &'static str {
        match self {
            Section::Brush => "Brush tip",
            Section::Shape => "Shape dynamics",
            Section::Scattering => "Scattering",
            Section::Texture => "Texture",
            Section::Dual => "Dual brush",
            Section::Color => "Color dynamics",
            Section::Transfer => "Transfer",
            Section::Stroke => "Stroke",
            Section::Mixer => "Mixer brush",
        }
    }

    /// On or off (`None`: always on).
    pub fn enabled(self, b: &Brush) -> Option<bool> {
        Some(match self {
            Section::Brush | Section::Stroke => return None,
            Section::Shape => b.shape_dynamics.enabled,
            Section::Scattering => b.scattering.enabled,
            Section::Texture => b.texture.enabled,
            Section::Dual => b.dual_brush.enabled,
            Section::Color => b.color_dynamics.enabled,
            Section::Transfer => b.transfer.enabled,
            Section::Mixer => b.mixer.enabled,
        })
    }

    pub fn set_enabled(self, b: &mut Brush, on: bool) {
        match self {
            Section::Brush | Section::Stroke => {}
            Section::Shape => b.shape_dynamics.enabled = on,
            Section::Scattering => b.scattering.enabled = on,
            Section::Texture => b.texture.enabled = on,
            Section::Dual => b.dual_brush.enabled = on,
            Section::Color => b.color_dynamics.enabled = on,
            Section::Transfer => b.transfer.enabled = on,
            Section::Mixer => b.mixer.enabled = on,
        }
    }
}

/// How a value shows.
#[derive(Clone, Copy)]
pub enum Unit { Percent, Pixels, Degrees, Count, PerSecond }

impl Unit {
    fn format(self) -> &'static dyn Fn(f32) -> String {
        match self {
            Unit::Percent => &|v| format!("{v:.0} %"),
            Unit::Pixels => &|v| format!("{v:.0} px"),
            Unit::Degrees => &|v| format!("{v:.0}°"),
            Unit::Count => &|v| format!("{v:.0}"),
            Unit::PerSecond => &|v| format!("{v:.0} /s"),
        }
    }
}

type Get = fn(&Brush) -> f32;
type Set = fn(&mut Brush, f32);

/// What a setting is edited with.
pub enum Kind {
    /// Values shown in `unit` (a 0..1 fraction as 0..100 %).
    Slider { lo: f32, hi: f32, mid: Option<f32>, unit: Unit, get: Get, set: Set },
    Switch { get: fn(&Brush) -> bool, set: fn(&mut Brush, bool) },
    /// The brush's blend mode.
    Blend,
}

/// What drives a setting besides its slider.
pub struct Inputs {
    /// Its dynamic, and the section that must be on for it to work.
    pub dynamic: fn(&mut Brush) -> &mut Dynamic,
    pub section: Section,
    /// The dynamic's random part is a row of the popup (else the
    /// setting's slider is it: scatter amount, jitters).
    pub random:  bool,
    /// An angle: inputs add their angle (no minimum, no curve).
    pub angle:   bool,
}

pub struct Setting {
    /// Stable name (saved in `Brush::tool_options`).
    pub id:      &'static str,
    pub section: Section,
    pub label:   &'static str,
    pub kind:    Kind,
    pub inputs:  Option<Inputs>,
    /// Shown only when this holds (hardness: a round tip).
    pub when:    Option<fn(&Brush) -> bool>,
}

/// Shown in Tool options when a brush names none.
pub const DEFAULT_SHOWN: [&str; 7] = ["size", "opacity", "flow", "hardness", "spacing", "smoothing", "blend"];

/// The settings a brush shows in Tool options.
pub fn shown(b: &Brush) -> Vec<&'static str> {
    if b.tool_options.is_empty() { return DEFAULT_SHOWN.to_vec(); }
    all().iter().map(|s| s.id).filter(|id| b.tool_options.iter().any(|k| k == id)).collect()
}

/// Show `id` in Tool options or not.
pub fn set_shown(b: &mut Brush, id: &str, on: bool) {
    if b.tool_options.is_empty() { b.tool_options = DEFAULT_SHOWN.iter().map(|s| s.to_string()).collect(); }
    b.tool_options.retain(|k| k != id);
    if on { b.tool_options.push(id.into()); }
}

const fn slider(lo: f32, hi: f32, mid: Option<f32>, unit: Unit, get: Get, set: Set) -> Kind { Kind::Slider { lo, hi, mid, unit, get, set } }
const fn pct(get: Get, set: Set) -> Kind { slider(0.0, 100.0, None, Unit::Percent, get, set) }
const fn switch(get: fn(&Brush) -> bool, set: fn(&mut Brush, bool)) -> Kind { Kind::Switch { get, set } }
const fn s(id: &'static str, section: Section, label: &'static str, kind: Kind) -> Setting { Setting { id, section, label, kind, inputs: None, when: None } }
const fn dynamic(id: &'static str, section: Section, label: &'static str, kind: Kind, dynamic: fn(&mut Brush) -> &mut Dynamic, on: Section, random: bool) -> Setting {
    Setting { id, section, label, kind, inputs: Some(Inputs { dynamic, section: on, random, angle: false }), when: None }
}

fn p100(v: f32) -> f32 { (v * 100.0).round() }

/// Every setting, section by section.
pub fn all() -> &'static [Setting] {
    use Section::*;
    static ALL: [Setting; 47] = [
        // Brush tip.
        dynamic("size", Brush, "Size", slider(1.0, crate::MAX_BRUSH_SIZE as f32, Some(100.0), Unit::Pixels, |b| b.size.round(), |b, v| b.resize_with_dual_brush(v.round().max(1.0))), |b| &mut b.shape_dynamics.size, Shape, true),
        dynamic("opacity", Brush, "Opacity", pct(|b| p100(b.opacity), |b, v| b.opacity = v / 100.0), |b| &mut b.transfer.opacity, Transfer, true),
        dynamic("flow", Brush, "Flow", pct(|b| p100(b.flow), |b, v| b.flow = v / 100.0), |b| &mut b.transfer.flow, Transfer, true),
        Setting { when: Some(|b| matches!(b.tip, TipShape::Round)), ..s("hardness", Brush, "Hardness", pct(|b| p100(b.hardness), |b, v| b.hardness = v / 100.0)) },
        s("spacing", Brush, "Spacing", slider(1.0, 1000.0, Some(25.0), Unit::Percent, |b| p100(b.spacing).max(1.0), |b, v| b.spacing = v.round().max(1.0) / 100.0)),
        s("spacing_on", Brush, "Spacing by distance", switch(|b| b.spacing_enabled, |b, v| b.spacing_enabled = v)),
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.shape_dynamics.angle, section: Shape, random: true, angle: true }), ..s("angle", Brush, "Angle", slider(-180.0, 180.0, None, Unit::Degrees, |b| b.angle.round(), |b, v| b.angle = v)) },
        dynamic("roundness", Brush, "Roundness", pct(|b| p100(b.roundness), |b, v| b.roundness = (v / 100.0).max(0.01)), |b| &mut b.shape_dynamics.roundness, Shape, true),
        s("blend", Brush, "Blend mode", Kind::Blend),
        s("flip_x", Brush, "Flip X", switch(|b| b.flip_x, |b, v| b.flip_x = v)),
        s("flip_y", Brush, "Flip Y", switch(|b| b.flip_y, |b, v| b.flip_y = v)),
        s("aliased", Brush, "Hard edge (pencil)", switch(|b| b.aliased, |b, v| b.aliased = v)),
        s("behind", Brush, "Paint behind", switch(|b| b.behind, |b, v| b.behind = v)),
        // Shape dynamics.
        dynamic("size_jitter", Shape, "Size jitter", pct(|b| p100(b.shape_dynamics.size.jitter), |b, v| b.shape_dynamics.size.jitter = v / 100.0), |b| &mut b.shape_dynamics.size, Shape, false),
        s("tilt_scale", Shape, "Tilt scale", slider(0.0, 200.0, None, Unit::Percent, |b| p100(b.shape_dynamics.tilt_scale), |b, v| b.shape_dynamics.tilt_scale = v / 100.0)),
        s("angle_jitter", Shape, "Angle jitter", pct(|b| p100(b.shape_dynamics.angle.jitter), |b, v| b.shape_dynamics.angle.jitter = v / 100.0)),
        dynamic("roundness_jitter", Shape, "Roundness jitter", pct(|b| p100(b.shape_dynamics.roundness.jitter), |b, v| b.shape_dynamics.roundness.jitter = v / 100.0), |b| &mut b.shape_dynamics.roundness, Shape, false),
        s("flip_x_jitter", Shape, "Flip X jitter", switch(|b| b.shape_dynamics.flip_x_jitter, |b, v| b.shape_dynamics.flip_x_jitter = v)),
        s("flip_y_jitter", Shape, "Flip Y jitter", switch(|b| b.shape_dynamics.flip_y_jitter, |b, v| b.shape_dynamics.flip_y_jitter = v)),
        s("projection", Shape, "Brush projection", switch(|b| b.shape_dynamics.brush_projection, |b, v| b.shape_dynamics.brush_projection = v)),
        // Scattering.
        dynamic("scatter", Scattering, "Scatter", slider(0.0, 1000.0, None, Unit::Percent, |b| p100(b.scattering.scatter.jitter), |b, v| b.scattering.scatter.jitter = v / 100.0), |b| &mut b.scattering.scatter, Scattering, false),
        s("scatter_both", Scattering, "Both axes", switch(|b| b.scattering.both_axes, |b, v| b.scattering.both_axes = v)),
        s("count", Scattering, "Count", slider(1.0, 16.0, None, Unit::Count, |b| b.scattering.count as f32, |b, v| b.scattering.count = v.round().clamp(1.0, 16.0) as u32)),
        dynamic("count_jitter", Scattering, "Count jitter", pct(|b| p100(b.scattering.count_jitter.jitter), |b, v| b.scattering.count_jitter.jitter = v / 100.0), |b| &mut b.scattering.count_jitter, Scattering, false),
        // Texture.
        s("texture_scale", Texture, "Scale", slider(1.0, 1000.0, Some(100.0), Unit::Percent, |b| p100(b.texture.scale).max(1.0), |b, v| b.texture.scale = v / 100.0)),
        s("texture_brightness", Texture, "Brightness", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.texture.brightness), |b, v| b.texture.brightness = v / 100.0)),
        s("texture_contrast", Texture, "Contrast", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.texture.contrast), |b, v| b.texture.contrast = v / 100.0)),
        s("texture_invert", Texture, "Invert", switch(|b| b.texture.invert, |b, v| b.texture.invert = v)),
        s("texture_each_tip", Texture, "Texture each tip", switch(|b| b.texture.each_tip, |b, v| b.texture.each_tip = v)),
        dynamic("texture_depth", Texture, "Depth", pct(|b| p100(b.texture.depth), |b, v| b.texture.depth = v / 100.0), |b| &mut b.texture.depth_jitter, Texture, true),
        // Dual brush.
        s("dual_size", Dual, "Size", slider(1.0, crate::MAX_BRUSH_SIZE as f32, Some(100.0), Unit::Pixels, |b| b.dual_brush.size.round(), |b, v| b.dual_brush.size = v.round().max(1.0))),
        s("dual_spacing", Dual, "Spacing", slider(1.0, 1000.0, Some(25.0), Unit::Percent, |b| p100(b.dual_brush.spacing).max(1.0), |b, v| b.dual_brush.spacing = v / 100.0)),
        s("dual_scatter", Dual, "Scatter", slider(0.0, 1000.0, None, Unit::Percent, |b| p100(b.dual_brush.scatter), |b, v| b.dual_brush.scatter = v / 100.0)),
        s("dual_count", Dual, "Count", slider(1.0, 16.0, None, Unit::Count, |b| b.dual_brush.count as f32, |b, v| b.dual_brush.count = v.round().clamp(1.0, 16.0) as u32)),
        // Color dynamics.
        dynamic("fg_bg", Color, "Foreground / background", pct(|b| p100(b.color_dynamics.fg_bg.jitter), |b, v| b.color_dynamics.fg_bg.jitter = v / 100.0), |b| &mut b.color_dynamics.fg_bg, Color, false),
        s("hue_jitter", Color, "Hue jitter", pct(|b| p100(b.color_dynamics.hue_jitter), |b, v| b.color_dynamics.hue_jitter = v / 100.0)),
        s("saturation_jitter", Color, "Saturation jitter", pct(|b| p100(b.color_dynamics.saturation_jitter), |b, v| b.color_dynamics.saturation_jitter = v / 100.0)),
        s("brightness_jitter", Color, "Brightness jitter", pct(|b| p100(b.color_dynamics.brightness_jitter), |b, v| b.color_dynamics.brightness_jitter = v / 100.0)),
        s("purity", Color, "Purity", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.color_dynamics.purity), |b, v| b.color_dynamics.purity = v / 100.0)),
        s("color_each_tip", Color, "Each tip", switch(|b| b.color_dynamics.per_tip, |b, v| b.color_dynamics.per_tip = v)),
        // Transfer.
        dynamic("opacity_jitter", Transfer, "Opacity jitter", pct(|b| p100(b.transfer.opacity.jitter), |b, v| b.transfer.opacity.jitter = v / 100.0), |b| &mut b.transfer.opacity, Transfer, false),
        dynamic("flow_jitter", Transfer, "Flow jitter", pct(|b| p100(b.transfer.flow.jitter), |b, v| b.transfer.flow.jitter = v / 100.0), |b| &mut b.transfer.flow, Transfer, false),
        // Stroke.
        s("smoothing", Stroke, "Smoothing", pct(|b| if b.smoothing.enabled { p100(b.smoothing.amount) } else { 0.0 }, |b, v| { b.smoothing.amount = v / 100.0; b.smoothing.enabled = v > 0.0; })),
        s("build_up", Stroke, "Airbrush", switch(|b| b.build_up, |b, v| b.build_up = v)),
        s("build_up_rate", Stroke, "Airbrush rate", slider(1.0, 100.0, None, Unit::PerSecond, |b| b.build_up_rate.round(), |b, v| b.build_up_rate = v)),
        s("wet_edges", Stroke, "Wet edges", switch(|b| b.wet_edges, |b, v| b.wet_edges = v)),
        s("noise", Stroke, "Noise", switch(|b| b.noise, |b, v| b.noise = v)),
    ];
    &ALL
}

/// The mixer's settings (their section's switch is the mixer itself).
pub fn mixer() -> &'static [Setting] {
    use Section::Mixer;
    static MIXER: [Setting; 6] = [
        s("mixer_wet", Mixer, "Wet", pct(|b| p100(b.mixer.wet), |b, v| b.mixer.wet = v / 100.0)),
        s("mixer_load", Mixer, "Load", pct(|b| p100(b.mixer.load), |b, v| b.mixer.load = v / 100.0)),
        s("mixer_mix", Mixer, "Mix", pct(|b| p100(b.mixer.mix), |b, v| b.mixer.mix = v / 100.0)),
        s("mixer_flow", Mixer, "Mixer flow", pct(|b| p100(b.mixer.flow), |b, v| b.mixer.flow = v / 100.0)),
        s("mixer_all", Mixer, "Sample all layers", switch(|b| b.mixer.sample_all_layers, |b, v| b.mixer.sample_all_layers = v)),
        s("mixer_clean", Mixer, "Clean after stroke", switch(|b| b.mixer.clean_after_stroke, |b, v| b.mixer.clean_after_stroke = v)),
    ];
    &MIXER
}

/// Every setting, the mixer's included.
pub fn every() -> impl Iterator<Item = &'static Setting> { all().iter().chain(mixer()) }

pub fn by_id(id: &str) -> Option<&'static Setting> { every().find(|s| s.id == id) }

/// An input's name in the popup.
fn input_name(c: Control) -> &'static str {
    match c {
        Control::PenPressure => "Pen pressure",
        Control::PenTilt => "Tilt",
        Control::Velocity => "Velocity",
        Control::Fade => "Fade",
        Control::StylusWheel => "Stylus wheel",
        Control::Rotation => "Barrel rotation",
        Control::Direction => "Stroke direction",
        Control::InitialDirection => "Initial direction",
        Control::Off => "Off",
    }
}

/// Some input drives the setting (its legacy pressure switches too).
pub fn driven(s: &Setting, b: &mut Brush) -> bool {
    let Some(i) = &s.inputs else { return false };
    let legacy = match s.id { "size" => b.pressure_size, "opacity" => b.pressure_opacity, _ => false };
    let on = i.section.enabled(b).unwrap_or(true);
    legacy || on && (i.dynamic)(b).is_controlled()
}

/// The old Size / Opacity pressure switches as inputs (the popup edits
/// those): the same strokes.
fn take_legacy(s: &Setting, i: &Inputs, b: &mut Brush) {
    let legacy = match s.id { "size" => std::mem::take(&mut b.pressure_size), "opacity" => std::mem::take(&mut b.pressure_opacity), _ => false };
    if !legacy { return; }
    i.section.set_enabled(b, true);
    let d = (i.dynamic)(b);
    if d.link(Control::PenPressure).is_none() { d.set_link(Control::PenPressure, Some(Link::default())); }
}

/// One setting's row: an eye (`eye`: shown in Tool options), its slider or
/// switch, its inputs' icon. `width`: the whole row.
pub fn row(ui: &mut Ui, st: &Setting, b: &mut Brush, width: f32, eye: Option<&mut bool>) {
    if st.when.is_some_and(|w| !w(b)) { return; }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut w = width;
        if let Some(on) = eye {
            if icon_toggle(ui, if *on { Icon::Eye } else { Icon::EyeClosed }, "Show in Tool options", true, false).clicked() { *on = !*on; }
            w -= 24.0;
        }
        let icon = st.inputs.is_some();
        let w = if icon { w - 24.0 } else { w };
        let id = st.id;
        match &st.kind {
            Kind::Slider { lo, hi, mid, unit, get, set } => {
                let spec = Fill { label: st.label, range: *lo..=*hi, mid: *mid, format: unit.format() };
                if let Some(v) = fill_slider(ui, id, spec, get(b), w) { set(b, v); }
            }
            Kind::Switch { get, set } => {
                let mut v = get(b);
                if ui.add_sized(vec2(w, 18.0), egui::Checkbox::new(&mut v, st.label)).changed() { set(b, v); }
            }
            Kind::Blend => { paint_mode_combo(ui, id, &mut b.mode, w - 8.0); }
        }
        if let Some(i) = &st.inputs {
            let on = driven(st, b);
            let resp = icon_toggle(ui, Icon::Dynamics, "Pen inputs (pressure, tilt, velocity...)", true, on);
            popup_below(&resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| inputs_popup(ui, st, i, b));
        }
    });
}

/// CSP's "Input affecting…": each input on or off, its minimum, its
/// curve; the random part.
fn inputs_popup(ui: &mut Ui, st: &Setting, i: &Inputs, b: &mut Brush) {
    take_legacy(st, i, b);
    ui.set_min_width(330.0);
    ui.label(egui::RichText::new(format!("Input affecting {}", st.label)).strong());
    if i.section.enabled(b) == Some(false) {
        ui.label(egui::RichText::new(format!("{} is off: an input turns it on.", i.section.name())).small().color(theme::TEXT_WEAK));
    }
    ui.add_space(4.0);
    let inputs: Vec<Control> = if i.angle { INPUTS.iter().copied().chain([Control::Direction, Control::InitialDirection]).collect() } else { INPUTS.to_vec() };
    let mut turned_on = false;
    {
        let d = (i.dynamic)(b);
        for c in &inputs {
            ui.horizontal(|ui| {
                let link = d.link(*c);
                let mut on = link.is_some();
                if ui.add_sized(vec2(130.0, 18.0), egui::Checkbox::new(&mut on, input_name(*c))).changed() {
                    let fresh = Link { input: *c, ..Link::default() };
                    if d.set_link(*c, on.then_some(fresh)) { turned_on |= on; } else { ui.ctx().debug_text("no room for more inputs"); }
                }
                if let (Some(l), false) = (d.link(*c), i.angle) {
                    let spec = Fill { label: "Minimum", range: 0.0..=100.0, mid: None, format: &|v| format!("{v:.0} %") };
                    if let Some(v) = fill_slider(ui, &format!("{}_{:?}_min", st.id, c), spec, (l.minimum * 100.0).round(), 180.0) {
                        d.set_link(*c, Some(Link { minimum: v / 100.0, ..l }));
                    }
                }
            });
            if *c == Control::Fade && d.link(Control::Fade).is_some() {
                let spec = Fill { label: "Fade steps", range: 1.0..=1000.0, mid: Some(25.0), format: &|v| format!("{v:.0}") };
                if let Some(v) = fill_slider(ui, &format!("{}_fade_steps", st.id), spec, d.fade_steps as f32, 314.0) { d.fade_steps = v.round().max(1.0) as u32; }
            }
        }
        if i.random {
            ui.horizontal(|ui| {
                ui.add_sized(vec2(130.0, 18.0), egui::Label::new("Random"));
                let spec = Fill { label: "Amount", range: 0.0..=100.0, mid: None, format: &|v| format!("{v:.0} %") };
                if let Some(v) = fill_slider(ui, &format!("{}_random", st.id), spec, (d.jitter * 100.0).round(), 180.0) {
                    d.jitter = v / 100.0;
                    turned_on |= v > 0.0;
                }
            });
        }
    }
    if turned_on { i.section.set_enabled(b, true); }
    // Each input's curve (input across, the setting up).
    let d = (i.dynamic)(b);
    let links: Vec<Link> = d.links().filter(|_| !i.angle).collect();
    if !links.is_empty() {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for l in links {
                ui.vertical(|ui| {
                    let mut pts = l.curve.points();
                    if curve_editor(ui, &format!("{}_{:?}", st.id, l.input), &mut pts, 120.0, input_name(l.input)) {
                        d.set_link(l.input, Some(Link { curve: ResponseCurve::new(&pts), ..l }));
                    }
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_the_defaults_exist() {
        let mut ids: Vec<&str> = every().map(|s| s.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate ids");
        for d in DEFAULT_SHOWN { assert!(by_id(d).is_some(), "{d}"); }
    }

    /// Showing one more setting starts from the usual ones; the order is
    /// the editor's.
    #[test]
    fn shown_settings_are_kept_per_brush() {
        let mut b = Brush::default();
        assert_eq!(shown(&b), DEFAULT_SHOWN.to_vec());
        set_shown(&mut b, "scatter", true);
        set_shown(&mut b, "flow", false);
        let v = shown(&b);
        assert!(v.contains(&"scatter") && !v.contains(&"flow") && v.contains(&"size"));
        assert!(v.iter().position(|s| *s == "size") < v.iter().position(|s| *s == "scatter"));
    }

    /// Each slider gives back what it was set to (within its rounding).
    #[test]
    fn sliders_round_trip() {
        for st in every() {
            let Kind::Slider { lo, hi, get, set, .. } = &st.kind else { continue };
            let mut b = Brush::default();
            let v = ((lo + hi) / 2.0).round();
            set(&mut b, v);
            assert!((get(&b) - v).abs() <= 1.0, "{}: {} vs {v}", st.id, get(&b));
        }
    }

    /// The old pressure switch becomes a pressure input when edited.
    #[test]
    fn the_old_pressure_switch_becomes_an_input() {
        let mut b = Brush { pressure_size: true, ..Brush::default() };
        let st = by_id("size").unwrap();
        assert!(driven(st, &mut b));
        take_legacy(st, st.inputs.as_ref().unwrap(), &mut b);
        assert!(!b.pressure_size && b.shape_dynamics.enabled && b.shape_dynamics.size.link(Control::PenPressure).is_some());
        assert!(driven(st, &mut b));
    }

    /// Every row draws, with and without its eye; the popup draws for each
    /// setting with inputs, every input on.
    #[test]
    fn rows_and_popups_draw() {
        let ctx = egui::Context::default();
        let mut b = Brush::default();
        crate::test_pass(&ctx, egui::RawInput::default(), |ui| {
            for st in every() {
                let mut on = true;
                row(ui, st, &mut b, 260.0, Some(&mut on));
                row(ui, st, &mut b, 260.0, None);
                if let Some(i) = &st.inputs {
                    for c in INPUTS { (i.dynamic)(&mut b).set_link(c, Some(Link::default())); }
                    inputs_popup(ui, st, i, &mut b);
                }
            }
        });
        assert!(b.shape_dynamics.size.links().count() >= 2);
    }
}
