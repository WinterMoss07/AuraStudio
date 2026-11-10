//! Merged pixels made ready for the GPU canvas off the thread that draws
//! (the paint thread's merge): cut along the pages, a page sent whole
//! sorted into its smallest kind (one color, two colors, full), a part
//! padded where it reaches the canvas edge. The drawing thread then only
//! writes them (`Canvas::upload_pieces`).

use aurastudio_core::{Rect, Rgba8};

use crate::canvas::{N, PAGE};

/// What a piece holds.
pub(crate) enum Form {
    /// All of the page, sorted; its pixels (padded) only when full.
    Whole(Sorted, Vec<Rgba8>),
    /// A part of the page (page pixels) and its rows, padded with the
    /// canvas's last column and row where it reaches the canvas edge.
    Part(Rect, Vec<Rgba8>),
}

/// The merged pixels of one GPU page, ready to write.
pub struct Piece {
    canvas: (u32, u32),
    page:   (u32, u32),
    form:   Form,
}

impl Piece {
    /// Canvas pixels it covers (inside the canvas).
    pub fn rect(&self) -> Rect {
        let (x, y) = ((self.page.0 * PAGE) as i32, (self.page.1 * PAGE) as i32);
        let frame = Rect::of_size(self.canvas.0, self.canvas.1);
        match &self.form {
            Form::Whole(..) => Rect::new(x, y, x + PAGE as i32, y + PAGE as i32).intersect(&frame),
            Form::Part(l, _) => Rect::new(x + l.x0, y + l.y0, x + l.x1, y + l.y1).intersect(&frame),
        }
    }

    /// Bytes it holds.
    pub fn bytes(&self) -> usize {
        match &self.form {
            Form::Whole(Sorted::Mono { t, .. }, px) => t.len() + px.len() * 4,
            Form::Whole(_, px) | Form::Part(_, px) => px.len() * 4,
        }
    }

    pub(crate) fn into_parts(self) -> ((u32, u32), (u32, u32), Form) { (self.canvas, self.page, self.form) }

    /// The pixels it gives over `rect()`, rows of it, as the canvas shows
    /// them (a part over a page of another kind: its own pixels).
    pub fn pixels(&self) -> Vec<Rgba8> {
        let r = self.rect();
        let (x0, y0) = ((self.page.0 * PAGE) as i32, (self.page.1 * PAGE) as i32);
        let at: Box<dyn Fn(i32, i32) -> Rgba8 + '_> = match &self.form {
            Form::Whole(Sorted::Uniform(c), _) => Box::new(move |_, _| *c),
            Form::Whole(Sorted::Mono { a, b, t }, _) => { let px = expand(*a, *b, t); Box::new(move |x, y| px[(y * PAGE as i32 + x) as usize]) }
            Form::Whole(Sorted::Full, px) => Box::new(move |x, y| px[(y * PAGE as i32 + x) as usize]),
            Form::Part(l, px) => Box::new(move |x, y| px[((y - l.y0) * l.w() as i32 + (x - l.x0)) as usize]),
        };
        (r.y0..r.y1).flat_map(|y| (r.x0..r.x1).map(move |x| (x, y))).map(|(x, y)| at(x - x0, y - y0)).collect()
    }
}

/// Repeat the canvas's last column and row over the rest of a page's
/// pixels (`r`: the page's canvas rect).
pub(crate) fn pad_page(canvas: (u32, u32), r: Rect, px: &mut [Rgba8]) {
    let (iw, ih) = ((canvas.0 as i32 - r.x0).clamp(1, PAGE as i32) as usize, (canvas.1 as i32 - r.y0).clamp(1, PAGE as i32) as usize);
    let p = PAGE as usize;
    if iw < p { for y in 0..ih { let v = px[y * p + iw - 1]; px[y * p + iw..(y + 1) * p].fill(v); } }
    if ih < p { for y in ih..p { px.copy_within((ih - 1) * p..ih * p, y * p); } }
}

/// The pages `rect` touches, (column, row).
fn pages_of(rect: Rect) -> impl Iterator<Item = (u32, u32)> {
    let p = PAGE as i32;
    (rect.y0.div_euclid(p)..(rect.y1 - 1).div_euclid(p) + 1).flat_map(move |py| (rect.x0.div_euclid(p)..(rect.x1 - 1).div_euclid(p) + 1).map(move |px| (px as u32, py as u32)))
}

/// Where page `page`'s piece of `rect` lies: the part (canvas pixels),
/// its local rect padded to the page's end where it reaches the canvas
/// edge, and whether it is all of the page inside the canvas.
fn layout(canvas: (u32, u32), page: (u32, u32), rect: Rect) -> Option<(Rect, Rect, Rect, bool)> {
    let frame = Rect::of_size(canvas.0, canvas.1);
    let pr = Rect::new((page.0 * PAGE) as i32, (page.1 * PAGE) as i32, ((page.0 + 1) * PAGE) as i32, ((page.1 + 1) * PAGE) as i32);
    let inside = pr.intersect(&frame);
    let part = pr.intersect(&rect).intersect(&frame);
    if part.is_empty() { return None; }
    let (lx0, ly0) = (part.x0 - pr.x0, part.y0 - pr.y0);
    let lx1 = if part.x1 == inside.x1 { PAGE as i32 } else { part.x1 - pr.x0 };
    let ly1 = if part.y1 == inside.y1 { PAGE as i32 } else { part.y1 - pr.y0 };
    Some((pr, part, Rect::new(lx0, ly0, lx1, ly1), part == inside))
}

/// The piece from its block (the rows of `l`, padded).
fn finish(canvas: (u32, u32), page: (u32, u32), pr: Rect, l: Rect, whole: bool, block: Vec<Rgba8>) -> Piece {
    let form = if whole {
        let page_px = if l == Rect::new(0, 0, PAGE as i32, PAGE as i32) { block } else {
            let mut px = vec![[0; 4]; N];
            for (k, row) in block.chunks_exact(l.w() as usize).enumerate() {
                let o = (l.y0 as usize + k) * PAGE as usize + l.x0 as usize;
                px[o..o + row.len()].copy_from_slice(row);
            }
            pad_page(canvas, pr, &mut px);
            px
        };
        let k = sort(&page_px);
        let keep = if matches!(k, Sorted::Full) { page_px } else { Vec::new() };
        Form::Whole(k, keep)
    } else {
        Form::Part(l, block)
    };
    Piece { canvas, page, form }
}

/// Page `page`'s piece of `src` (the rows of `rect`), copied.
fn piece(canvas: (u32, u32), page: (u32, u32), rect: Rect, src: &[Rgba8]) -> Option<Piece> {
    let (pr, part, l, whole) = layout(canvas, page, rect)?;
    let rw = rect.w() as usize;
    let at = |x: i32, y: i32| -> Rgba8 {
        let cx = (pr.x0 + x).min(part.x1 - 1);
        let cy = (pr.y0 + y).min(part.y1 - 1);
        src[(cy - rect.y0) as usize * rw + (cx - rect.x0) as usize]
    };
    let block: Vec<Rgba8> = (l.y0..l.y1).flat_map(|y| (l.x0..l.x1).map(move |x| at(x, y))).collect();
    Some(finish(canvas, page, pr, l, whole, block))
}

/// Pieces of the pages `rect` touches, from its rows `src` (copied).
pub fn prepare_slice(canvas: (u32, u32), rect: Rect, src: &[Rgba8]) -> Vec<Piece> {
    if rect.is_empty() || src.len() as u64 != rect.area() { return Vec::new(); }
    pages_of(rect).filter_map(|p| piece(canvas, p, rect, src)).collect()
}

/// The same, taking the rows: a rect inside one page that needs no
/// padding (what the paint thread merges, page by page, away from the
/// canvas edge) is moved in as it is, not copied.
pub fn prepare(canvas: (u32, u32), rect: Rect, src: Vec<Rgba8>) -> Vec<Piece> {
    if rect.is_empty() || src.len() as u64 != rect.area() { return Vec::new(); }
    let mut pages = pages_of(rect);
    if let (Some(p), None) = (pages.next(), pages.next()) {
        if let Some((pr, part, l, whole)) = layout(canvas, p, rect) {
            if part == rect && l.w() == rect.w() && l.h() == rect.h() { return vec![finish(canvas, p, pr, l, whole, src)]; }
        }
    }
    prepare_slice(canvas, rect, &src)
}

/// A page's pixels sorted: the smallest kind that gives them back.
pub enum Sorted {
    Uniform(Rgba8),
    Mono { a: Rgba8, b: Rgba8, t: Vec<u8> },
    Full,
}

/// Largest difference per channel a two-color page may make.
const MONO_TOLERANCE: i32 = 1;

/// The kind that holds `px` (a whole page) exactly (two-color: within one
/// step per channel).
pub(crate) fn sort(px: &[Rgba8]) -> Sorted {
    let Some(&a) = px.first() else { return Sorted::Uniform([0; 4]) };
    let dist = |p: &Rgba8| (0..4).map(|c| { let d = p[c] as i32 - a[c] as i32; d * d }).sum::<i32>();
    let Some(b) = px.iter().max_by_key(|p| dist(p)).copied() else { return Sorted::Uniform(a) };
    if b == a { return Sorted::Uniform(a); }
    let d: [i32; 4] = std::array::from_fn(|c| b[c] as i32 - a[c] as i32);
    let dd: i64 = d.iter().map(|v| (*v as i64) * (*v as i64)).sum();
    let mut t = Vec::with_capacity(px.len());
    for p in px {
        let dot: i64 = (0..4).map(|c| (p[c] as i64 - a[c] as i64) * d[c] as i64).sum();
        let k = ((dot * 255 + dd / 2) / dd).clamp(0, 255) as i32;
        // Given back as the shader does: a + (b − a)·k/255, rounded.
        for c in 0..4 {
            let back = a[c] as i32 + (d[c] * k + if d[c] >= 0 { 127 } else { -127 }) / 255;
            if (back - p[c] as i32).abs() > MONO_TOLERANCE { return Sorted::Full; }
        }
        t.push(k as u8);
    }
    Sorted::Mono { a, b, t }
}

/// `p` as a mix of `a` and `b` (within the tolerance), if it is one.
pub(crate) fn mix_of(a: Rgba8, b: Rgba8, p: Rgba8) -> Option<u8> {
    let d: [i32; 4] = std::array::from_fn(|c| b[c] as i32 - a[c] as i32);
    let dd: i64 = d.iter().map(|v| (*v as i64) * (*v as i64)).sum();
    if dd == 0 { return (p == a).then_some(0); }
    let dot: i64 = (0..4).map(|c| (p[c] as i64 - a[c] as i64) * d[c] as i64).sum();
    let k = ((dot * 255 + dd / 2) / dd).clamp(0, 255) as i32;
    (0..4).all(|c| (a[c] as i32 + (d[c] * k + if d[c] >= 0 { 127 } else { -127 }) / 255 - p[c] as i32).abs() <= MONO_TOLERANCE).then_some(k as u8)
}

/// The pixels of a two-color page (as `EXPAND` draws them).
pub(crate) fn expand(a: Rgba8, b: Rgba8, t: &[u8]) -> Vec<Rgba8> {
    let lut: Vec<Rgba8> = (0..256).map(|k| std::array::from_fn(|c| {
        let d = b[c] as i32 - a[c] as i32;
        (a[c] as i32 + (d * k + if d >= 0 { 127 } else { -127 }) / 255) as u8
    })).collect();
    t.iter().map(|k| lut[*k as usize]).collect()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn noise(r: Rect) -> Vec<Rgba8> { (0..r.area()).map(|i| { let v = (i as u32).wrapping_mul(2654435761); [(v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8, 255] }).collect() }

    /// Cut along the pages, each piece gives back exactly its pixels; a
    /// page inside the canvas sent whole is sorted; a rect inside one page
    /// is moved in without a copy.
    #[test]
    fn pieces_give_back_their_pixels() {
        let canvas = (700, 500);
        for rect in [Rect::new(0, 0, 700, 500), Rect::new(250, 100, 520, 300), Rect::new(600, 400, 700, 500), Rect::new(10, 10, 20, 20)] {
            let src = noise(rect);
            let pieces = prepare(canvas, rect, src.clone());
            let mut seen = 0;
            for p in &pieces {
                let r = p.rect();
                assert!(rect.contains_rect(&r));
                for (k, q) in p.pixels().iter().enumerate() {
                    let (x, y) = (r.x0 + (k as u32 % r.w()) as i32, r.y0 + (k as u32 / r.w()) as i32);
                    assert_eq!(*q, src[((y - rect.y0) * rect.w() as i32 + (x - rect.x0)) as usize], "({x}, {y}) of {rect:?}");
                    seen += 1;
                }
            }
            assert_eq!(seen as u64, rect.area());
        }
        let whole = prepare(canvas, Rect::new(256, 0, 512, 256), vec![[7; 4]; N]);
        assert!(matches!(&whole[..], [Piece { form: Form::Whole(Sorted::Uniform([7, 7, 7, 7]), _), .. }]));
        let src = noise(Rect::new(30, 30, 90, 70));
        let at = src.as_ptr();
        let one = prepare(canvas, Rect::new(30, 30, 90, 70), src);
        assert!(matches!(&one[..], [Piece { form: Form::Part(_, px), .. }] if px.as_ptr() == at), "moved, not copied");
    }

    /// A part reaching the canvas edge is padded to the page's end with
    /// the edge's last column and row (mipmaps there stay the edge's).
    #[test]
    fn edge_parts_are_padded() {
        let canvas = (300, 300);
        let rect = Rect::new(280, 280, 300, 300);
        let p = prepare(canvas, rect, noise(rect));
        let [Piece { form: Form::Part(l, px), .. }] = &p[..] else { panic!("one part") };
        assert_eq!(*l, Rect::new(24, 24, 256, 256));
        assert_eq!(px[px.len() - 1], noise(rect)[rect.area() as usize - 1]);
    }
}
