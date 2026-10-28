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
    /// Every run of `rect` at once (its rows, cut at tile edges; in no
    /// particular order), prepared: for work on several cores. None: one
    /// by one through span_mut.
    fn spans_mut(&mut self, _rect: Rect) -> Option<Vec<Span<'_>>> { None }
}

/// A run of pixels inside one tile (PaintSurface::spans_mut): from (x, y),
/// its pixels, stroke base and stroke mask.
pub struct Span<'a> {
    pub x:      u32,
    pub y:      u32,
    pub pixels: &'a mut [u8],
    pub base:   &'a [u8],
    pub mask:   &'a mut [u8],
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
    fn spans_mut(&mut self, r: Rect) -> Option<Vec<Span<'_>>> {
        let (w, h) = (self.width as usize, self.height);
        let (x0, x1) = (r.0.min(self.width), r.2.min(self.width));
        let mut spans = Vec::new();
        let rows = self.pixels.chunks_exact_mut(w * 4).zip(self.base.chunks_exact(w * 4)).zip(self.mask.chunks_exact_mut(w));
        for (y, ((mut px, mut base), mut mask)) in rows.enumerate().skip(r.1 as usize).take(r.3.min(h).saturating_sub(r.1) as usize) {
            // (Cut at x0, then at each tile edge up to x1.)
            px = &mut px[x0 as usize * 4..x1 as usize * 4];
            base = &base[x0 as usize * 4..x1 as usize * 4];
            mask = &mut mask[x0 as usize..x1 as usize];
            let mut x = x0;
            while x < x1 {
                let n = tile_run(x, (x1 - x) as usize);
                let (p, rest_p) = px.split_at_mut(n * 4);
                let (b, rest_b) = base.split_at(n * 4);
                let (m, rest_m) = mask.split_at_mut(n);
                spans.push(Span { x, y: y as u32, pixels: p, base: b, mask: m });
                (px, base, mask) = (rest_p, rest_b, rest_m);
                x += n as u32;
            }
        }
        Some(spans)
    }
}
