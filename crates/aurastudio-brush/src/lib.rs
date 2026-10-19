use aurastudio_canvas::BlendMode;

pub mod forge;
pub use forge::{Forge, ForgeBrush, PenSample};
pub use forge::myb::from_str as parse_myb;

#[derive(Clone, Debug)]
pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    /// Radius in canvas pixels; 0.5 is a 1 px brush (any diameter from
    /// 1 px, odd ones too: 3 px lines for clean-up).
    pub radius:  f32,
    /// How the brush color combines with what is already on the layer.
    pub blend:   BlendMode,
    /// Edge softness, 0..1. 1 = crisp edge (1 px anti-aliasing); lower
    /// values fade the coverage out from `radius * hardness` to `radius`.
    pub hardness: f32,
    /// Eraser: the dabs take opacity away instead of adding color.
    pub erase:   bool,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25.0, blend: BlendMode::Normal, hardness: 1.0, erase: false }
    }
}

impl Default for Brush {
    fn default() -> Self { Self::new() }
}
