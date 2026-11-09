//! The canvas on the GPU, in pages of 256 × 256 (Krita's tiles): only
//! what is painted takes memory. A page is
//! - **uniform**: one color (blank paper, an empty area), no pixels at
//!   all, just its entry in the page table;
//! - **two-color**: every pixel a mix of two colors (line art: ink and
//!   paper, with its anti-aliasing), one byte a pixel in the R8 atlas;
//! - **full**: RGBA, four bytes a pixel in the RGBA atlas.
//!
//! The page table (a small texture, one texel a page) tells the canvas
//! shader which, where in its atlas, and the colors. Pages change kind as
//! they are painted: a partial upload to a uniform page is applied to its
//! color and the page sorted again; one to a two-color page with its two
//! colors is written as bytes, else the page is made full on the GPU
//! (drawn from the R8 atlas: no copy of its pixels is kept here); a page
//! is sorted again whenever a whole page of pixels comes. The atlases
//! grow and shrink by whole pages (copied, mipmaps included); their pages
//! are 256-aligned, so each one's mipmaps (down to 1 px) never mix with
//! a neighbor's. Pages past the canvas edge repeat its last row and
//! column (the mipmaps there stay those of the edge).

use std::collections::HashMap;

use aurastudio_core::{Rect, Rgba8};
use wgpu::*;

use crate::mip::MipGen;
use crate::upload::Uploads;

/// Side of a page, px.
pub const PAGE: u32 = 256;
/// Mip levels of a page: 256 down to 1.
pub const PAGE_LEVELS: u32 = 9;
const N: usize = (PAGE * PAGE) as usize;

/// Kinds in the page table (its texel's x).
const UNIFORM: u32 = 0;
const MONO: u32 = 1;
const FULL: u32 = 2;

/// What a page holds.
enum Page {
    Uniform(Rgba8),
    /// pixel = mix(a, b, t): `t` one byte a pixel, in the R8 atlas only.
    Mono { a: Rgba8, b: Rgba8, slot: u32 },
    Full { slot: u32 },
}

/// A page's pixels sorted: the smallest kind that gives them back.
pub(crate) enum Sorted {
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
fn mix_of(a: Rgba8, b: Rgba8, p: Rgba8) -> Option<u8> {
    let d: [i32; 4] = std::array::from_fn(|c| b[c] as i32 - a[c] as i32);
    let dd: i64 = d.iter().map(|v| (*v as i64) * (*v as i64)).sum();
    if dd == 0 { return (p == a).then_some(0); }
    let dot: i64 = (0..4).map(|c| (p[c] as i64 - a[c] as i64) * d[c] as i64).sum();
    let k = ((dot * 255 + dd / 2) / dd).clamp(0, 255) as i32;
    (0..4).all(|c| (a[c] as i32 + (d[c] * k + if d[c] >= 0 { 127 } else { -127 }) / 255 - p[c] as i32).abs() <= MONO_TOLERANCE).then_some(k as u8)
}

/// The pixels of a two-color page (as `EXPAND` draws them).
#[cfg(test)]
fn expand(a: Rgba8, b: Rgba8, t: &[u8]) -> Vec<Rgba8> {
    let lut: Vec<Rgba8> = (0..256).map(|k| std::array::from_fn(|c| {
        let d = b[c] as i32 - a[c] as i32;
        (a[c] as i32 + (d * k + if d >= 0 { 127 } else { -127 }) / 255) as u8
    })).collect();
    t.iter().map(|k| lut[*k as usize]).collect()
}

fn pack(c: Rgba8) -> u32 { u32::from_le_bytes(c) }

/// One atlas: pages side by side in a texture (RGBA8 or R8), a free list.
struct Atlas {
    format:  TextureFormat,
    texture: Texture,
    view:    TextureView,
    /// Pages across, and room for.
    cols:    u32,
    cap:     u32,
    /// Slots in use (true) or free.
    used:    Vec<bool>,
    mip:     MipGen,
    levels:  u32,
    /// Bumped each time `texture` is replaced (a bind group made before
    /// reads the old one).
    version: u64,
}

impl Atlas {
    /// Pages across and down for room for `cap` (as square as can be:
    /// the room is cols × rows, all of it used).
    fn layout(cap: u32) -> (u32, u32) {
        let cols = ((cap.max(1) as f64).sqrt().ceil() as u32).max(1);
        (cols, cap.max(1).div_ceil(cols))
    }

    /// Room kept free past `n` pages in use (no copy for each new page).
    fn slack(n: u32) -> u32 { (n / 8).max(2) }

    fn make(device: &Device, format: TextureFormat, cap: u32, levels: u32) -> (Texture, TextureView, u32) {
        let (cols, rows) = Self::layout(cap);
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(if format == TextureFormat::R8Unorm { "canvas pages (two-color)" } else { "canvas pages (full)" }),
            size: Extent3d { width: cols * PAGE, height: rows * PAGE, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::COPY_SRC | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        (texture, view, cols)
    }

    fn new(device: &Device, format: TextureFormat, levels: u32) -> Self {
        let (texture, view, cols) = Self::make(device, format, 1, levels);
        let mip = MipGen::new(device, &texture);
        Self { format, texture, view, cols, cap: 1, used: vec![false], mip, levels, version: 0 }
    }

    /// At least `n` free slots: the atlas grown (by a quarter at least) if
    /// not.
    /// Writes held in `up` go first: the copy into the grown texture must
    /// see them (else those pages came out black).
    fn reserve(&mut self, device: &Device, queue: &Queue, up: &mut Uploads, n: u32) {
        let free = self.cap - self.in_use();
        if free >= n { return; }
        if up.is_held() { up.release(queue); up.hold(); }
        let want = (self.cap + n - free).max(self.cap + self.cap / 4);
        let (cols, rows) = Self::layout(want);
        let keep: Vec<u32> = (0..self.cap).collect();
        self.relayout(device, queue, cols * rows, &keep);
    }

    fn origin(&self, slot: u32) -> (u32, u32) { ((slot % self.cols) * PAGE, (slot / self.cols) * PAGE) }

    fn in_use(&self) -> u32 { self.used.iter().filter(|u| **u).count() as u32 }

    fn bytes(&self) -> u64 {
        let (w, h) = (self.texture.width() as u64, self.texture.height() as u64);
        let bpp = self.format.block_copy_size(Some(TextureAspect::All)).unwrap_or(4) as u64;
        (0..self.levels).map(|l| (w >> l).max(1) * (h >> l).max(1) * bpp).sum()
    }

    /// A free slot, the atlas grown if there is none.
    fn alloc(&mut self, device: &Device, queue: &Queue, up: &mut Uploads) -> u32 {
        self.reserve(device, queue, up, 1);
        let i = self.used.iter().position(|u| !*u).unwrap_or(0);
        if let Some(u) = self.used.get_mut(i) { *u = true; }
        i as u32
    }

    fn free(&mut self, slot: u32) { if let Some(u) = self.used.get_mut(slot as usize) { *u = false; } }

    /// A texture of room for `cap` pages; `keep[i]`: the old slot of the
    /// new slot i (all levels copied).
    fn relayout(&mut self, device: &Device, queue: &Queue, cap: u32, keep: &[u32]) {
        aurastudio_core::zone!("gpu pages relayout");
        let (texture, view, cols) = Self::make(device, self.format, cap, self.levels);
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("pages relayout") });
        // (Levels waiting to be rebuilt: rebuilt first, then copied.)
        self.mip.rebuild(&mut enc, &self.texture);
        let old_cols = self.cols;
        for (new, &old) in keep.iter().enumerate() {
            if !self.used.get(old as usize).copied().unwrap_or(false) { continue; }
            let (ox, oy) = ((old % old_cols) * PAGE, (old / old_cols) * PAGE);
            let (nx, ny) = ((new as u32 % cols) * PAGE, (new as u32 / cols) * PAGE);
            for l in 0..self.levels {
                let s = PAGE >> l;
                enc.copy_texture_to_texture(
                    TexelCopyTextureInfo { texture: &self.texture, mip_level: l, origin: Origin3d { x: ox >> l, y: oy >> l, z: 0 }, aspect: TextureAspect::All },
                    TexelCopyTextureInfo { texture: &texture, mip_level: l, origin: Origin3d { x: nx >> l, y: ny >> l, z: 0 }, aspect: TextureAspect::All },
                    Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                );
            }
        }
        queue.submit([enc.finish()]);
        let used: Vec<bool> = (0..cap as usize).map(|i| keep.get(i).is_some_and(|&o| self.used.get(o as usize).copied().unwrap_or(false))).collect();
        self.texture = texture;
        self.view = view;
        self.cols = cols;
        self.cap = cap;
        self.used = used;
        self.version += 1;
        self.mip.attach_clean(device, &self.texture);
    }

    /// Much more room than in use: cut to fit (with some slack), the
    /// pages packed at the start. Returns old slot -> new slot.
    fn shrink(&mut self, device: &Device, queue: &Queue) -> Option<HashMap<u32, u32>> {
        let n = self.in_use();
        // (Nothing left: back to one page of room.)
        let room = if n == 0 { 1 } else { n + Self::slack(n) };
        if n > 0 && self.cap <= n + 2 * Self::slack(n) { return None; }
        let (cols, rows) = Self::layout(room);
        if cols * rows >= self.cap { return None; }
        let keep: Vec<u32> = (0..self.cap).filter(|&s| self.used[s as usize]).collect();
        let map = keep.iter().enumerate().map(|(new, &old)| (old, new as u32)).collect();
        self.relayout(device, queue, cols * rows, &keep);
        Some(map)
    }

    /// With or without mipmaps: a new texture, level 0 copied, every page's
    /// levels to build.
    fn set_levels(&mut self, device: &Device, queue: &Queue, levels: u32) {
        if levels == self.levels { return; }
        let (texture, view, _) = Self::make(device, self.format, self.cap, levels);
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("pages levels") });
        enc.copy_texture_to_texture(
            TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
            Extent3d { width: self.texture.width(), height: self.texture.height(), depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        self.texture = texture;
        self.view = view;
        self.levels = levels;
        self.version += 1;
        self.mip.attach(device, &self.texture);
    }

    fn mark(&mut self, slot: u32, r: Rect) {
        if self.levels <= 1 { return; }
        let (x, y) = self.origin(slot);
        self.mip.mark(Rect::new(x as i32 + r.x0, y as i32 + r.y0, x as i32 + r.x1, y as i32 + r.y1));
    }
}

/// A full page as the transform preview draws into it: its canvas rect,
/// its place in the RGBA atlas, its slot.
pub(crate) type FullPage = (Rect, (u32, u32), u32);

/// How many pages of each kind, and the bytes (memory report).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageCount {
    pub uniform: u32,
    pub mono:    u32,
    pub full:    u32,
    pub bytes:   u64,
}

pub struct Canvas {
    pub width:  u32,
    pub height: u32,
    /// Pages across and down.
    cols:       u32,
    rows:       u32,
    pages:      Vec<Page>,
    rgba:       Atlas,
    mono:       Atlas,
    /// The page table: kind, atlas place (x | y << 16, in pages), the
    /// two colors.
    table:      Texture,
    table_view: TextureView,
    entries:    Vec<[u32; 4]>,
    table_dirty: bool,
    /// Pages a transform's preview made full (`take_lifted`).
    lifted:     Vec<usize>,
    /// Draws two-color pages into full ones (made when first needed).
    expand:     Option<Expand>,
}

impl Canvas {
    /// A blank (transparent) canvas: no pixels yet. `mipmaps`: each page's
    /// levels down to 1 px (built by aurastudio-gpu).
    pub fn new(device: &Device, width: u32, height: u32, mipmaps: bool) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        let (cols, rows) = (width.div_ceil(PAGE), height.div_ceil(PAGE));
        let levels = if mipmaps { PAGE_LEVELS } else { 1 };
        let table = device.create_texture(&TextureDescriptor {
            label: Some("canvas page table"),
            size: Extent3d { width: cols, height: rows, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba32Uint,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let table_view = table.create_view(&TextureViewDescriptor::default());
        let n = (cols * rows) as usize;
        Self {
            width, height, cols, rows,
            pages: (0..n).map(|_| Page::Uniform([0; 4])).collect(),
            rgba: Atlas::new(device, TextureFormat::Rgba8Unorm, levels),
            mono: Atlas::new(device, TextureFormat::R8Unorm, levels),
            table, table_view,
            entries: vec![[UNIFORM, 0, 0, 0]; n],
            table_dirty: true,
            lifted: Vec::new(),
            expand: None,
        }
    }

    pub fn mipmaps(&self) -> bool { self.rgba.levels > 1 }
    /// Changes whenever a texture the canvas shader binds is replaced (an
    /// atlas grown, shrunk, or with other levels): the bind group is made
    /// again. Grown then shrunk back to the same room still counts.
    pub fn generation(&self) -> u64 { self.rgba.version + self.mono.version }
    pub(crate) fn views(&self) -> (&TextureView, &TextureView, &TextureView) { (&self.rgba.view, &self.mono.view, &self.table_view) }

    /// The pages by kind, and what the atlases and the table take.
    pub fn count(&self) -> PageCount {
        let mut c = PageCount::default();
        for p in &self.pages {
            match p { Page::Uniform(_) => c.uniform += 1, Page::Mono { .. } => c.mono += 1, Page::Full { .. } => c.full += 1 }
        }
        c.bytes = self.rgba.bytes() + self.mono.bytes() + self.entries.len() as u64 * 16;
        c
    }

    /// Canvas pixels of page `i`.
    fn page_rect(&self, i: usize) -> Rect {
        let (px, py) = ((i as u32 % self.cols) * PAGE, (i as u32 / self.cols) * PAGE);
        Rect::new(px as i32, py as i32, (px + PAGE) as i32, (py + PAGE) as i32)
    }

    fn frame(&self) -> Rect { Rect::of_size(self.width, self.height) }

    /// Page `i`'s entry in the table.
    fn entry(&self, i: usize) -> [u32; 4] {
        let place = |a: &Atlas, slot: u32| { let (x, y) = a.origin(slot); (x / PAGE) | ((y / PAGE) << 16) };
        match &self.pages[i] {
            Page::Uniform(c) => [UNIFORM, 0, pack(*c), 0],
            Page::Mono { a, b, slot, .. } => [MONO, place(&self.mono, *slot), pack(*a), pack(*b)],
            Page::Full { slot } => [FULL, place(&self.rgba, *slot), 0, 0],
        }
    }

    fn set_entry(&mut self, i: usize) {
        let e = self.entry(i);
        if self.entries[i] != e { self.entries[i] = e; self.table_dirty = true; }
    }

    /// Two-color page `i` made full: its pixels drawn on the GPU from the
    /// R8 atlas into an RGBA slot.
    fn mono_to_full(&mut self, device: &Device, queue: &Queue, up: &mut Uploads, i: usize) {
        let Page::Mono { a, b, slot: m } = self.pages[i] else { return };
        let slot = self.rgba.alloc(device, queue, up);
        let (mx, my) = self.mono.origin(m);
        let (rx, ry) = self.rgba.origin(slot);
        let ex = self.expand.get_or_insert_with(|| Expand::new(device));
        ex.draw(device, up.encoder(device), (&self.mono.texture, &self.rgba.texture), (rx, ry), (mx as i32 - rx as i32, my as i32 - ry as i32), a, b);
        self.rgba.mark(slot, Rect::new(0, 0, PAGE as i32, PAGE as i32));
        self.mono.free(m);
        self.pages[i] = Page::Full { slot };
        self.set_entry(i);
        if !up.is_held() { up.release(queue); }
    }

    /// Repeat the canvas's last column and row over the rest of page `i`.
    fn pad(&self, i: usize, px: &mut [Rgba8]) {
        let r = self.page_rect(i);
        let (iw, ih) = ((self.width as i32 - r.x0).clamp(1, PAGE as i32) as usize, (self.height as i32 - r.y0).clamp(1, PAGE as i32) as usize);
        let p = PAGE as usize;
        if iw < p { for y in 0..ih { let v = px[y * p + iw - 1]; px[y * p + iw..(y + 1) * p].fill(v); } }
        if ih < p { for y in ih..p { px.copy_within((ih - 1) * p..ih * p, y * p); } }
    }

    /// Page `i` is now `px` (all of it, padded): sorted, its slots and the
    /// GPU's pixels changed as needed.
    fn set_page(&mut self, device: &Device, queue: &Queue, up: &mut Uploads, i: usize, px: &[Rgba8], sorted: Sorted) {
        let old = std::mem::replace(&mut self.pages[i], Page::Uniform([0; 4]));
        let (mono_slot, full_slot) = match old { Page::Mono { slot, .. } => (Some(slot), None), Page::Full { slot } => (None, Some(slot)), Page::Uniform(_) => (None, None) };
        let all = Rect::new(0, 0, PAGE as i32, PAGE as i32);
        self.pages[i] = match sorted {
            Sorted::Uniform(c) => Page::Uniform(c),
            Sorted::Mono { a, b, t } => {
                let slot = mono_slot.unwrap_or_else(|| self.mono.alloc(device, queue, up));
                let (x, y) = self.mono.origin(slot);
                up.write(device, queue, &self.mono.texture, (x, y), (PAGE, PAGE), &t);
                self.mono.mark(slot, all);
                Page::Mono { a, b, slot }
            }
            Sorted::Full => {
                let slot = full_slot.unwrap_or_else(|| self.rgba.alloc(device, queue, up));
                let (x, y) = self.rgba.origin(slot);
                up.write(device, queue, &self.rgba.texture, (x, y), (PAGE, PAGE), px.as_flattened());
                self.rgba.mark(slot, all);
                Page::Full { slot }
            }
        };
        // Slots no longer used.
        let (mono_now, full_now) = (matches!(self.pages[i], Page::Mono { .. }), matches!(self.pages[i], Page::Full { .. }));
        if let (Some(s), false) = (full_slot, full_now) { self.rgba.free(s); }
        if let (Some(s), false) = (mono_slot, mono_now) { self.mono.free(s); }
        self.set_entry(i);
    }

    /// Page `i`, which `rect` covers whole inside the canvas: its pixels
    /// from `src` (the rows of `rect`), padded.
    fn whole_page(&self, i: usize, rect: Rect, src: &[Rgba8]) -> Vec<Rgba8> {
        let pr = self.page_rect(i);
        let part = pr.intersect(&self.frame());
        let rw = rect.w() as usize;
        let mut page = vec![[0; 4]; N];
        for y in part.y0..part.y1 {
            let o = (y - rect.y0) as usize * rw + (part.x0 - rect.x0) as usize;
            let d = (y - pr.y0) as usize * PAGE as usize;
            page[d..d + part.w() as usize].copy_from_slice(&src[o..o + part.w() as usize]);
        }
        self.pad(i, &mut page);
        page
    }

    /// Copy merged pixels (the rows of `rect`, straight RGBA, inside the
    /// canvas) to the pages.
    pub(crate) fn upload(&mut self, device: &Device, queue: &Queue, up: &mut Uploads, rect: Rect, src: &[Rgba8]) {
        aurastudio_core::zone!("gpu pages upload");
        let rect = rect.intersect(&self.frame());
        if rect.is_empty() { return; }
        let rw = rect.w() as usize;
        let p = PAGE as i32;
        // The writes in one submission (released at the end, unless the
        // caller holds them for the whole frame).
        let outer = up.is_held();
        up.hold();
        let ids: Vec<usize> = (rect.y0.div_euclid(p)..(rect.y1 - 1).div_euclid(p) + 1)
            .flat_map(|py| (rect.x0.div_euclid(p)..(rect.x1 - 1).div_euclid(p) + 1).map(move |px| (py, px)))
            .map(|(py, px)| (py as u32 * self.cols + px as u32) as usize).collect();
        // Pages covered whole are sorted first: the atlases then grow once
        // to what they need (a picture opened: one copy, not one per page).
        let mut sorted: HashMap<usize, Sorted> = HashMap::new();
        if ids.len() > 1 {
            let (mut full, mut mono) = (0u32, 0u32);
            for &i in &ids {
                let inside = self.page_rect(i).intersect(&self.frame());
                if inside.intersect(&rect) != inside { continue; }
                let page = self.whole_page(i, rect, src);
                let k = sort(&page);
                match (&k, &self.pages[i]) {
                    (Sorted::Full, Page::Full { .. }) | (Sorted::Mono { .. }, Page::Mono { .. }) => {}
                    (Sorted::Full, _) => full += 1,
                    (Sorted::Mono { .. }, _) => mono += 1,
                    _ => {}
                }
                sorted.insert(i, k);
            }
            self.rgba.reserve(device, queue, up, full);
            self.mono.reserve(device, queue, up, mono);
        }
        for i in ids {
            {
                let pr = self.page_rect(i);
                let inside = pr.intersect(&self.frame());
                let part = pr.intersect(&rect);
                // Local rect of the part, and how far it reaches with the
                // padding (the canvas edge repeated to the page's end).
                let (lx0, ly0) = ((part.x0 - pr.x0) as usize, (part.y0 - pr.y0) as usize);
                let lx1 = if part.x1 == inside.x1 { PAGE as usize } else { (part.x1 - pr.x0) as usize };
                let ly1 = if part.y1 == inside.y1 { PAGE as usize } else { (part.y1 - pr.y0) as usize };
                // The source pixel for local (x, y) (clamped to the part).
                let at = |x: usize, y: usize| -> Rgba8 {
                    let cx = (pr.x0 + x as i32).min(part.x1 - 1);
                    let cy = (pr.y0 + y as i32).min(part.y1 - 1);
                    src[(cy - rect.y0) as usize * rw + (cx - rect.x0) as usize]
                };
                let whole = part == inside;
                if let Some(k) = sorted.remove(&i) {
                    // (Sorted above; a full page's pixels taken again.)
                    if let Page::Uniform(c) = self.pages[i] { if matches!(k, Sorted::Uniform(u) if u == c) { continue; } }
                    let page = if matches!(k, Sorted::Full) { self.whole_page(i, rect, src) } else { Vec::new() };
                    self.set_page(device, queue, up, i, &page, k);
                    continue;
                }
                // A two-color page painted with its two colors (ink on
                // paper): its bytes there, nothing sorted again. Else it
                // is made full, and the part written as for a full page.
                if !whole {
                    if let Page::Mono { a, b, slot } = self.pages[i] {
                        let mut ks = Vec::with_capacity((lx1 - lx0) * (ly1 - ly0));
                        let fits = (ly0..ly1).all(|y| (lx0..lx1).all(|x| mix_of(a, b, at(x, y)).map(|k| ks.push(k)).is_some()));
                        if fits {
                            let (ox, oy) = self.mono.origin(slot);
                            up.write(device, queue, &self.mono.texture, (ox + lx0 as u32, oy + ly0 as u32), ((lx1 - lx0) as u32, (ly1 - ly0) as u32), &ks);
                            self.mono.mark(slot, Rect::new(lx0 as i32, ly0 as i32, lx1 as i32, ly1 as i32));
                            continue;
                        }
                        self.mono_to_full(device, queue, up, i);
                    }
                }
                if let (false, Page::Full { slot }) = (whole, &self.pages[i]) {
                    // A full page: the part written as it is.
                    let slot = *slot;
                    let mut block = Vec::with_capacity((lx1 - lx0) * (ly1 - ly0));
                    for y in ly0..ly1 { for x in lx0..lx1 { block.push(at(x, y)); } }
                    let (ox, oy) = self.rgba.origin(slot);
                    up.write(device, queue, &self.rgba.texture, (ox + lx0 as u32, oy + ly0 as u32), ((lx1 - lx0) as u32, (ly1 - ly0) as u32), block.as_flattened());
                    self.rgba.mark(slot, Rect::new(lx0 as i32, ly0 as i32, lx1 as i32, ly1 as i32));
                    continue;
                }
                // All of the page known (sent whole, or one color): sorted
                // again.
                let fill = match self.pages[i] { Page::Uniform(c) if !whole => c, _ => [0; 4] };
                let mut page = vec![fill; N];
                for y in ly0..ly1 { for x in lx0..lx1 { page[y * PAGE as usize + x] = at(x, y); } }
                self.pad(i, &mut page);
                // (Unchanged: nothing to do.)
                if let Page::Uniform(c) = self.pages[i] { if page.iter().all(|q| *q == c) { continue; } }
                let sorted = sort(&page);
                self.set_page(device, queue, up, i, &page, sorted);
            }
        }
        if !outer { up.release(queue); }
    }

    /// The pages `rect` touches made full (a transform's preview draws into
    /// them): their canvas rect and place in the RGBA atlas.
    pub(crate) fn full_pages(&mut self, device: &Device, queue: &Queue, up: &mut Uploads, rect: Rect) -> Vec<FullPage> {
        let rect = rect.intersect(&self.frame());
        let mut out = Vec::new();
        if rect.is_empty() { return out; }
        let p = PAGE as i32;
        let ids: Vec<usize> = (rect.y0.div_euclid(p)..(rect.y1 - 1).div_euclid(p) + 1)
            .flat_map(|py| (rect.x0.div_euclid(p)..(rect.x1 - 1).div_euclid(p) + 1).map(move |px| (py, px)))
            .map(|(py, px)| (py as u32 * self.cols + px as u32) as usize).collect();
        for &i in &ids {
            match self.pages[i] {
                Page::Uniform(c) => self.set_page(device, queue, up, i, &vec![c; N], Sorted::Full),
                Page::Mono { .. } => self.mono_to_full(device, queue, up, i),
                Page::Full { .. } => continue,
            }
            if !self.lifted.contains(&i) { self.lifted.push(i); }
        }
        // (Places taken once all are made: the atlas may have grown.)
        for &i in &ids {
            if let Page::Full { slot } = self.pages[i] { out.push((self.page_rect(i), self.rgba.origin(slot), slot)); }
        }
        out
    }

    /// The transform's preview is over: the pages it made full that still
    /// are, as canvas rects. Sent again whole, they are sorted again (else
    /// they keep 4 bytes a pixel until a whole merge).
    pub(crate) fn take_lifted(&mut self) -> Vec<Rect> {
        let frame = self.frame();
        std::mem::take(&mut self.lifted).into_iter().filter(|&i| matches!(self.pages[i], Page::Full { .. })).map(|i| self.page_rect(i).intersect(&frame)).collect()
    }

    /// The RGBA atlas (the transform preview's target).
    pub(crate) fn rgba_texture(&self) -> &Texture { &self.rgba.texture }

    /// A transform's preview drew over `r` of the full page in `slot`.
    pub(crate) fn mark_full(&mut self, slot: u32, r: Rect) { self.rgba.mark(slot, r); }

    /// With or without mipmaps (the pixels are kept).
    pub(crate) fn set_mipmaps(&mut self, device: &Device, queue: &Queue, on: bool) {
        let levels = if on { PAGE_LEVELS } else { 1 };
        if levels == self.rgba.levels { return; }
        self.rgba.set_levels(device, queue, levels);
        self.mono.set_levels(device, queue, levels);
    }

    /// Before a frame: the atlases shrunk if mostly free, the table sent,
    /// the mipmaps rebuilt where pages changed.
    /// `shrink`: not while a transform's preview draws in `enc` (a shrunk
    /// atlas is a copy made before it).
    pub(crate) fn prepare(&mut self, device: &Device, queue: &Queue, enc: &mut CommandEncoder, shrink: bool) {
        for mono in [false, true] {
            if !shrink { break; }
            let atlas = if mono { &mut self.mono } else { &mut self.rgba };
            let Some(map) = atlas.shrink(device, queue) else { continue };
            for p in &mut self.pages {
                match p {
                    Page::Mono { slot, .. } if mono => *slot = map[slot],
                    Page::Full { slot } if !mono => *slot = map[slot],
                    _ => {}
                }
            }
            // (The moved pages' levels were copied.)
        }
        // (A grown atlas moved its pages too.)
        for i in 0..self.pages.len() { self.set_entry(i); }
        if std::mem::take(&mut self.table_dirty) {
            queue.write_texture(
                TexelCopyTextureInfo { texture: &self.table, mip_level: 0, origin: Origin3d::ZERO, aspect: TextureAspect::All },
                bytemuck::cast_slice(&self.entries),
                TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.cols * 16), rows_per_image: Some(self.rows) },
                Extent3d { width: self.cols, height: self.rows, depth_or_array_layers: 1 },
            );
        }
        self.rgba.mip.rebuild(enc, &self.rgba.texture);
        self.mono.mip.rebuild(enc, &self.mono.texture);
    }
}

/// A two-color page drawn into an RGBA slot: pixel = a + (b − a)·t/255,
/// rounded as `sort` gives them back.
const EXPAND: &str = r#"
@group(0) @binding(0) var m: texture_2d<f32>;
// shift: R8 atlas pixel − RGBA atlas pixel.
struct E { shift: vec4<i32>, a: vec4<i32>, b: vec4<i32> }
@group(0) @binding(1) var<uniform> e: E;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let k = i32(round(textureLoad(m, vec2<i32>(pos.xy) + e.shift.xy, 0).r * 255.0));
    let d = e.b - e.a;
    let r = select(vec4(-127), vec4(127), d >= vec4(0));
    return vec4<f32>(e.a + (d * k + r) / 255) / 255.0;
}
"#;

struct Expand {
    pipe: RenderPipeline,
    bgl:  BindGroupLayout,
}

impl Expand {
    fn new(device: &Device) -> Self {
        let module = device.create_shader_module(ShaderModuleDescriptor { label: Some("expand"), source: ShaderSource::Wgsl(EXPAND.into()) });
        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("expand"),
            entries: &[
                BindGroupLayoutEntry { binding: 0, visibility: ShaderStages::FRAGMENT, ty: BindingType::Texture { sample_type: TextureSampleType::Float { filterable: false }, view_dimension: TextureViewDimension::D2, multisampled: false }, count: None },
                BindGroupLayoutEntry { binding: 1, visibility: ShaderStages::FRAGMENT, ty: BindingType::Buffer { ty: BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor { label: Some("expand"), bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("expand"),
            layout: Some(&layout),
            vertex: VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(ColorTargetState { format: TextureFormat::Rgba8Unorm, blend: None, write_mask: ColorWrites::ALL })] }),
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipe, bgl }
    }

    /// The page at `shift` from `to` in the R8 atlas, into the RGBA page
    /// at `to` (level 0).
    #[allow(clippy::too_many_arguments)]
    fn draw(&self, device: &Device, enc: &mut CommandEncoder, (mono, rgba): (&Texture, &Texture), to: (u32, u32), shift: (i32, i32), a: Rgba8, b: Rgba8) {
        use wgpu::util::DeviceExt;
        let level0 = TextureViewDescriptor { base_mip_level: 0, mip_level_count: Some(1), ..Default::default() };
        let (src, dst) = (mono.create_view(&level0), rgba.create_view(&level0));
        let v: [i32; 12] = [shift.0, shift.1, 0, 0, a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32, b[0] as i32, b[1] as i32, b[2] as i32, b[3] as i32];
        let u = device.create_buffer_init(&util::BufferInitDescriptor { label: Some("expand"), contents: bytemuck::cast_slice(&v), usage: BufferUsages::UNIFORM });
        let bind = device.create_bind_group(&BindGroupDescriptor {
            label: Some("expand"), layout: &self.bgl,
            entries: &[
                BindGroupEntry { binding: 0, resource: BindingResource::TextureView(&src) },
                BindGroupEntry { binding: 1, resource: u.as_entire_binding() },
            ],
        });
        let mut pass = enc.begin_render_pass(&RenderPassDescriptor {
            label: Some("expand"),
            color_attachments: &[Some(RenderPassColorAttachment { view: &dst, depth_slice: None, resolve_target: None, ops: Operations { load: LoadOp::Load, store: StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_viewport(to.0 as f32, to.1 as f32, PAGE as f32, PAGE as f32, 0.0, 1.0);
        pass.set_scissor_rect(to.0, to.1, PAGE, PAGE);
        pass.set_pipeline(&self.pipe);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_sorted_into_the_smallest_kind() {
        assert!(matches!(sort(&vec![[255; 4]; N]), Sorted::Uniform([255, 255, 255, 255])));
        // Ink on paper, anti-aliased: two colors.
        let mut px = vec![[255, 255, 255, 255]; N];
        for (k, p) in px.iter_mut().enumerate().take(300) { let v = (k % 256) as u8; *p = [v, v, v, 255]; }
        let Sorted::Mono { a, b, t } = sort(&px) else { panic!("two colors") };
        assert_eq!(expand(a, b, &t), px);
        // Red ink on paper, soft: still two colors.
        let mut px = vec![[255, 255, 255, 255]; N];
        for (k, p) in px.iter_mut().enumerate().take(256) { let s = k as i32; *p = [255, (255 - s) as u8, (255 - s) as u8, 255]; }
        let Sorted::Mono { a, b, t } = sort(&px) else { panic!("two colors") };
        for (g, w) in expand(a, b, &t).iter().zip(&px) { for c in 0..4 { assert!((g[c] as i32 - w[c] as i32).abs() <= 1); } }
        // A third color: full.
        px[1000] = [0, 0, 255, 255];
        assert!(matches!(sort(&px), Sorted::Full));
    }
}
