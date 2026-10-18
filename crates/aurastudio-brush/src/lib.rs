use aurastudio_canvas::BlendMode;

mod dab_engine;
mod mypaint_engine;
mod raster;
pub use dab_engine::DabEngine;
pub use mypaint_engine::MyPaintEngine;
/// hokusai's brush type (a parsed .myb), for the brush library.
pub use hokusai::Brush as MyPaintBrush;
pub use hokusai::myb::from_str as parse_myb;

#[derive(Clone, Debug)]
pub struct Brush {
    pub color:   [u8; 3],
    pub opacity: f32,
    pub radius:  u32,
    /// How the brush color combines with what is already on the layer.
    pub blend:   BlendMode,
    /// Edge softness, 0..1. 1 = crisp edge (1 px anti-aliasing); lower
    /// values fade the coverage out from `radius * hardness` to `radius`.
    pub hardness: f32,
}

impl Brush {
    pub fn new() -> Self {
        Self { color: [0, 0, 0], opacity: 1.0, radius: 25, blend: BlendMode::Normal, hardness: 1.0 }
    }
}

impl Default for Brush {
    fn default() -> Self { Self::new() }
}
