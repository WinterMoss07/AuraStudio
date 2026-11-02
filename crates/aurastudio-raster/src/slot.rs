//! What one square of the grid holds, in which form, and how a form is
//! encoded (the same bytes for cold tiles, undo patches and files).

use std::sync::Arc;

use aurastudio_core::{Fnv, Pixel};

/// A tile at a glance, computed once when the tile settles (compaction,
/// load): what shows, where, how it averages. Coarse display, bounds,
/// thumbnails and the merge's shortcuts read it instead of the pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Premultiplied average color of the tile's pixels on the surface.
    pub avg:   [u8; 4],
    /// Visible pixels' box inside the tile: x0, y0, x1, y1 (empty when
    /// x1 == 0).
    pub bbox:  [u16; 4],
    pub flags: u8,
}

impl Summary {
    pub const EMPTY: u8 = 1;
    pub const OPAQUE: u8 = 2;
    pub const UNIFORM: u8 = 4;
    pub fn empty(&self) -> bool { self.flags & Self::EMPTY != 0 }
    pub fn opaque(&self) -> bool { self.flags & Self::OPAQUE != 0 }
}

/// Summary and content fingerprint of a settled tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Info {
    pub sum: Summary,
    pub fp:  u64,
}

#[derive(Clone, Debug)]
pub(crate) struct FullTile<P: Pixel> {
    pub px:   Vec<P>,
    /// `None` while being painted (stale until the next compaction).
    pub info: Option<Info>,
}

/// One color, varying coverage: line art, ink, flat colo.
#[derive(Clone, Debug)]
pub(crate) struct AlphaTile<P: Pixel> {
    /// The visible pixels' color (alpha at its max).
    pub color: P,
    /// What the fully transparent pixels hold.
    pub clear: P,
    pub a:     Vec<u8>,
    pub info:  Info,
}

/// A tile kept compressed (cold, or an old undo step): its encoding.
#[derive(Clone, Debug)]
pub(crate) struct ColdTile {
    pub data: Vec<u8>,
    pub info: Info,
}

#[derive(Clone, Debug)]
pub(crate) enum Slot<P: Pixel> {
    /// The surface's fill (nothing stored).
    Fill,
    /// Every pixel this color.
    Uniform(P),
    Alpha(Arc<AlphaTile<P>>),
    /// `tile × tile` pixels, row by row (edge tiles too: their part past
    /// the surface is never read).
    Full(Arc<FullTile<P>>),
    Cold(Arc<ColdTile>),
}

impl<P: Pixel> Slot<P> {
    /// The single color of Fill / Uniform (`fill` for Fill).
    #[inline]
    pub fn color(&self, fill: P) -> Option<P> {
        match self { Slot::Fill => Some(fill), Slot::Uniform(c) => Some(*c), _ => None }
    }

    /// Every pixel of the tile (`n` of them), whatever the form.
    pub fn expand(&self, fill: P, n: usize) -> Vec<P> {
        match self {
            Slot::Fill => vec![fill; n],
            Slot::Uniform(c) => vec![*c; n],
            Slot::Full(t) => t.px.clone(),
            Slot::Alpha(t) => t.a.iter().map(|a| if *a == 0 { t.clear } else { P::join_alpha(t.color, *a) }).collect(),
            Slot::Cold(c) => match decode::<P>(&c.data, n) { Some(s) => s.expand(fill, n), None => vec![fill; n] },
        }
    }

    /// The cold tile opened (other forms as they are).
    pub fn warm(&self, n: usize) -> Option<Slot<P>> {
        match self { Slot::Cold(c) => decode(&c.data, n), s => Some(s.clone()) }
    }

    pub fn is_fill(&self) -> bool { matches!(self, Slot::Fill) }
}

thread_local! {
    /// A row's bytes while hashing (kept: no allocation per tile).
    static RAW: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The summary and fingerprint of a tile's pixels (`w` × `h` on the
/// surface, rows of `t`).
pub(crate) fn info_of<P: Pixel>(px: &[P], w: usize, h: usize, t: usize) -> Info {
    let mut sum = [0f64; 4];
    let (mut x0, mut y0, mut x1, mut y1) = (u16::MAX, u16::MAX, 0u16, 0u16);
    let (mut all_opaque, mut any) = (true, false);
    let mut fp = Fnv::default();
    RAW.with_borrow_mut(|raw| {
    for y in 0..h {
        let row = &px[y * t..y * t + w];
        raw.clear();
        for (x, p) in row.iter().enumerate() {
            let q = p.to_premul();
            for k in 0..4 { sum[k] += q[k] as f64; }
            if p.visible() {
                any = true;
                x0 = x0.min(x as u16); x1 = x1.max(x as u16 + 1);
                y0 = y0.min(y as u16); y1 = y1.max(y as u16 + 1);
            }
            all_opaque &= p.opaque();
            p.write_le(raw);
        }
        fp.bytes(raw);
    }
    });
    let n = (w * h).max(1) as f64;
    let avg = sum.map(|v| aurastudio_core::math::to_u8((v / n) as f32));
    let mut flags = 0;
    if !any { flags |= Summary::EMPTY; }
    if all_opaque && w * h > 0 { flags |= Summary::OPAQUE; }
    let bbox = if any { [x0, y0, x1, y1] } else { [0; 4] };
    Info { sum: Summary { avg, bbox, flags }, fp: fp.finish() }
}

/// The summary of one color over `w` × `h`.
pub(crate) fn info_of_color<P: Pixel>(c: P, w: usize, h: usize) -> Info {
    let q = c.to_premul().map(aurastudio_core::math::to_u8);
    let mut flags = Summary::UNIFORM;
    if !c.visible() { flags |= Summary::EMPTY; }
    if c.opaque() { flags |= Summary::OPAQUE; }
    let bbox = if c.visible() { [0, 0, w as u16, h as u16] } else { [0; 4] };
    let mut fp = Fnv::default();
    let mut raw = Vec::new();
    c.write_le(&mut raw);
    fp.u32(0xC010_0000).bytes(&raw).u32(w as u32).u32(h as u32);
    Info { sum: Summary { avg: q, bbox, flags }, fp: fp.finish() }
}

// --- Encoding ---------------------------------------------------------

const FILL: u8 = 0;
const UNIFORM: u8 = 1;
const FULL: u8 = 2;
const ALPHA: u8 = 3;

/// The bytes of a slot (tag first): fill, uniform + its pixel, LZ4 of the
/// pixels, or color + clear + LZ4 of the coverage.
pub(crate) fn encode<P: Pixel>(s: &Slot<P>) -> Vec<u8> {
    match s {
        Slot::Fill => vec![FILL],
        Slot::Uniform(c) => { let mut v = vec![UNIFORM]; c.write_le(&mut v); v }
        Slot::Full(t) => {
            let mut raw = Vec::with_capacity(t.px.len() * P::BYTES);
            for p in &t.px { p.write_le(&mut raw); }
            let mut v = vec![FULL];
            v.extend_from_slice(&lz4_flex::compress_prepend_size(&raw));
            v
        }
        Slot::Alpha(t) => {
            let mut v = vec![ALPHA];
            t.color.write_le(&mut v);
            t.clear.write_le(&mut v);
            v.extend_from_slice(&lz4_flex::compress_prepend_size(&t.a));
            v
        }
        Slot::Cold(c) => c.data.clone(),
    }
}

/// Most bytes `encode` gives for a tile of `n` pixels.
pub(crate) fn encoded_max<P: Pixel>(n: usize) -> usize { 1 + 2 * P::BYTES + 4 + n * P::BYTES + n * P::BYTES / 255 + 16 }

/// Back from `encode` (a tile of `n` pixels); `None` if damaged.
pub(crate) fn decode<P: Pixel>(b: &[u8], n: usize) -> Option<Slot<P>> {
    let (&tag, rest) = b.split_first()?;
    match tag {
        FILL if rest.is_empty() => Some(Slot::Fill),
        UNIFORM if rest.len() == P::BYTES => P::read_le(rest).map(Slot::Uniform),
        FULL => {
            let raw = lz4_flex::decompress_size_prepended(rest).ok()?;
            if raw.len() != n * P::BYTES { return None; }
            let px: Option<Vec<P>> = raw.chunks_exact(P::BYTES).map(P::read_le).collect();
            Some(Slot::Full(Arc::new(FullTile { px: px?, info: None })))
        }
        ALPHA if P::ALPHA_FORM => {
            let color = P::read_le(rest)?;
            let clear = P::read_le(rest.get(P::BYTES..)?)?;
            let a = lz4_flex::decompress_size_prepended(rest.get(2 * P::BYTES..)?).ok()?;
            if a.len() != n { return None; }
            Some(Slot::Alpha(Arc::new(AlphaTile { color, clear, a, info: Info::default() })))
        }
        _ => None,
    }
}
