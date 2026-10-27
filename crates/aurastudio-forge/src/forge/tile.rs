//! 64×64 tiles of fix15 RGBA, like libmypaint's.

pub const TILE_SIZE: usize = 64;

/// `[y][x][rgba]`, fix15, premultiplied.
pub type TilePixels = [[[u16; 4]; TILE_SIZE]; TILE_SIZE];
