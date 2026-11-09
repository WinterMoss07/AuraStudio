//! The tile table of a surface: chunks of `CH` × `CH` tiles, made only
//! where something is stored, found by index in a dense array of chunk
//! slots (no hashing). The array spans the chunks written so far (the
//! canvas and its margin: the document keeps writes inside).

use aurastudio_core::Pixel;

use crate::slot::Slot;

/// Tiles per chunk side: 128 px at 16 px tiles (a chunk's table is 1 KB),
/// 1024 px at 128 px tiles.
pub(crate) const CH: i32 = 8;
const CN: usize = (CH * CH) as usize;

#[derive(Clone, Debug)]
pub(crate) struct Chunk<P: Pixel> {
    pub slots: Vec<Slot<P>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Grid<P: Pixel> {
    /// Chunk coordinates of `cells[0]`.
    x0:    i32,
    y0:    i32,
    cols:  i32,
    rows:  i32,
    cells: Vec<Option<Box<Chunk<P>>>>,
}

impl<P: Pixel> Default for Grid<P> {
    fn default() -> Self { Self { x0: 0, y0: 0, cols: 0, rows: 0, cells: Vec::new() } }
}

/// (chunk key, index inside the chunk) of tile (tx, ty).
#[inline]
pub(crate) fn split(tx: i32, ty: i32) -> ((i32, i32), usize) {
    ((tx.div_euclid(CH), ty.div_euclid(CH)), (ty.rem_euclid(CH) * CH + tx.rem_euclid(CH)) as usize)
}

/// Tile coordinates of slot `i` of chunk `k`.
#[inline]
pub(crate) fn join(k: (i32, i32), i: usize) -> (i32, i32) { (k.0 * CH + (i as i32 % CH), k.1 * CH + (i as i32 / CH)) }

impl<P: Pixel> Grid<P> {
    /// Index of chunk `k` in `cells`, if the array spans it.
    #[inline]
    fn index(&self, k: (i32, i32)) -> Option<usize> {
        let (x, y) = (k.0 - self.x0, k.1 - self.y0);
        (x >= 0 && y >= 0 && x < self.cols && y < self.rows).then(|| (y * self.cols + x) as usize)
    }

    #[inline]
    fn chunk(&self, k: (i32, i32)) -> Option<&Chunk<P>> { self.index(k).and_then(|i| self.cells[i].as_deref()) }

    #[inline]
    pub fn get(&self, tx: i32, ty: i32) -> &Slot<P> {
        let (k, i) = split(tx, ty);
        match self.chunk(k) { Some(c) => &c.slots[i], None => &Slot::Fill }
    }

    /// The array grown to span chunk `k`.
    fn reach(&mut self, k: (i32, i32)) -> usize {
        if let Some(i) = self.index(k) { return i; }
        let (x0, y0) = if self.cells.is_empty() { k } else { (self.x0.min(k.0), self.y0.min(k.1)) };
        let (x1, y1) = if self.cells.is_empty() { (k.0 + 1, k.1 + 1) } else { ((self.x0 + self.cols).max(k.0 + 1), (self.y0 + self.rows).max(k.1 + 1)) };
        let (cols, rows) = (x1 - x0, y1 - y0);
        let mut cells: Vec<Option<Box<Chunk<P>>>> = (0..cols * rows).map(|_| None).collect();
        for (i, c) in std::mem::take(&mut self.cells).into_iter().enumerate() {
            let (x, y) = (self.x0 + i as i32 % self.cols, self.y0 + i as i32 / self.cols);
            cells[((y - y0) * cols + (x - x0)) as usize] = c;
        }
        *self = Self { x0, y0, cols, rows, cells };
        self.index(k).unwrap_or(0)
    }

    /// The slot, its chunk made if needed.
    #[inline]
    pub fn get_mut(&mut self, tx: i32, ty: i32) -> &mut Slot<P> {
        let (k, i) = split(tx, ty);
        let at = self.reach(k);
        &mut self.cells[at].get_or_insert_with(|| Box::new(Chunk { slots: (0..CN).map(|_| Slot::Fill).collect() })).slots[i]
    }

    /// Put `s` at (tx, ty). A fill where no chunk exists makes none.
    pub fn set(&mut self, tx: i32, ty: i32, s: Slot<P>) {
        if s.is_fill() {
            let (k, i) = split(tx, ty);
            if let Some(c) = self.index(k).and_then(|at| self.cells[at].as_mut()) { c.slots[i] = Slot::Fill; }
            return;
        }
        *self.get_mut(tx, ty) = s;
    }

    /// Drop the chunks holding only the fill (the array too when none is
    /// left).
    pub fn prune(&mut self) {
        for c in &mut self.cells {
            if c.as_ref().is_some_and(|c| c.slots.iter().all(Slot::is_fill)) { *c = None; }
        }
        if self.cells.iter().all(Option::is_none) { self.clear(); }
    }

    pub fn clear(&mut self) { *self = Self::default(); }

    pub fn is_empty(&self) -> bool { self.cells.iter().all(Option::is_none) }

    /// Chunks made.
    pub fn len(&self) -> usize { self.cells.iter().filter(|c| c.is_some()).count() }

    /// Every chunk made, with its key.
    pub fn chunks_mut(&mut self) -> impl Iterator<Item = ((i32, i32), &mut Chunk<P>)> {
        let (x0, y0, cols) = (self.x0, self.y0, self.cols.max(1));
        self.cells.iter_mut().enumerate().filter_map(move |(i, c)| c.as_deref_mut().map(|c| ((x0 + i as i32 % cols, y0 + i as i32 / cols), c)))
    }

    fn chunks(&self) -> impl Iterator<Item = ((i32, i32), &Chunk<P>)> {
        let (x0, y0, cols) = (self.x0, self.y0, self.cols.max(1));
        self.cells.iter().enumerate().filter_map(move |(i, c)| c.as_deref().map(|c| ((x0 + i as i32 % cols, y0 + i as i32 / cols), c)))
    }

    /// Every stored slot: (tx, ty, slot).
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32, &Slot<P>)> {
        self.chunks().flat_map(|(k, c)| c.slots.iter().enumerate().filter(|(_, s)| !s.is_fill()).map(move |(i, s)| { let (tx, ty) = join(k, i); (tx, ty, s) }))
    }

    /// Every stored slot, mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (i32, i32, &mut Slot<P>)> {
        self.chunks_mut().flat_map(|(k, c)| c.slots.iter_mut().enumerate().filter(|(_, s)| !s.is_fill()).map(move |(i, s)| { let (tx, ty) = join(k, i); (tx, ty, s) }))
    }

    /// Bytes of the table itself: the array, and each chunk's slots.
    pub fn table_bytes(&self) -> usize {
        self.cells.len() * std::mem::size_of::<Option<Box<Chunk<P>>>>() + self.len() * (CN * std::mem::size_of::<Slot<P>>() + std::mem::size_of::<Chunk<P>>())
    }
}
