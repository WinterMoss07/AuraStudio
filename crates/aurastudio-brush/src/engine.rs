//! One brush engine for the whole app: a shared front (pen samples in,
//! changed areas out, the same stroke lifecycle) over the painting rules
//! of each brush format. Today: the dab engine (AuraStudio's own brushes)
//! and MyPaint. Krita's Pixel and Color Smudge rules come next, behind the
//! same front; the code they share (tiles, dabs, sensors) moves here piece
//! by piece, checked against the reference renders
//! (aurastudio-brushes/tests/reference.rs).

use aurastudio_canvas::{PaintSurface, Rect};

use crate::{Brush, DabEngine, MyPaintEngine};

/// One pen sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PenSample {
    /// Canvas pixels.
    pub x:        f32,
    pub y:        f32,
    /// 0..1.
    pub pressure: f32,
    /// Pen tilt toward +x / +y, -1..1 (1: 60°, libmypaint's scale); 0 for
    /// a mouse.
    pub tilt:     (f32, f32),
    /// Pen time, seconds.
    pub time:     f64,
}

impl PenSample {
    pub fn new(x: f32, y: f32, pressure: f32, time: f64) -> Self {
        Self { x, y, pressure, tilt: (0.0, 0.0), time }
    }
}

/// A stroke being painted with one brush's rules.
pub enum BrushEngine {
    Dab(DabEngine),
    MyPaint(Box<MyPaintEngine>),
}

impl BrushEngine {
    pub fn dab() -> Self { Self::Dab(DabEngine::new()) }

    pub fn mypaint(brush: hokusai::Brush) -> Self { Self::MyPaint(Box::new(MyPaintEngine::new(brush))) }

    /// Size, opacity, hardness, color, blend, eraser from the settings.
    pub fn sync_from(&mut self, b: &Brush) {
        match self {
            Self::Dab(e) => e.sync_from(b),
            Self::MyPaint(e) => e.sync_from(b),
        }
    }

    /// Dabs spaced in proportion to the size (stroke previews: a size
    /// change scales the stroke instead of shifting its dabs). MyPaint
    /// brushes space theirs by their radius already.
    pub fn proportional_spacing(&mut self) {
        if let Self::Dab(e) = self {
            let r = e.radius();
            e.set_spacing(r * 0.08);
        }
    }

    /// A new stroke.
    pub fn begin_stroke(&mut self) {
        match self {
            Self::Dab(e) => e.begin_stroke(),
            Self::MyPaint(e) => e.begin_stroke(),
        }
    }

    /// The pen comes back (onto the canvas) within the same stroke.
    pub fn resume(&mut self) {
        match self {
            Self::Dab(e) => e.begin_stroke(),
            Self::MyPaint(e) => e.resume(),
        }
    }

    /// One sample: paints up to it. Returns the area changed.
    pub fn stroke_to<S: PaintSurface>(&mut self, s: &mut S, p: PenSample) -> Option<Rect> {
        match self {
            Self::Dab(e) => e.stroke_to(s, p.x, p.y, p.pressure),
            Self::MyPaint(e) => e.stroke_to(s, p),
        }
    }

    /// Pen lifted (or off the canvas): paint what is due up to there.
    pub fn lift<S: PaintSurface>(&mut self, s: &mut S) -> Option<Rect> {
        match self {
            Self::Dab(e) => e.end_stroke(s),
            Self::MyPaint(e) => e.lift(s),
        }
    }

    /// The stroke is over.
    pub fn end_stroke<S: PaintSurface>(&mut self, s: &mut S) -> Option<Rect> {
        match self {
            Self::Dab(e) => e.end_stroke(s),
            Self::MyPaint(e) => e.end_stroke(s),
        }
    }
}
