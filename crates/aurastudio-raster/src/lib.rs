//! Tiled pixels: only the tiles that differ from the layer's fill color
//! exist (an empty layer costs nothing).

use aurastudio_core::{Rect, TILE};

/// Bytes of one tile: TILE × TILE RGBA pixels. Edge tiles have the same
/// size; their part past the canvas is never read.
pub const TILE_BYTES: usize = (TILE * TILE * 4) as usize;

pub type Tile = Box<[u8; TILE_BYTES]>;

#[derive(Clone)]
pub struct Tiles {
    width:  u32,
    height: u32,
    cols:   u32,
    rows:   u32,
    /// Empty (not allocated) until the first tile is stored: one slot per
    /// square of the canvas then (8 bytes each).
    tiles:  Vec<Option<Tile>>,
    /// Color of every pixel no tile covers.
    fill:   [u8; 4],
}

pub fn new_tile(fill: [u8; 4]) -> Tile {
    let mut t: Tile = vec![0u8; TILE_BYTES].into_boxed_slice().try_into().unwrap();
    if fill != [0; 4] {
        for p in t.chunks_exact_mut(4) { p.copy_from_slice(&fill); }
    }
    t
}

impl Tiles {
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
        let (cols, rows) = (width.div_ceil(TILE), height.div_ceil(TILE));
        Self { width, height, cols, rows, tiles: Vec::new(), fill }
    }

    /// From full RGBA rows (width × height × 4 bytes); squares that are all
    /// `fill` are not stored.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8], fill: [u8; 4]) -> Self {
        let mut t = Self::new(width, height, fill);
        t.write_rect((0, 0, width, height), rgba);
        t
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn fill(&self) -> [u8; 4] { self.fill }
    pub fn cols(&self) -> u32 { self.cols }

    /// Number of tiles in memory.
    pub fn stored(&self) -> usize { self.tiles.iter().filter(|t| t.is_some()).count() }

    /// Bytes held by the tiles.
    pub fn bytes(&self) -> usize { self.stored() * TILE_BYTES }

    pub fn index(&self, tx: u32, ty: u32) -> usize { (ty * self.cols + tx) as usize }

    pub fn tile(&self, tx: u32, ty: u32) -> Option<&Tile> {
        self.tiles.get(self.index(tx, ty))?.as_ref()
    }

    /// Tile (tx, ty), created from the fill color if absent.
    pub fn tile_mut(&mut self, tx: u32, ty: u32) -> &mut Tile {
        let i = self.index(tx, ty);
        let fill = self.fill;
        self.table()[i].get_or_insert_with(|| new_tile(fill))
    }

    /// The slots of the tiles, made on first use.
    fn table(&mut self) -> &mut Vec<Option<Tile>> {
        if self.tiles.is_empty() { self.tiles.resize_with((self.cols * self.rows) as usize, || None); }
        &mut self.tiles
    }

    /// Every stored tile: (tx, ty, pixels).
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32, &Tile)> {
        self.tiles.iter().enumerate().filter_map(move |(i, t)| {
            t.as_ref().map(|t| (i as u32 % self.cols, i as u32 / self.cols, t))
        })
    }

    /// Bounds of the stored tiles (where something was painted), clipped
    /// to the layer; None when it is all fill color.
    pub fn content_bounds(&self) -> Option<Rect> {
        self.tile_bounds(|_, _| true)
    }

    /// Bounds of the stored tiles with a visible pixel (alpha > 0) on the
    /// canvas: a tile erased to transparency, or left after an undo,
    /// counts as empty (`content_bounds` counts it). None: nothing shows.
    pub fn visible_bounds(&self) -> Option<Rect> {
        self.tile_bounds(|t, r| Self::shows(t, (0, 0, r.2 - r.0, r.3 - r.1)))
    }

    /// Union of the canvas rects of the stored tiles that `keep` takes
    /// (given the tile and its rect).
    fn tile_bounds(&self, keep: impl Fn(&Tile, Rect) -> bool) -> Option<Rect> {
        let mut b: Option<Rect> = None;
        for (tx, ty, t) in self.iter() {
            let r = (tx * TILE, ty * TILE, ((tx + 1) * TILE).min(self.width), ((ty + 1) * TILE).min(self.height));
            if !keep(t, r) { continue; }
            b = Some(b.map_or(r, |b| aurastudio_core::union(b, r)));
        }
        b
    }

    /// Exact bounds of the pixels with alpha (the whole layer for an
    /// opaque fill). None: nothing shows.
    pub fn pixel_bounds(&self) -> Option<Rect> {
        if self.fill[3] > 0 { return Some((0, 0, self.width, self.height)); }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (tx, ty, t) in self.iter() {
            let (bx, by) = (tx * TILE, ty * TILE);
            let (tw, th) = ((self.width - bx).min(TILE), (self.height - by).min(TILE));
            // Skip a tile that can't widen the bounds.
            if x0 <= bx && y0 <= by && x1 >= bx + tw && y1 >= by + th { continue; }
            for y in 0..th {
                let row = &t[(y * TILE * 4) as usize..((y * TILE + tw) * 4) as usize];
                let Some(a) = row.chunks_exact(4).position(|p| p[3] > 0) else { continue };
                let b = row.chunks_exact(4).rposition(|p| p[3] > 0).unwrap_or(a);
                x0 = x0.min(bx + a as u32);
                x1 = x1.max(bx + b as u32 + 1);
                y0 = y0.min(by + y);
                y1 = y1.max(by + y + 1);
            }
        }
        (x0 < x1).then_some((x0, y0, x1, y1))
    }

    /// A pixel of `rect` shows (alpha > 0; the fill's when no tile).
    pub fn any_visible_in(&self, rect: Rect) -> bool {
        let rect = (rect.0, rect.1, rect.2.min(self.width), rect.3.min(self.height));
        if rect.2 <= rect.0 || rect.3 <= rect.1 { return false; }
        for ty in rect.1 / TILE..rect.3.div_ceil(TILE) {
            for tx in rect.0 / TILE..rect.2.div_ceil(TILE) {
                let local = (rect.0.max(tx * TILE) - tx * TILE, rect.1.max(ty * TILE) - ty * TILE,
                             rect.2.min((tx + 1) * TILE) - tx * TILE, rect.3.min((ty + 1) * TILE) - ty * TILE);
                let shows = match self.tile(tx, ty) { Some(t) => Self::shows(t, local), None => self.fill[3] > 0 };
                if shows { return true; }
            }
        }
        false
    }

    /// A pixel of tile `t` inside `local` (tile coordinates) has alpha.
    fn shows(t: &Tile, local: Rect) -> bool {
        (local.1..local.3).any(|y| {
            let row = &t[((y * TILE + local.0) * 4) as usize..((y * TILE + local.2) * 4) as usize];
            row.chunks_exact(4).any(|p| p[3] > 0)
        })
    }

    /// Put a tile read from a file (or None to drop one).
    pub fn set_tile(&mut self, tx: u32, ty: u32, tile: Option<Tile>) {
        if tx < self.cols && ty < self.rows {
            let i = self.index(tx, ty);
            if tile.is_none() && self.tiles.is_empty() { return; }
            self.table()[i] = tile;
        }
    }

    /// Back to the fill color everywhere (frees every tile).
    pub fn clear(&mut self) {
        self.tiles = Vec::new();
    }

    /// One pixel (outside the canvas: the fill color).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height { return self.fill; }
        match self.tile(x / TILE, y / TILE) {
            Some(t) => {
                let i = (((y % TILE) * TILE + x % TILE) * 4) as usize;
                [t[i], t[i + 1], t[i + 2], t[i + 3]]
            }
            None => self.fill,
        }
    }

    /// Row `y` from `x0` to `x1`, as pieces that never cross a tile: `f(x,
    /// Some(pixels))` for stored tiles, `f(x, None)` for runs of the fill
    /// color (`n` pixels long).
    pub fn for_row(&self, y: u32, x0: u32, x1: u32, mut f: impl FnMut(u32, usize, Option<&[u8]>)) {
        let ty = y / TILE;
        let row = ((y % TILE) * TILE) as usize * 4;
        let mut x = x0;
        while x < x1 {
            let tx = x / TILE;
            let end = ((tx + 1) * TILE).min(x1);
            let n = (end - x) as usize;
            match self.tile(tx, ty) {
                Some(t) => {
                    let s = row + (x % TILE) as usize * 4;
                    f(x, n, Some(&t[s..s + n * 4]));
                }
                None => f(x, n, None),
            }
            x = end;
        }
    }

    /// Pixels of `rect`, RGBA rows tightly packed.
    pub fn read_rect(&self, rect: Rect) -> Vec<u8> {
        let mut out = Vec::with_capacity((rect.2 - rect.0) as usize * (rect.3 - rect.1) as usize * 4);
        for y in rect.1..rect.3 {
            self.for_row(y, rect.0, rect.2, |_, n, px| match px {
                Some(px) => out.extend_from_slice(px),
                None => for _ in 0..n { out.extend_from_slice(&self.fill); },
            });
        }
        out
    }

    /// Write `data` (RGBA rows of `rect`) into the layer. Tiles that end up
    /// all fill color are freed (undoing a stroke on an empty area gives
    /// its memory back).
    pub fn write_rect(&mut self, rect: Rect, data: &[u8]) {
        let rw = (rect.2 - rect.0) as usize;
        if rw == 0 { return; }
        for ty in rect.1 / TILE..rect.3.div_ceil(TILE) {
            for tx in rect.0 / TILE..rect.2.div_ceil(TILE) {
                let (x0, x1) = ((tx * TILE).max(rect.0), ((tx + 1) * TILE).min(rect.2));
                let (y0, y1) = ((ty * TILE).max(rect.1), ((ty + 1) * TILE).min(rect.3));
                if x0 >= x1 || y0 >= y1 { continue; }
                // Nothing stored and only fill to write: stays absent.
                let all_fill = (y0..y1).all(|y| {
                    let s = ((y - rect.1) as usize * rw + (x0 - rect.0) as usize) * 4;
                    data[s..s + (x1 - x0) as usize * 4].chunks_exact(4).all(|p| p == self.fill)
                });
                if all_fill && self.tile(tx, ty).is_none() { continue; }
                let t = self.tile_mut(tx, ty);
                for y in y0..y1 {
                    let s = ((y - rect.1) as usize * rw + (x0 - rect.0) as usize) * 4;
                    let d = (((y % TILE) * TILE + x0 % TILE) * 4) as usize;
                    let n = (x1 - x0) as usize * 4;
                    t[d..d + n].copy_from_slice(&data[s..s + n]);
                }
                self.drop_if_fill(tx, ty);
            }
        }
    }

    /// Free tile (tx, ty) if every pixel of it on the canvas is the fill
    /// color.
    pub fn drop_if_fill(&mut self, tx: u32, ty: u32) {
        let Some(t) = self.tile(tx, ty) else { return };
        let w = (self.width - tx * TILE).min(TILE) as usize;
        let h = (self.height - ty * TILE).min(TILE);
        let fill = self.fill;
        let uniform = (0..h).all(|ly| {
            let s = (ly * TILE) as usize * 4;
            t[s..s + w * 4].chunks_exact(4).all(|p| p == fill)
        });
        if uniform { self.set_tile(tx, ty, None); }
    }

    /// The same pixels on a canvas of another size, the old one placed at
    /// (dx, dy) (canvas resize, content centered). Outside: fill color.
    pub fn resized(&self, width: u32, height: u32, dx: i64, dy: i64) -> Self {
        let mut out = Self::new(width, height, self.fill);
        for (tx, ty, _) in self.iter() {
            // This tile's part of the old canvas, moved, clipped to the new.
            let (ox0, oy0) = ((tx * TILE) as i64, (ty * TILE) as i64);
            let ox1 = ((tx + 1) * TILE).min(self.width) as i64;
            let oy1 = ((ty + 1) * TILE).min(self.height) as i64;
            let (nx0, ny0) = ((ox0 + dx).max(0), (oy0 + dy).max(0));
            let (nx1, ny1) = ((ox1 + dx).min(width as i64), (oy1 + dy).min(height as i64));
            if nx0 >= nx1 || ny0 >= ny1 { continue; }
            let src = (nx0 - dx, ny0 - dy, nx1 - dx, ny1 - dy);
            let data = self.read_rect((src.0 as u32, src.1 as u32, src.2 as u32, src.3 as u32));
            out.write_rect((nx0 as u32, ny0 as u32, nx1 as u32, ny1 as u32), &data);
        }
        out
    }

    /// Every pixel as RGBA rows (tests, thumbnails of small canvases).
    pub fn to_rgba(&self) -> Vec<u8> {
        self.read_rect((0, 0, self.width, self.height))
    }
}

/// Same pixels, whatever is stored (a tile of fill color = no tile).
impl PartialEq for Tiles {
    fn eq(&self, other: &Self) -> bool {
        (self.width, self.height) == (other.width, other.height) && self.to_rgba() == other.to_rgba()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_layers_hold_nothing() {
        let t = Tiles::new(6071, 8598, [255, 255, 255, 0]);
        assert_eq!(t.stored(), 0);
        assert_eq!(t.pixel(100, 100), [255, 255, 255, 0]);
    }

    #[test]
    fn an_empty_layer_has_no_table_either() {
        let mut t = Tiles::new(4096, 4096, [0; 4]);
        assert_eq!(t.tiles.capacity(), 0);
        assert!(t.tile(3, 3).is_none() && t.iter().next().is_none() && t.content_bounds().is_none());
        t.set_tile(1, 1, None);
        assert_eq!(t.tiles.capacity(), 0, "dropping nothing makes nothing");
        t.write_rect((70, 70, 71, 71), &[1, 2, 3, 255]);
        assert_eq!(t.stored(), 1);
        t.clear();
        assert_eq!((t.stored(), t.tiles.capacity()), (0, 0), "cleared: the table goes too");
        assert_eq!(t.pixel(70, 70), [0; 4]);
    }

    #[test]
    fn rects_round_trip_and_fill_is_freed() {
        let mut t = Tiles::new(200, 150, [0; 4]);
        let rect = (60, 60, 140, 70);
        let data: Vec<u8> = (0..80 * 10).flat_map(|i| [i as u8, 2, 3, 255]).collect();
        t.write_rect(rect, &data);
        assert_eq!(t.read_rect(rect), data);
        assert_eq!(t.stored(), 6, "the rect spans 3 × 2 tiles (y 60..70 crosses 64)");
        assert_eq!(t.pixel(59, 60), [0; 4]);
        // Writing the fill back frees the tiles.
        t.write_rect(rect, &vec![0; data.len()]);
        assert_eq!(t.stored(), 0);
    }

    #[test]
    fn rows_come_in_tile_pieces() {
        let mut t = Tiles::new(300, 10, [9, 9, 9, 9]);
        t.write_rect((70, 0, 71, 1), &[1, 2, 3, 4]);
        let mut pieces = Vec::new();
        t.for_row(0, 10, 200, |x, n, px| pieces.push((x, n, px.is_some())));
        assert_eq!(pieces, vec![(10, 54, false), (64, 64, true), (128, 64, false), (192, 8, false)]);
    }

    #[test]
    fn resize_moves_the_content() {
        let mut t = Tiles::new(100, 100, [0; 4]);
        t.write_rect((10, 10, 11, 11), &[5, 5, 5, 255]);
        let r = t.resized(300, 50, 100, -5);
        assert_eq!(r.pixel(110, 5), [5, 5, 5, 255]);
        assert_eq!(r.stored(), 1);
        assert_eq!(r.resized(100, 100, -100, 5).pixel(10, 10), [5, 5, 5, 255]);
    }
}
