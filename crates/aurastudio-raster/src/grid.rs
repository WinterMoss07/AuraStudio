//! The tile table of an unbounded surface: tiles anywhere (negative
//! coordinates too), grouped in chunks of `CH` × `CH` tiles held only
//! where something is stored. Painting far from the canvas costs one
//! chunk there, not a table spanning the distance.

use std::collections::HashMap;

use aurastudio_core::Pixel;

use crate::slot::Slot;

/// Tiles per chunk side.
pub(crate) const CH: i32 = 32;
const CN: usize = (CH * CH) as usize;

#[derive(Clone, Debug)]
pub(crate) struct Chunk<P: Pixel> {
    pub slots: Vec<Slot<P>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Grid<P: Pixel> {
    pub chunks: HashMap<(i32, i32), Chunk<P>>,
}

impl<P: Pixel> Default for Grid<P> {
    fn default() -> Self { Self { chunks: HashMap::new() } }
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
    #[inline]
    pub fn get(&self, tx: i32, ty: i32) -> &Slot<P> {
        let (k, i) = split(tx, ty);
        match self.chunks.get(&k) { Some(c) => &c.slots[i], None => &Slot::Fill }
    }

    /// The slot, its chunk made if needed.
    #[inline]
    pub fn get_mut(&mut self, tx: i32, ty: i32) -> &mut Slot<P> {
        let (k, i) = split(tx, ty);
        &mut self.chunks.entry(k).or_insert_with(|| Chunk { slots: (0..CN).map(|_| Slot::Fill).collect() }).slots[i]
    }

    /// Put `s` at (tx, ty); returns what was there. A fill where no chunk
    /// exists makes none.
    pub fn set(&mut self, tx: i32, ty: i32, s: Slot<P>) -> Slot<P> {
        if s.is_fill() {
            let (k, i) = split(tx, ty);
            return match self.chunks.get_mut(&k) { Some(c) => std::mem::replace(&mut c.slots[i], Slot::Fill), None => Slot::Fill };
        }
        std::mem::replace(self.get_mut(tx, ty), s)
    }

    /// Drop the chunks holding only the fill.
    pub fn prune(&mut self) { self.chunks.retain(|_, c| !c.slots.iter().all(Slot::is_fill)); }

    /// Every stored slot: (tx, ty, slot).
    pub fn iter(&self) -> impl Iterator<Item = (i32, i32, &Slot<P>)> {
        self.chunks.iter().flat_map(|(k, c)| c.slots.iter().enumerate().filter(|(_, s)| !s.is_fill()).map(move |(i, s)| { let (tx, ty) = join(*k, i); (tx, ty, s) }))
    }

    /// Every stored slot, mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (i32, i32, &mut Slot<P>)> {
        self.chunks.iter_mut().flat_map(|(k, c)| c.slots.iter_mut().enumerate().filter(|(_, s)| !s.is_fill()).map(move |(i, s)| { let (tx, ty) = join(*k, i); (tx, ty, s) }))
    }

    /// Bytes of the table itself.
    pub fn table_bytes(&self) -> usize { self.chunks.len() * (CN * std::mem::size_of::<Slot<P>>() + 32) }
}
