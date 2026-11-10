//! Every brush setting the editor shows, in CSP's categories, and how a
//! row of them is drawn: the label over a bar, the value with its arrows,
//! and at the right the button of the pen inputs that drive it. The
//! Advanced tool options window lists them by category, each with an eye:
//! lit, it shows in the Tool options docker too (per brush:
//! `Brush::tool_options`). The inputs button shows what drives the
//! setting (one input's icon, a double V for several, a crossed box for
//! none); its popup (CSP's "… Dynamics") turns each input on or off with
//! its minimum, maximum and curve.

use aurastudio_forge::{Brush, Control, Dynamic, Link, ResponseCurve, TipShape, INPUTS};
use egui::{pos2, vec2, Color32, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, Ui};

use crate::curve_edit::curve_editor;
use crate::widgets::{fill_slider_with, Fill, Icon, ROW};
use crate::theme;

/// CSP's categories (the indented ones are under Brush shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category { BrushSize, Ink, ColorJitter, AntiAliasing, BrushShape, BrushTip, Spraying, Stroke, Texture, Dual, Watercolor, Correction }

impl Category {
    pub const ALL: [Category; 12] = [Category::BrushSize, Category::Ink, Category::ColorJitter, Category::AntiAliasing, Category::BrushShape, Category::BrushTip, Category::Spraying, Category::Stroke, Category::Texture, Category::Dual, Category::Watercolor, Category::Correction];

    pub fn name(self) -> &'static str {
        match self {
            Category::BrushSize => "Brush size",
            Category::Ink => "Ink",
            Category::ColorJitter => "Color jitter",
            Category::AntiAliasing => "Anti-aliasing",
            Category::BrushShape => "Brush shape",
            Category::BrushTip => "Brush tip",
            Category::Spraying => "Spraying effect",
            Category::Stroke => "Stroke",
            Category::Texture => "Texture",
            Category::Dual => "2-Brush shape",
            Category::Watercolor => "Watercolor edge",
            Category::Correction => "Correction",
        }
    }

    /// Listed under Brush shape.
    pub fn is_sub(self) -> bool { matches!(self, Category::BrushTip | Category::Spraying | Category::Stroke | Category::Texture) }

    /// What the category is for (at the top of its settings).
    pub fn about(self) -> &'static str {
        match self {
            Category::BrushSize => "How big the brush is, and what changes it while you draw.",
            Category::Ink => "How much paint each stroke lays, how it blends, and how it mixes with what is under it.",
            Category::ColorJitter => "Changes the color along the stroke: toward the background color, and in hue, saturation and brightness.",
            Category::AntiAliasing => "Soft or hard edges.",
            Category::BrushShape => "The tip, how it is placed along the stroke, and its texture.",
            Category::BrushTip => "The shape of each tip: hardness, angle, roundness, flips.",
            Category::Spraying => "Scatters the tips around the stroke, several at a time.",
            Category::Stroke => "How far apart the tips are, and paint that keeps coming while the pen stays.",
            Category::Texture => "You can apply a paper texture to the stroke.",
            Category::Dual => "A second tip that cuts the first one: only where both are, paint shows.",
            Category::Watercolor => "Paint gathers at the edge of the stroke, as watercolor drying.",
            Category::Correction => "Smooths the line you draw.",
        }
    }

    /// The effect's switch, for categories that are one.
    pub fn switch(self) -> Option<Toggle> {
        Some(match self {
            Category::ColorJitter => (|b| b.color_dynamics.enabled, |b, v| b.color_dynamics.enabled = v),
            Category::Spraying => (|b| b.scattering.enabled, |b, v| b.scattering.enabled = v),
            Category::Texture => (|b| b.texture.enabled, |b, v| b.texture.enabled = v),
            Category::Dual => (|b| b.dual_brush.enabled, |b, v| b.dual_brush.enabled = v),
            _ => return None,
        })
    }
}

/// A switch: read it, set it.
pub type Toggle = (fn(&Brush) -> bool, fn(&mut Brush, bool));

/// The engine's switch a dynamic needs on (Photoshop's sections, not
/// shown: an input turns it on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate { Shape, Scatter, Texture, Color, Transfer }

impl Gate {
    fn on(self, b: &Brush) -> bool {
        match self {
            Gate::Shape => b.shape_dynamics.enabled,
            Gate::Scatter => b.scattering.enabled,
            Gate::Texture => b.texture.enabled,
            Gate::Color => b.color_dynamics.enabled,
            Gate::Transfer => b.transfer.enabled,
        }
    }

    /// Turned on for an input: what it held, unseen while it was off, is
    /// set back to nothing first (it would wake up with it).
    fn wake(self, b: &mut Brush) {
        if self.on(b) { return; }
        match self {
            Gate::Shape => {
                let sd = &mut b.shape_dynamics;
                (sd.size, sd.angle, sd.roundness) = Default::default();
                (sd.flip_x_jitter, sd.flip_y_jitter, sd.brush_projection) = (false, false, false);
                sd.enabled = true;
            }
            Gate::Transfer => {
                let t = &mut b.transfer;
                (t.opacity, t.flow, t.wetness, t.mix) = Default::default();
                t.enabled = true;
            }
            Gate::Scatter => b.scattering.enabled = true,
            Gate::Texture => b.texture.enabled = true,
            Gate::Color => b.color_dynamics.enabled = true,
        }
    }
}

/// How a value shows.
#[derive(Clone, Copy)]
pub enum Unit { Percent, Pixels, Degrees, Count, PerSecond }

impl Unit {
    /// How a value is written in its slider.
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
    /// Values in `unit` (a 0..1 fraction shows as 0..100 %); `mid`: the
    /// value at the bar's middle (more room for small sizes).
    Slider { lo: f32, hi: f32, mid: Option<f32>, unit: Unit, get: Get, set: Set },
    Switch { get: fn(&Brush) -> bool, set: fn(&mut Brush, bool) },
    /// The brush's blend mode.
    Blend,
}

/// The pen inputs a setting can follow.
pub struct Inputs {
    pub dynamic: fn(&mut Brush) -> &mut Dynamic,
    pub gate:    Gate,
    /// Random is a row of the popup (else the setting's own value is it).
    pub random:  bool,
    /// An angle: inputs add their angle (no minimum, maximum or curve).
    pub angle:   bool,
}

pub struct Setting {
    /// Stable name (kept in `Brush::tool_options`).
    pub id:    &'static str,
    pub cat:   Category,
    pub label: &'static str,
    pub kind:  Kind,
    pub inputs: Option<Inputs>,
    /// Shown only when this holds (hardness: a round tip).
    pub when:  Option<fn(&Brush) -> bool>,
    /// The switch it depends on (indented under it, dimmed while off).
    pub under: Option<&'static str>,
    /// What it does (the About box).
    pub help:  &'static str,
}

/// Shown in Tool options when a brush names none.
pub const DEFAULT_SHOWN: [&str; 6] = ["size", "opacity", "flow", "hardness", "smoothing", "blend"];

/// The settings a brush shows in Tool options, in the editor's order.
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
const fn s(id: &'static str, cat: Category, label: &'static str, kind: Kind, help: &'static str) -> Setting {
    Setting { id, cat, label, kind, inputs: None, when: None, under: None, help }
}
/// The pen inputs of a setting (not an angle).
const fn ins(dynamic: fn(&mut Brush) -> &mut Dynamic, gate: Gate, random: bool) -> Inputs { Inputs { dynamic, gate, random, angle: false } }
const fn driven(id: &'static str, cat: Category, label: &'static str, kind: Kind, help: &'static str, inputs: Inputs) -> Setting {
    Setting { id, cat, label, kind, inputs: Some(inputs), when: None, under: None, help }
}
const fn under(st: Setting, parent: &'static str) -> Setting { Setting { under: Some(parent), ..st } }

fn p100(v: f32) -> f32 { (v * 100.0).round() }

/// Every setting, category by category.
pub fn all() -> &'static [Setting] {
    use Category::*;
    static ALL: [Setting; 44] = [
        // Brush size.
        driven("size", BrushSize, "Brush size", slider(1.0, crate::MAX_BRUSH_SIZE as f32, Some(100.0), Unit::Pixels, |b| b.size.round(), |b, v| b.resize_with_dual_brush(v.round().max(1.0))),
            "The diameter of the tip, in pixels of the picture.", ins(|b| &mut b.shape_dynamics.size, Gate::Shape, true)),
        s("tilt_scale", BrushSize, "Tilt scale", slider(0.0, 200.0, None, Unit::Percent, |b| p100(b.shape_dynamics.tilt_scale), |b, v| b.shape_dynamics.tilt_scale = v / 100.0),
            "With the size following the tilt: how much bigger a flat pen makes it."),
        // Ink.
        driven("opacity", Ink, "Opacity", pct(|b| p100(b.opacity), |b, v| b.opacity = v / 100.0),
            "The most a stroke covers what is under it, however much you go over it.", ins(|b| &mut b.transfer.opacity, Gate::Transfer, true)),
        driven("flow", Ink, "Brush density", pct(|b| p100(b.flow), |b, v| b.flow = v / 100.0),
            "How much paint each tip lays: low, a stroke builds up where tips overlap.", ins(|b| &mut b.transfer.flow, Gate::Transfer, true)),
        s("blend", Ink, "Blend mode", Kind::Blend, "How the paint mixes with the pixels under it (Multiply darkens, Screen lightens...)."),
        s("behind", Ink, "Paint behind", switch(|b| b.behind, |b, v| b.behind = v), "Paint only shows where the layer is transparent, as if laid under it."),
        s("mixer_on", Ink, "Color mixing", switch(|b| b.mixer.enabled, |b, v| b.mixer.enabled = v), "The brush picks up the colors it goes over and mixes them with its own (Mixer brush)."),
        under(s("mixer_wet", Ink, "Wet", pct(|b| p100(b.mixer.wet), |b, v| b.mixer.wet = v / 100.0), "How much paint the brush picks up from the canvas."), "mixer_on"),
        under(s("mixer_load", Ink, "Amount of paint", pct(|b| p100(b.mixer.load), |b, v| b.mixer.load = v / 100.0), "How much of its own paint the brush holds at the start of a stroke."), "mixer_on"),
        under(s("mixer_mix", Ink, "Mix", pct(|b| p100(b.mixer.mix), |b, v| b.mixer.mix = v / 100.0), "How much of the picked-up paint is in the mix."), "mixer_on"),
        under(s("mixer_flow", Ink, "Paint flow", pct(|b| p100(b.mixer.flow), |b, v| b.mixer.flow = v / 100.0), "How fast the mixed paint goes onto the canvas."), "mixer_on"),
        under(s("mixer_all", Ink, "Sample all layers", switch(|b| b.mixer.sample_all_layers, |b, v| b.mixer.sample_all_layers = v), "Picks up the colors of every layer shown, not only this one."), "mixer_on"),
        under(s("mixer_clean", Ink, "Clean after each stroke", switch(|b| b.mixer.clean_after_stroke, |b, v| b.mixer.clean_after_stroke = v), "Each stroke starts with clean paint."), "mixer_on"),
        // Color jitter.
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.color_dynamics.fg_bg, gate: Gate::Color, random: false, angle: false }),
            ..s("fg_bg", ColorJitter, "Toward background color", pct(|b| p100(b.color_dynamics.fg_bg.jitter), |b, v| b.color_dynamics.fg_bg.jitter = v / 100.0), "How far the color may go toward the background color.") },
        s("hue_jitter", ColorJitter, "Hue", pct(|b| p100(b.color_dynamics.hue_jitter), |b, v| b.color_dynamics.hue_jitter = v / 100.0), "How far the hue may change."),
        s("saturation_jitter", ColorJitter, "Saturation", pct(|b| p100(b.color_dynamics.saturation_jitter), |b, v| b.color_dynamics.saturation_jitter = v / 100.0), "How far the saturation may change."),
        s("brightness_jitter", ColorJitter, "Brightness", pct(|b| p100(b.color_dynamics.brightness_jitter), |b, v| b.color_dynamics.brightness_jitter = v / 100.0), "How far the brightness may change."),
        s("purity", ColorJitter, "Purity", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.color_dynamics.purity), |b, v| b.color_dynamics.purity = v / 100.0), "More or less saturated, for the whole stroke."),
        s("color_each_tip", ColorJitter, "Apply to each tip", switch(|b| b.color_dynamics.per_tip, |b, v| b.color_dynamics.per_tip = v), "On: the color changes from tip to tip. Off: once per stroke."),
        // Anti-aliasing.
        s("aliased", AntiAliasing, "Hard edge (no anti-aliasing)", switch(|b| b.aliased, |b, v| b.aliased = v), "Pixels are fully painted or not at all, as a pixel-art pencil."),
        // Brush tip.
        Setting { when: Some(|b| matches!(b.tip, TipShape::Round)), ..s("hardness", BrushTip, "Hardness", pct(|b| p100(b.hardness), |b, v| b.hardness = v / 100.0), "How sharp the edge of a round tip is.") },
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.shape_dynamics.angle, gate: Gate::Shape, random: true, angle: true }),
            ..s("angle", BrushTip, "Angle", slider(-180.0, 180.0, None, Unit::Degrees, |b| b.angle.round(), |b, v| b.angle = v), "The turn of the tip.") },
        driven("roundness", BrushTip, "Thickness", pct(|b| p100(b.roundness), |b, v| b.roundness = (v / 100.0).max(0.01)),
            "Squashes the tip: 100 % is round.", ins(|b| &mut b.shape_dynamics.roundness, Gate::Shape, true)),
        s("flip_x", BrushTip, "Flip horizontally", switch(|b| b.flip_x, |b, v| b.flip_x = v), "The tip mirrored left to right."),
        s("flip_y", BrushTip, "Flip vertically", switch(|b| b.flip_y, |b, v| b.flip_y = v), "The tip mirrored top to bottom."),
        s("flip_x_jitter", BrushTip, "Random horizontal flip", switch(|b| b.shape_dynamics.flip_x_jitter, |b, v| { b.shape_dynamics.flip_x_jitter = v; if v { Gate::Shape.wake(b); b.shape_dynamics.flip_x_jitter = true; } }), "Each tip may be mirrored left to right."),
        s("flip_y_jitter", BrushTip, "Random vertical flip", switch(|b| b.shape_dynamics.flip_y_jitter, |b, v| { b.shape_dynamics.flip_y_jitter = v; if v { Gate::Shape.wake(b); b.shape_dynamics.flip_y_jitter = true; } }), "Each tip may be mirrored top to bottom."),
        s("projection", BrushTip, "Follow the pen's tilt and turn", switch(|b| b.shape_dynamics.brush_projection, |b, v| { b.shape_dynamics.brush_projection = v; if v { Gate::Shape.wake(b); b.shape_dynamics.brush_projection = true; } }), "The tip leans and turns as the pen does (brush projection)."),
        // Spraying effect.
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.scattering.scatter, gate: Gate::Scatter, random: false, angle: false }),
            ..s("scatter", Spraying, "Spray size", slider(0.0, 1000.0, None, Unit::Percent, |b| p100(b.scattering.scatter.jitter), |b, v| b.scattering.scatter.jitter = v / 100.0), "How far from the stroke the tips land, in tip sizes.") },
        s("scatter_both", Spraying, "Both directions", switch(|b| b.scattering.both_axes, |b, v| b.scattering.both_axes = v), "Off: tips only move across the stroke. On: along it too."),
        s("count", Spraying, "Particle count", slider(1.0, 16.0, None, Unit::Count, |b| b.scattering.count as f32, |b, v| b.scattering.count = v.round().clamp(1.0, 16.0) as u32), "Tips laid at each step."),
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.scattering.count_jitter, gate: Gate::Scatter, random: false, angle: false }),
            ..s("count_jitter", Spraying, "Particle count jitter", pct(|b| p100(b.scattering.count_jitter.jitter), |b, v| b.scattering.count_jitter.jitter = v / 100.0), "How much the count may change from step to step.") },
        // Stroke.
        s("spacing_on", Stroke, "Fixed spacing", switch(|b| b.spacing_enabled, |b, v| b.spacing_enabled = v), "On: tips at a fixed distance. Off: closer when you draw fast."),
        under(s("spacing", Stroke, "Gap", slider(1.0, 1000.0, Some(25.0), Unit::Percent, |b| p100(b.spacing).max(1.0), |b, v| b.spacing = v.round().max(1.0) / 100.0), "The distance between tips, in tip sizes: low, a smooth line; high, dots."), "spacing_on"),
        s("build_up", Stroke, "Continuous spray", switch(|b| b.build_up, |b, v| b.build_up = v), "Paint keeps coming while the pen stays (airbrush)."),
        under(s("build_up_rate", Stroke, "Spray rate", slider(1.0, 100.0, None, Unit::PerSecond, |b| b.build_up_rate.round(), |b, v| b.build_up_rate = v), "Tips laid per second while the pen stays."), "build_up"),
        // Texture.
        Setting { inputs: Some(Inputs { dynamic: |b| &mut b.texture.depth_jitter, gate: Gate::Texture, random: true, angle: false }),
            ..s("texture_depth", Texture, "Texture density", pct(|b| p100(b.texture.depth), |b, v| b.texture.depth = v / 100.0), "How strongly the paper shows in the stroke.") },
        under(s("texture_invert", Texture, "Invert texture", switch(|b| b.texture.invert, |b, v| b.texture.invert = v), "The paper's light and dark parts swapped."), "texture_depth"),
        under(s("texture_scale", Texture, "Scale ratio", slider(1.0, 1000.0, Some(100.0), Unit::Percent, |b| p100(b.texture.scale).max(1.0), |b, v| b.texture.scale = v / 100.0), "The size of the paper's grain."), "texture_depth"),
        under(s("texture_brightness", Texture, "Brightness", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.texture.brightness), |b, v| b.texture.brightness = v / 100.0), "Lighter or darker paper."), "texture_depth"),
        under(s("texture_contrast", Texture, "Contrast", slider(-100.0, 100.0, None, Unit::Percent, |b| p100(b.texture.contrast), |b, v| b.texture.contrast = v / 100.0), "More or less difference between the paper's light and dark parts."), "texture_depth"),
        under(s("texture_each_tip", Texture, "Apply to each tip", switch(|b| b.texture.each_tip, |b, v| b.texture.each_tip = v), "On: the texture is applied to each tip, so it gets denser where tips overlap. Off: to the stroke as a whole."), "texture_depth"),
        s("noise", Texture, "Noise", switch(|b| b.noise, |b, v| b.noise = v), "Grain at the soft edges of the tips."),
        // 2-Brush shape.
        s("dual_size", Dual, "Size", slider(1.0, crate::MAX_BRUSH_SIZE as f32, Some(100.0), Unit::Pixels, |b| b.dual_brush.size.round(), |b, v| b.dual_brush.size = v.round().max(1.0)), "The diameter of the second tip."),
    ];
    &ALL
}

/// The settings after the first table (it would grow too long).
fn more() -> &'static [Setting] {
    use Category::*;
    static MORE: [Setting; 5] = [
        s("dual_spacing", Dual, "Gap", slider(1.0, 1000.0, Some(25.0), Unit::Percent, |b| p100(b.dual_brush.spacing).max(1.0), |b, v| b.dual_brush.spacing = v / 100.0), "The distance between the second tip's marks."),
        s("dual_scatter", Dual, "Spray size", slider(0.0, 1000.0, None, Unit::Percent, |b| p100(b.dual_brush.scatter), |b, v| b.dual_brush.scatter = v / 100.0), "How far the second tip's marks land from the stroke."),
        s("dual_count", Dual, "Particle count", slider(1.0, 16.0, None, Unit::Count, |b| b.dual_brush.count as f32, |b, v| b.dual_brush.count = v.round().clamp(1.0, 16.0) as u32), "Marks of the second tip at each step."),
        s("wet_edges", Watercolor, "Watercolor edge", switch(|b| b.wet_edges, |b, v| b.wet_edges = v), "Paint gathers at the edges of the stroke, lighter inside."),
        s("smoothing", Correction, "Stabilization", pct(|b| if b.smoothing.enabled { p100(b.smoothing.amount) } else { 0.0 }, |b, v| { b.smoothing.amount = v / 100.0; b.smoothing.enabled = v > 0.0; }), "Smooths the line you draw: higher, smoother, but it trails behind the pen."),
    ];
    &MORE
}

/// Every setting.
pub fn every() -> impl Iterator<Item = &'static Setting> { all().iter().chain(more()) }

pub fn by_id(id: &str) -> Option<&'static Setting> { every().find(|s| s.id == id) }

/// The settings of `cat` (with those under Brush shape for it).
pub fn of(cat: Category) -> impl Iterator<Item = &'static Setting> {
    every().filter(move |s| s.cat == cat || (cat == Category::BrushShape && s.cat.is_sub()))
}

/// An input's name in the popup.
pub fn input_name(c: Control) -> &'static str {
    match c {
        Control::PenPressure => "Pen pressure",
        Control::PenTilt => "Tilt",
        Control::Velocity => "Velocity",
        Control::Fade => "Fade",
        Control::StylusWheel => "Stylus wheel",
        Control::Rotation => "Barrel rotation",
        Control::Direction => "Line direction",
        Control::InitialDirection => "Initial direction",
        Control::Off => "Off",
    }
}

/// The two ends of an input's axis on its curve.
fn input_ends(c: Control) -> (&'static str, &'static str) {
    match c {
        Control::PenTilt => ("Horizontal", "Vertical"),
        Control::Velocity => ("Slow", "Fast"),
        Control::Fade => ("Start", "End"),
        Control::Rotation => ("0°", "360°"),
        _ => ("0%", "100%"),
    }
}

/// What drives a setting now, for its button.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Driven { Nothing, One(Glyph), Several }

/// The small pictures of the inputs (CSP's), drawn with lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glyph { Input(Control), Random }

/// What drives `st` on `b` (the old Size / Opacity pressure switches too).
pub fn driven_by(st: &Setting, b: &mut Brush) -> Driven {
    let Some(i) = &st.inputs else { return Driven::Nothing };
    let legacy = match st.id { "size" => b.pressure_size, "opacity" => b.pressure_opacity, _ => false };
    if !i.gate.on(b) { return if legacy { Driven::One(Glyph::Input(Control::PenPressure)) } else { Driven::Nothing }; }
    let d = (i.dynamic)(b);
    let mut glyphs: Vec<Glyph> = d.links().map(|l| Glyph::Input(l.input)).collect();
    if legacy && !glyphs.contains(&Glyph::Input(Control::PenPressure)) { glyphs.push(Glyph::Input(Control::PenPressure)); }
    if i.random && d.jitter > 0.0 { glyphs.push(Glyph::Random); }
    match glyphs.len() { 0 => Driven::Nothing, 1 => Driven::One(glyphs[0]), _ => Driven::Several }
}

/// The old Size / Opacity pressure switches as a pressure input (the
/// popup edits those): the same strokes.
fn take_legacy(st: &Setting, i: &Inputs, b: &mut Brush) {
    let legacy = match st.id { "size" => std::mem::take(&mut b.pressure_size), "opacity" => std::mem::take(&mut b.pressure_opacity), _ => false };
    if !legacy { return; }
    i.gate.wake(b);
    let d = (i.dynamic)(b);
    if d.link(Control::PenPressure).is_none() { d.set_link(Control::PenPressure, Some(Link::default())); }
}

/// Draw glyph `g` in `r`.
pub fn paint_glyph(p: &Painter, r: Rect, g: Glyph, color: Color32) {
    let st = Stroke::new(1.4, color);
    let c = r.center();
    let k = r.width().min(r.height()) / 14.0;
    let at = |x: f32, y: f32| pos2(c.x + x * k, c.y + y * k);
    match g {
        // An arrow pressing down on a line.
        Glyph::Input(Control::PenPressure) => {
            p.line_segment([at(0.0, -5.0), at(0.0, 2.5)], st);
            p.add(Shape::line(vec![at(-3.0, -0.5), at(0.0, 2.5), at(3.0, -0.5)], st));
            p.line_segment([at(-4.5, 5.0), at(4.5, 5.0)], st);
        }
        // A pen leaning: a triangle.
        Glyph::Input(Control::PenTilt) => { p.add(Shape::closed_line(vec![at(0.0, -5.0), at(5.0, 4.5), at(-5.0, 4.5)], st)); }
        // Speed lines into an angle.
        Glyph::Input(Control::Velocity) => {
            p.add(Shape::line(vec![at(-5.0, -4.0), at(4.0, 0.0), at(-5.0, 4.0)], st));
            p.line_segment([at(-5.0, 0.0), at(0.5, 0.0)], st);
        }
        // Bars fading out.
        Glyph::Input(Control::Fade) => {
            for (i, a) in [1.0f32, 0.7, 0.45, 0.25].iter().enumerate() {
                let x = -4.5 + i as f32 * 3.0;
                p.line_segment([at(x, -4.0), at(x, 4.0)], Stroke::new(1.6, color.gamma_multiply(*a)));
            }
        }
        // A wheel: a circle and its notch.
        Glyph::Input(Control::StylusWheel) => {
            p.circle_stroke(c, 4.8 * k, st);
            p.line_segment([at(0.0, -4.8), at(0.0, -1.5)], st);
        }
        // A turning arrow.
        Glyph::Input(Control::Rotation) => {
            let pts: Vec<Pos2> = (0..=10).map(|i| { let a = 0.3 + i as f32 * 0.5; at(4.5 * a.cos(), 4.5 * a.sin()) }).collect();
            let end = *pts.last().unwrap_or(&c);
            p.add(Shape::line(pts, st));
            p.add(Shape::line(vec![end + vec2(-2.5 * k, -k), end, end + vec2(0.5 * k, -2.8 * k)], st));
        }
        // An arrow along the stroke.
        Glyph::Input(Control::Direction | Control::InitialDirection) => {
            p.line_segment([at(-5.0, 4.0), at(4.0, -4.0)], st);
            p.add(Shape::line(vec![at(0.0, -4.0), at(4.0, -4.0), at(4.0, 0.0)], st));
        }
        // A die.
        Glyph::Random => {
            p.rect_stroke(Rect::from_center_size(c, vec2(9.5 * k, 9.5 * k)), 1.5, st, egui::StrokeKind::Middle);
            for (x, y) in [(-2.2, -2.2), (2.2, 2.2), (0.0, 0.0)] { p.circle_filled(at(x, y), 0.9 * k, color); }
        }
        Glyph::Input(Control::Off) => {}
    }
}

/// The button's picture: one input's glyph, a double V for several, a
/// crossed box for none.
fn paint_driven(p: &Painter, r: Rect, d: Driven, color: Color32) {
    let k = r.width().min(r.height()) / 14.0;
    let c = r.center();
    let at = |x: f32, y: f32| pos2(c.x + x * k, c.y + y * k);
    match d {
        Driven::One(g) => paint_glyph(p, r, g, color),
        Driven::Several => {
            let st = Stroke::new(1.5, color);
            p.add(Shape::line(vec![at(-4.5, -4.5), at(0.0, -0.5), at(4.5, -4.5)], st));
            p.add(Shape::line(vec![at(-4.5, 0.5), at(0.0, 4.5), at(4.5, 0.5)], st));
        }
        Driven::Nothing => {
            let st = Stroke::new(1.2, color.gamma_multiply(0.6));
            let b = Rect::from_center_size(c, vec2(10.0 * k, 10.0 * k));
            p.rect_stroke(b, 1.0, st, egui::StrokeKind::Middle);
            p.line_segment([b.left_bottom(), b.right_top()], st);
        }
    }
}

/// A value: its bar (dragged), the number (typed or dragged), its
/// arrows. `w`: the whole width. The new value when it changed.
/// The eye of a row in the editor: lit, shown in Tool options.
fn eye_box(ui: &mut Ui, on: &mut bool) {
    let (r, resp) = ui.allocate_exact_size(vec2(ROW, ROW), Sense::click());
    let resp = resp.on_hover_text("Show in Tool options");
    let p = ui.painter();
    p.rect_filled(r, 2.0, Color32::from_gray(0x2b));
    p.rect_stroke(r, 2.0, Stroke::new(1.0, if *on { Color32::from_rgb(0xe8, 0xa3, 0x3a) } else { Color32::from_gray(0x66) }), egui::StrokeKind::Inside);
    if *on { crate::icons::paint(ui, Icon::Eye, r, 13.0, theme::TEXT); }
    if resp.clicked() { *on = !*on; }
}

/// The inputs button of `st`; its popup.
fn inputs_button(ui: &mut Ui, st: &Setting, i: &Inputs, b: &mut Brush) {
    let d = driven_by(st, b);
    let (r, resp) = ui.allocate_exact_size(vec2(ROW, ROW), Sense::click());
    let resp = resp.on_hover_text(format!("{} dynamics: what changes it as you draw", st.label));
    let lit = d != Driven::Nothing;
    let p = ui.painter();
    p.rect_filled(r, 3.0, if lit { Color32::from_rgb(0x2c, 0x4a, 0x86) } else if resp.hovered() { theme::WIDGET_HOVER } else { Color32::from_gray(0x3a) });
    p.rect_stroke(r, 3.0, Stroke::new(1.0, if lit { theme::ACCENT } else { Color32::from_gray(0x6a) }), egui::StrokeKind::Inside);
    paint_driven(p, r.shrink(2.0), d, theme::TEXT);
    crate::widgets::popup_below(&resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| dynamics_popup(ui, st, i, b));
}

/// One setting's row: the eye (`eye`: shown in Tool options), the label,
/// the value, the inputs button. `width`: the whole row. The row's
/// response (its hover gives the About box).
pub fn row(ui: &mut Ui, st: &Setting, b: &mut Brush, width: f32, eye: Option<&mut bool>) -> Option<egui::Response> {
    if st.when.is_some_and(|w| !w(b)) { return None; }
    let enabled = st.under.and_then(by_id).is_none_or(|p| match p.kind { Kind::Switch { get, .. } => get(b), Kind::Slider { get, .. } => get(b) > 0.0, Kind::Blend => true });
    let indent = if st.under.is_some() { 16.0 } else { 0.0 };
    // One line, everything ROW high on one axis: the eye, the slider (or
    // box, or list), the dynamics button in a column on the right.
    let r = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().interact_size.y = ROW;
        let mut w = width;
        if let Some(on) = eye { eye_box(ui, on); w -= ROW + 4.0; }
        if indent > 0.0 {
            let (r, _) = ui.allocate_exact_size(vec2(indent, ROW), Sense::hover());
            ui.painter().line_segment([pos2(r.left() + 5.0, r.top() - 2.0), pos2(r.left() + 5.0, r.bottom() + 2.0)], Stroke::new(1.0, Color32::from_gray(0x77)));
            w -= indent + 4.0;
        }
        // (Every row keeps the button's place: the sliders end together.)
        let w = (w - ROW - 4.0).max(40.0);
        match &st.kind {
            Kind::Slider { lo, hi, mid, unit, get, set } => {
                let spec = Fill { label: st.label, range: *lo..=*hi, mid: *mid, format: unit.format() };
                if let Some(v) = fill_slider_with(ui, st.id, spec, get(b), w, enabled) { set(b, v); }
            }
            Kind::Switch { get, set } => {
                let mut v = get(b);
                let changed = ui.allocate_ui_with_layout(vec2(w, ROW), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.set_width(w);
                    ui.add_enabled(enabled, egui::Checkbox::new(&mut v, st.label)).changed()
                }).inner;
                if changed { set(b, v); }
            }
            Kind::Blend => { mode_list(ui, st, &mut b.mode, w); }
        }
        match &st.inputs {
            Some(i) => inputs_button(ui, st, i, b),
            None => { ui.allocate_exact_size(vec2(ROW, ROW), Sense::hover()); }
        }
    });
    Some(r.response)
}

/// The blend mode list, its name inside as a slider has it.
fn mode_list(ui: &mut Ui, st: &Setting, mode: &mut aurastudio_forge::PaintMode, w: f32) {
    egui::ComboBox::from_id_salt(st.id)
        .selected_text(format!("{}: {}", st.label, mode.label()))
        .width(w)
        .height(400.0)
        .show_ui(ui, |ui| {
            for m in aurastudio_forge::PaintMode::all() { ui.selectable_value(mode, m, m.label()); }
        });
}

/// CSP's "… Dynamics": each input on or off, with its minimum and
/// maximum; the random part; each input's curve below.
fn dynamics_popup(ui: &mut Ui, st: &Setting, i: &Inputs, b: &mut Brush) {
    take_legacy(st, i, b);
    ui.set_min_width(470.0);
    ui.vertical_centered(|ui| ui.label(format!("{} Dynamics", st.label)));
    ui.add_space(2.0);
    ui.label(egui::RichText::new(format!("Input affecting {}", st.label)).color(theme::TEXT_WEAK));
    ui.separator();
    let gate_off = !i.gate.on(b);
    let inputs: Vec<Control> = if i.angle { INPUTS.iter().copied().chain([Control::Direction, Control::InitialDirection]).collect() } else { INPUTS.to_vec() };
    // (An input chosen while its section was off: what it held goes first.)
    let mut wake = false;
    {
        let d = (i.dynamic)(b);
        let pct = |ui: &mut Ui, id: String, label: &str, v: f32, on: bool| -> Option<f32> {
            let mut out = None;
            ui.horizontal(|ui| {
                ui.add_space(150.0);
                out = fill_slider_with(ui, &id, crate::widgets::percent(label), (v * 100.0).round(), 310.0, on);
            });
            out.map(|v| v / 100.0)
        };
        for c in &inputs {
            let link = if gate_off { None } else { d.link(*c) };
            let mut on = link.is_some();
            let changed = ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                paint_glyph(ui.painter(), r, Glyph::Input(*c), if on { theme::TEXT } else { theme::TEXT_WEAK });
                ui.checkbox(&mut on, input_name(*c)).changed()
            }).inner;
            if changed {
                if on && gate_off { wake = true; }
                if !d.set_link(*c, on.then_some(Link { input: *c, ..Link::default() })) { ui.ctx().debug_text("no room for more inputs"); }
            }
            if i.angle { continue; }
            let l = d.link(*c).filter(|_| on);
            let lv = l.unwrap_or_default();
            if let Some(v) = pct(ui, format!("{}_{:?}_min", st.id, c), "Minimum value", lv.minimum, l.is_some()) {
                d.set_link(*c, Some(Link { minimum: v, ..lv }));
            }
            if l.is_some() {
                if let Some(v) = pct(ui, format!("{}_{:?}_max", st.id, c), "Maximum value", lv.maximum, true) {
                    d.set_link(*c, Some(Link { maximum: v, ..lv }));
                }
                if *c == Control::Fade {
                    ui.horizontal(|ui| {
                        ui.add_space(150.0);
                        let spec = Fill { label: "Fade length", range: 1.0..=1000.0, mid: Some(25.0), format: Unit::Count.format() };
                        if let Some(v) = fill_slider_with(ui, &format!("{}_fade", st.id), spec, d.fade_steps as f32, 310.0, true) { d.fade_steps = v.round().max(1.0) as u32; }
                    });
                }
            }
        }
        if i.random {
            let mut on = !gate_off && d.jitter > 0.0;
            let changed = ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                paint_glyph(ui.painter(), r, Glyph::Random, if on { theme::TEXT } else { theme::TEXT_WEAK });
                ui.checkbox(&mut on, "Random").changed()
            }).inner;
            if changed {
                if on && gate_off { wake = true; }
                d.jitter = if on { 0.5 } else { 0.0 };
            }
            // Random's minimum: what is left at the most random (1 - jitter).
            if let Some(v) = pct(ui, format!("{}_random", st.id), "Minimum value", 1.0 - d.jitter, on) { d.jitter = (1.0 - v).clamp(0.0, 1.0); }
        }
    }
    if wake {
        // Keep what was just chosen, then wake the section.
        let kept = (i.dynamic)(b).clone();
        i.gate.wake(b);
        *(i.dynamic)(b) = kept;
    }
    // Each input's curve.
    if i.angle { return; }
    let d = (i.dynamic)(b);
    let links: Vec<Link> = d.links().collect();
    if links.is_empty() { return; }
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for l in links {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{} settings", input_name(l.input))).color(theme::TEXT_WEAK));
                let mut pts = l.curve.points();
                if curve_editor(ui, &format!("{}_{:?}", st.id, l.input), &mut pts, 150.0, input_name(l.input), input_ends(l.input)) {
                    d.set_link(l.input, Some(Link { curve: ResponseCurve::new(&pts), ..l }));
                }
            });
        }
    });
}

/// The stroke preview as the editor shows it: grey then white (how the
/// stroke looks on dark and light), the brush's name in its corner.
pub fn paint_preview(ui: &Ui, rect: Rect, name: &str, pic: Option<crate::atlas::Region>) {
    let p = ui.painter_at(rect);
    let (l, r) = rect.split_left_right_at_fraction(0.5);
    p.rect_filled(l, 0.0, Color32::from_gray(0xcf));
    p.rect_filled(r, 0.0, Color32::from_gray(0xfa));
    if let Some(reg) = pic {
        let s = vec2(reg.w as f32, reg.h as f32);
        let k = (rect.width() / s.x).min(rect.height() / s.y);
        crate::atlas::paint(&p, reg, Rect::from_center_size(rect.center(), s * k), Color32::WHITE);
    }
    let font = FontId::proportional(11.0);
    let g = p.layout_no_wrap(name.to_owned(), font, theme::TEXT);
    let tag = Rect::from_min_size(rect.min, g.size() + vec2(10.0, 2.0));
    p.rect_filled(tag, 0.0, Color32::from_gray(0x3c));
    p.galley(tag.min + vec2(5.0, 1.0), g, theme::TEXT);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
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
        for s in every() { if let Some(u) = s.under { assert!(matches!(by_id(u).map(|p| &p.kind), Some(Kind::Switch { .. } | Kind::Slider { .. })), "{}", s.id); } }
        // Every category but Brush shape has its own settings.
        for c in Category::ALL { assert!(of(c).next().is_some(), "{c:?}"); }
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

    /// The button tells what drives the setting.
    #[test]
    fn the_button_shows_what_drives_a_setting() {
        let st = by_id("opacity").unwrap();
        let mut b = Brush::default();
        assert_eq!(driven_by(st, &mut b), Driven::Nothing);
        b.pressure_opacity = true;
        assert_eq!(driven_by(st, &mut b), Driven::One(Glyph::Input(Control::PenPressure)));
        take_legacy(st, st.inputs.as_ref().unwrap(), &mut b);
        assert!(!b.pressure_opacity && b.transfer.enabled);
        assert_eq!(driven_by(st, &mut b), Driven::One(Glyph::Input(Control::PenPressure)));
        b.transfer.opacity.set_link(Control::PenTilt, Some(Link::default()));
        assert_eq!(driven_by(st, &mut b), Driven::Several);
    }

    /// A section turned on for an input forgets what it held while off.
    #[test]
    fn a_woken_section_starts_clean() {
        let mut b = Brush::default();
        b.shape_dynamics.enabled = false;
        b.shape_dynamics.angle.jitter = 0.7;
        Gate::Shape.wake(&mut b);
        assert!(b.shape_dynamics.enabled && b.shape_dynamics.angle.jitter == 0.0);
    }

    /// Every row and popup draws.
    #[test]
    fn rows_and_popups_draw() {
        let ctx = egui::Context::default();
        let mut b = Brush::default();
        crate::test_pass(&ctx, egui::RawInput::default(), |ui| {
            for st in every() {
                let mut on = true;
                row(ui, st, &mut b, 300.0, Some(&mut on));
                row(ui, st, &mut b, 300.0, None);
                if let Some(i) = &st.inputs {
                    for c in INPUTS { (i.dynamic)(&mut b).set_link(c, Some(Link::default())); }
                    dynamics_popup(ui, st, i, &mut b);
                }
            }
            let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0));
            for g in INPUTS.iter().map(|c| Glyph::Input(*c)).chain([Glyph::Random]) { paint_glyph(ui.painter(), r, g, Color32::WHITE); }
            for d in [Driven::Nothing, Driven::Several] { paint_driven(ui.painter(), r, d, Color32::WHITE); }
        });
    }
}
