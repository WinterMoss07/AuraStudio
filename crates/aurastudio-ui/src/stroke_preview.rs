//! Stroke previews: the brush in use over Tool options and Advanced tool
//! options (a band, half on gray paper), and each brush of the brush list.
//! The app paints them (on a thread of its own, the list's kept on disk);
//! here they are asked for, kept in the texture cache while shown, drawn.
//!
//! A preview is a hint, not the stroke: small, made at one pixel per point
//! (smoothed on a HiDPI screen), asked again only when what it shows
//! changes (not the color, not a size past 100 px), at most ten times a
//! second while a slider moves, never for a panel being resized (the
//! picture keeps its size, centered), never while nothing shows it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use aurastudio_forge::Brush;
use egui::{pos2, vec2, Color32, FontId, Painter, Rect, Stroke};

use crate::atlas::{Pic, Region};
use crate::{theme, Actions};

/// Pictures, in pixels (one per point): Tool options' band, Advanced tool
/// options' (a cache page is 512 wide), a row of the brush list.
pub const TOOL_OPTIONS: (u32, u32) = (320, 38);
pub const ADVANCED: (u32, u32) = (504, 84);
pub const LIST: (u32, u32) = (240, 32);

/// A brush past this size (px) shows as this size: its preview no longer
/// changes.
pub const BIGGEST: f32 = 100.0;

/// The stroke's paint, and the paper of a band's left half.
pub const INK: [u8; 3] = [236, 236, 240];
pub const PAPER: [u8; 3] = [178, 178, 182];

/// Asked again at most this often while the brush changes.
const EVERY: Duration = Duration::from_millis(100);

/// A preview not drawn for this long is let go.
const UNSEEN: Duration = Duration::from_secs(2);

/// Where the brush in use is previewed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    ToolOptions,
    Advanced,
}

impl Slot {
    pub fn size(self) -> (u32, u32) { match self { Slot::ToolOptions => TOOL_OPTIONS, Slot::Advanced => ADVANCED } }
}

/// What the stroke draws: a brush's S, or a geometry tool's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PreviewShape {
    #[default]
    S,
    Line,
    Rectangle,
    Ellipse,
    Arc,
}

/// A picture the app is asked to paint.
#[derive(Clone, Debug)]
pub enum PreviewAsk {
    /// The brush in use (`brush`: tips loaded) for `slot`.
    Current { slot: Slot, brush: Box<Brush>, shape: PreviewShape },
    /// Row `index` of the brush list: the brush as saved (tips not loaded).
    List { index: usize, settings: Box<Brush> },
}

/// What a preview shows of a brush: it, without what changes nothing in
/// the picture (the color, the seed, the Tool options list, a size past
/// `BIGGEST`).
pub fn shown_of(b: &Brush) -> Brush {
    let mut n = Brush { color: [0.0, 0.0, 0.0, 1.0], seed: 0, tool_options: Vec::new(), ..b.clone() };
    if n.size > BIGGEST { n.resize_with_dual_brush(BIGGEST); }
    n
}

#[derive(Default)]
struct Current {
    /// What was last asked (as `shown_of` gives it), and when.
    asked:   Option<(Brush, PreviewShape)>,
    at:      Option<Instant>,
    pic:     Option<Pic>,
    pending: Option<egui::ColorImage>,
    seen:    Option<Instant>,
}

struct Row {
    /// The brush its picture shows.
    settings: Brush,
    pic:      Option<Pic>,
    seen:     Instant,
}

#[derive(Default)]
struct Previews {
    current: HashMap<Slot, Current>,
    rows:    HashMap<usize, Row>,
    /// Rows asked for, not back yet (the brush asked).
    asked:   HashMap<usize, Brush>,
    /// List pictures back, not in the cache yet.
    arrived: Vec<(usize, Brush, egui::ColorImage)>,
}

thread_local! {
    static PREVIEWS: RefCell<Previews> = RefCell::new(Previews::default());
}

fn with<R>(f: impl FnOnce(&mut Previews) -> R) -> R { PREVIEWS.with(|p| f(&mut p.borrow_mut())) }

/// The picture of `slot` painted (straight RGBA).
pub fn set_current(slot: Slot, w: u32, h: u32, rgba: &[u8]) {
    if rgba.len() != (w * h * 4) as usize { return; }
    let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba);
    with(|p| p.current.entry(slot).or_default().pending = Some(img));
}

/// The picture of row `index` (its alpha; `settings`: the brush it shows).
pub fn set_row(index: usize, settings: &Brush, w: u32, h: u32, alpha: &[u8]) {
    if alpha.len() != (w * h) as usize { return; }
    // (White: tinted with the ink when drawn.)
    let px = alpha.iter().map(|&a| Color32::from_white_alpha(a)).collect();
    let img = egui::ColorImage::new([w as usize, h as usize], px);
    with(|p| {
        if p.asked.get(&index) == Some(settings) { p.asked.remove(&index); }
        p.arrived.push((index, settings.clone(), img));
    });
}

/// Into the cache: in place of the picture before when it has its size,
/// else the old one freed first (its room serves the new one).
fn store(pic: &mut Option<Pic>, img: egui::ColorImage, ctx: &egui::Context) {
    let [w, h] = img.size;
    if let Some(r) = pic.as_ref().map(|p| p.0).filter(|r| (r.w as usize, r.h as usize) == (w, h)) {
        crate::atlas::ui(|a| a.write(r, img));
        return;
    }
    *pic = None;
    if let Some(r) = crate::atlas::ui(|a| a.alloc(ctx, w, h)) {
        crate::atlas::ui(|a| a.write(r, img));
        *pic = Some(Pic(r));
    }
}

/// The brush in use, in `rect` (its band): asked for when what it shows
/// changed, drawn with `name` in its corner.
pub fn current(painter: &Painter, rect: Rect, slot: Slot, brush: &Brush, shape: PreviewShape, name: &str, a: &mut Actions) {
    let ctx = painter.ctx().clone();
    let now = Instant::now();
    let region = with(|p| {
        let c = p.current.entry(slot).or_default();
        c.seen = Some(now);
        if let Some(img) = c.pending.take() { store(&mut c.pic, img, &ctx); }
        let shown = (shown_of(brush), shape);
        if c.asked.as_ref() != Some(&shown) {
            // (Ten a second at most: a slider dragged asks for the last.)
            match c.at.map(|t| now.duration_since(t)) {
                Some(d) if d < EVERY => ctx.request_repaint_after(EVERY - d),
                _ => {
                    a.previews.push(PreviewAsk::Current { slot, brush: Box::new(brush.clone()), shape });
                    c.asked = Some(shown);
                    c.at = Some(now);
                }
            }
        }
        c.pic.as_ref().map(|p| p.0)
    });
    band(painter, rect, region, slot.size(), true, name);
}

/// Row `index` of the brush list (`settings`: the brush as saved), in
/// `rect`: its picture asked for when it has none of this brush.
pub fn row(painter: &Painter, rect: Rect, index: usize, settings: &Brush, name: &str, a: &mut Actions) {
    let ctx = painter.ctx().clone();
    let now = Instant::now();
    let region = with(|p| {
        // Arrived pictures in (of the brush the row shows still).
        for (i, s, img) in std::mem::take(&mut p.arrived) {
            let r = p.rows.entry(i).or_insert_with(|| Row { settings: s.clone(), pic: None, seen: now });
            r.settings = s;
            store(&mut r.pic, img, &ctx);
        }
        let stale = p.rows.get(&index).is_none_or(|r| r.settings != *settings);
        if stale {
            p.rows.remove(&index);
            if p.asked.get(&index) != Some(settings) {
                a.previews.push(PreviewAsk::List { index, settings: Box::new(settings.clone()) });
                p.asked.insert(index, settings.clone());
            }
            return None;
        }
        let r = p.rows.get_mut(&index)?;
        r.seen = now;
        r.pic.as_ref().map(|p| p.0)
    });
    band(painter, rect, region, LIST, false, name);
}

/// Once a frame: previews not drawn for a while are let go (a closed
/// window, rows scrolled away): asked again when shown again.
pub fn tidy() {
    let now = Instant::now();
    with(|p| {
        p.current.retain(|_, c| c.seen.is_some_and(|t| now.duration_since(t) < UNSEEN));
        p.rows.retain(|_, r| now.duration_since(r.seen) < UNSEEN);
        // (Back for a list not drawn this frame: not needed now.)
        p.arrived.clear();
    });
}

/// Pictures held (the tests).
#[cfg(test)]
fn held() -> (usize, usize) { with(|p| (p.current.values().filter(|c| c.pic.is_some()).count(), p.rows.values().filter(|r| r.pic.is_some()).count())) }

/// A band: the desk, the picture at its own size centered (smaller when
/// the band is), in the detail mode the paper running on to the band's
/// left end; the name in the bottom right corner.
fn band(painter: &Painter, rect: Rect, pic: Option<Region>, size: (u32, u32), detail: bool, name: &str) {
    let p = painter.with_clip_rect(rect);
    p.rect_filled(rect, 2.0, theme::DESK);
    let full = vec2(size.0 as f32, size.1 as f32);
    let k = (rect.width() / full.x).min(rect.height() / full.y).min(1.0);
    let at = Rect::from_center_size(rect.center(), full * k);
    if detail {
        let [r, g, b] = PAPER;
        let paper = Color32::from_rgb(r, g, b);
        // Before the picture comes: its paper half alone.
        let end = if pic.is_some() { at.left() } else { at.center().x };
        p.rect_filled(Rect::from_min_max(pos2(rect.left(), at.top()), pos2(end, at.bottom())), 0.0, paper);
    }
    if let Some(reg) = pic {
        let [r, g, b] = INK;
        let tint = if detail { Color32::WHITE } else { Color32::from_rgb(r, g, b) };
        crate::atlas::paint(&p, reg, at, tint);
    }
    let font = FontId::proportional(11.0);
    let galley = p.layout_no_wrap(name.to_owned(), font, theme::TEXT);
    let s = galley.size() + vec2(8.0, 2.0);
    let tag = Rect::from_min_size(rect.right_bottom() - s - vec2(3.0, 3.0), s);
    p.rect_filled(tag, 2.0, Color32::from_black_alpha(150));
    p.galley(tag.left_top() + vec2(4.0, 1.0), galley, theme::TEXT);
    p.rect_stroke(rect, 2.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ctx: &egui::Context, f: impl FnOnce(&Painter)) {
        let mut f = Some(f);
        crate::test_pass(ctx, Default::default(), |root| {
            egui::CentralPanel::default().show(root, |ui| { if let Some(f) = f.take() { f(ui.painter()); } });
        });
    }

    fn rect() -> Rect { Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 38.0)) }

    /// Asked once for what it shows: not for another color, not for a
    /// size past 100 px, not twice within a tenth of a second.
    #[test]
    fn a_preview_is_asked_only_when_its_picture_changes() {
        let ctx = egui::Context::default();
        let mut a = Actions::default();
        let mut b = Brush { size: 20.0, ..Brush::default() };
        let ask = |b: &Brush, a: &mut Actions| frame(&ctx, |p| current(p, rect(), Slot::ToolOptions, b, PreviewShape::S, "Pen", a));
        ask(&b, &mut a);
        assert_eq!(a.previews.len(), 1);
        b.color = [1.0, 0.0, 0.0, 1.0];
        ask(&b, &mut a);
        assert_eq!(a.previews.len(), 1, "a color changes nothing");
        b.size = 30.0;
        ask(&b, &mut a);
        assert_eq!(a.previews.len(), 1, "too soon: asked later");
        std::thread::sleep(EVERY);
        ask(&b, &mut a);
        assert_eq!(a.previews.len(), 2, "the last size, once");
        b.size = 300.0;
        std::thread::sleep(EVERY);
        ask(&b, &mut a);
        b.size = 900.0;
        std::thread::sleep(EVERY);
        ask(&b, &mut a);
        assert_eq!(a.previews.len(), 3, "past 100 px the picture is the same");
    }

    /// Back, the picture is kept in place; not drawn for a while, let go.
    #[test]
    fn a_picture_is_kept_while_shown_and_let_go_after() {
        let ctx = egui::Context::default();
        let mut a = Actions::default();
        let b = Brush::default();
        let (w, h) = TOOL_OPTIONS;
        frame(&ctx, |p| current(p, rect(), Slot::ToolOptions, &b, PreviewShape::S, "Pen", &mut a));
        set_current(Slot::ToolOptions, w, h, &vec![200; (w * h * 4) as usize]);
        frame(&ctx, |p| current(p, rect(), Slot::ToolOptions, &b, PreviewShape::S, "Pen", &mut a));
        let first = with(|p| p.current[&Slot::ToolOptions].pic.as_ref().map(|p| p.0));
        assert!(first.is_some() && held().0 == 1);
        // Another picture of the same size: the same place.
        set_current(Slot::ToolOptions, w, h, &vec![100; (w * h * 4) as usize]);
        frame(&ctx, |p| current(p, rect(), Slot::ToolOptions, &b, PreviewShape::S, "Pen", &mut a));
        assert_eq!(with(|p| p.current[&Slot::ToolOptions].pic.as_ref().map(|p| p.0)), first);
        with(|p| p.current.get_mut(&Slot::ToolOptions).unwrap().seen = Some(Instant::now() - UNSEEN));
        tidy();
        assert_eq!(held().0, 0);
    }

    /// A row asks for its brush once; its picture shows that brush only.
    #[test]
    fn a_list_row_asks_once_and_follows_its_brush() {
        let ctx = egui::Context::default();
        let mut a = Actions::default();
        let b = Brush { size: 12.0, ..Brush::default() };
        let r = Rect::from_min_size(pos2(0.0, 0.0), vec2(250.0, 34.0));
        frame(&ctx, |p| row(p, r, 7, &b, "Pen", &mut a));
        frame(&ctx, |p| row(p, r, 7, &b, "Pen", &mut a));
        assert_eq!(a.previews.len(), 1);
        let (w, h) = LIST;
        set_row(7, &b, w, h, &vec![255; (w * h) as usize]);
        frame(&ctx, |p| row(p, r, 7, &b, "Pen", &mut a));
        assert_eq!(held().1, 1);
        // The brush changed: its old picture goes, the new one is asked.
        let b2 = Brush { size: 40.0, ..b };
        frame(&ctx, |p| row(p, r, 7, &b2, "Pen", &mut a));
        assert_eq!((held().1, a.previews.len()), (0, 2));
    }
}
