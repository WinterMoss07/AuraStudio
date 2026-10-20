//! What a dab is drawn on (tiles lent by the engine) and the dab itself.

use crate::forge::color::RgbaF32;
use crate::forge::tile::TilePixels;

#[derive(Debug, Clone, Copy)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: RgbaF32, // linear, straight alpha at this boundary
    pub opaque: f32,
    pub hardness: f32,
    pub alpha_eraser: f32,
    pub aspect_ratio: f32,
    pub angle: f32, // degrees
    pub lock_alpha: f32,
    pub colorize: f32,
    pub posterize: f32,
    pub posterize_num: f32,
    pub paint: f32,
}

pub trait TiledSurface {
    fn tile_request_start(&mut self, tx: i32, ty: i32) -> &mut TilePixels;
    fn tile_request_end(&mut self, tx: i32, ty: i32);

    /// Draw one dab; true if a pixel changed.
    fn draw_dab(&mut self, dab: &Dab) -> bool {
        crate::forge::brushmodes::draw_dab_default(self, dab)
    }

    /// Read-only tile lookup, used by the default `get_color` and by any
    /// caller that wants to inspect the canvas without dirtying tiles.
    fn tile_lookup(&self, _tx: i32, _ty: i32) -> Option<&TilePixels> {
        None
    }

    /// Average color around (x, y) (smudge).
    fn get_color(&self, x: f32, y: f32, radius: f32) -> RgbaF32 {
        crate::forge::brushmodes::get_color_default(self, x, y, radius)
    }

    /// The pigment average (paint ≥ 0), else get_color.
    fn get_color_pigment(&self, x: f32, y: f32, radius: f32, paint: f32) -> RgbaF32 {
        if paint < 0.0 {
            self.get_color(x, y, radius)
        } else {
            crate::forge::brushmodes::get_color_pigment_default(self, x, y, radius, paint)
        }
    }
}
