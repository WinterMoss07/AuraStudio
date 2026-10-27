pub mod blend;
pub use blend::BlendMode;

/// x0, y0, x1, y1 in pixels (x1 / y1 exclusive).
pub type Rect = (u32, u32, u32, u32);

/// The smallest rect holding `a` and `b`.
pub fn union(a: Rect, b: Rect) -> Rect {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// Side of the squares layers are stored in (see aurastudio-layers).
pub const TILE: u32 = 64;

/// What brushes paint on: layer pixels, the pixels before the stroke and
/// the stroke's coverage, by runs that never cross a tile.
pub trait PaintSurface {
    fn width(&self)  -> u32;
    fn height(&self) -> u32;
    /// Called by the engines before they read or write `rect`: a layer
    /// creates the tiles there and fills its stroke snapshot, only where
    /// the stroke goes.
    fn prepare(&mut self, rect: Rect);
    /// (pixels, stroke base, stroke mask) of the `len` pixels from (x, y).
    /// The run stays inside one tile: `x / TILE == (x + len - 1) / TILE`.
    fn span_mut(&mut self, x: u32, y: u32, len: usize) -> (&mut [u8], &[u8], &mut [u8]);
}

/// Pixels in a run starting at `x` that stay in its tile, at most `max`.
pub fn tile_run(x: u32, max: usize) -> usize {
    ((TILE - x % TILE) as usize).min(max)
}

/// A plain surface holding every pixel (tests, benchmarks, examples).
pub struct FlatSurface {
    pub width:  u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub base:   Vec<u8>,
    pub mask:   Vec<u8>,
}

impl FlatSurface {
    /// Every pixel `rgba`, as before a stroke.
    pub fn new(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let pixels = rgba.repeat(width as usize * height as usize);
        Self { width, height, base: pixels.clone(), pixels, mask: vec![0; width as usize * height as usize] }
    }

    /// Start another stroke over what is painted now.
    pub fn next_stroke(&mut self) {
        self.base.copy_from_slice(&self.pixels);
        self.mask.fill(0);
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[i..i + 4].try_into().unwrap()
    }
}

impl PaintSurface for FlatSurface {
    fn width(&self) -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn prepare(&mut self, _rect: Rect) {}
    fn span_mut(&mut self, x: u32, y: u32, len: usize) -> (&mut [u8], &[u8], &mut [u8]) {
        let i = y as usize * self.width as usize + x as usize;
        (&mut self.pixels[i * 4..(i + len) * 4], &self.base[i * 4..(i + len) * 4], &mut self.mask[i..i + len])
    }
}
