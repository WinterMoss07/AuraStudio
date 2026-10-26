//! Lucide icons (ISC, see icons/LICENSE-lucide), white, tinted when drawn.

use std::cell::RefCell;
use std::collections::HashMap;

use egui::{vec2, Color32, ImageSource, Rect, Ui};

use crate::atlas::{Atlas, Region};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // Toolbox.
    Brush, Eraser, Picker, Fill, Lasso, RectSelect,
    Line, Rectangle, Ellipse, Arc,
    // Layers docker.
    NewLayer, NewFolder, Duplicate, Delete,
    Eye, EyeClosed, Lock, LockOpen, Folder, FolderOpen, Collapsed, Expanded,
    Clip, Reference, Draft, FilterLayer, Label, FilterBy, Close,
    // Toolbar.
    FileNew, FileOpen, Save, Copy, Paste,
    // Tools docker: the brush mediums.
    Pencil, Ink, Charcoal, Paint, Watercolor, Airbrush, Blend, Effects, Misc, Plus,
    // Toolbox groups.
    Shapes,
    // Selection and transform tools.
    EllipseSelect, PolySelect, Wand, Transform,
    SelReplace, SelAdd, SelSubtract, SelIntersect,
    Perspective, Warp, RotateLeft, RotateRight, FlipH, FlipV,
    // Animation.
    Film, Minus, Camera,
    Play, Pause, SkipBack, SkipForward, StepBack, StepForward, Repeat, InsertFrame, RemoveFrame,
    SwitchView, Folders,
}

impl Icon {
    pub fn source(self) -> ImageSource<'static> {
        use egui::include_image as svg;
        match self {
            Icon::Brush      => svg!("../icons/brush.svg"),
            Icon::Eraser     => svg!("../icons/eraser.svg"),
            Icon::Picker     => svg!("../icons/pipette.svg"),
            Icon::Fill       => svg!("../icons/paint-bucket.svg"),
            Icon::Lasso      => svg!("../icons/lasso.svg"),
            Icon::RectSelect => svg!("../icons/square-dashed.svg"),
            Icon::Line       => svg!("../icons/slash.svg"),
            Icon::Rectangle  => svg!("../icons/square.svg"),
            Icon::Ellipse    => svg!("../icons/circle.svg"),
            Icon::Arc        => svg!("../icons/spline.svg"),
            Icon::FileNew    => svg!("../icons/file-plus.svg"),
            Icon::FileOpen   => svg!("../icons/folder-open.svg"),
            Icon::Save       => svg!("../icons/save.svg"),
            Icon::Copy       => svg!("../icons/copy.svg"),
            Icon::Paste      => svg!("../icons/clipboard-paste.svg"),
            Icon::NewLayer   => svg!("../icons/layers-plus.svg"),
            Icon::NewFolder  => svg!("../icons/folder-plus.svg"),
            Icon::Duplicate  => svg!("../icons/copy.svg"),
            Icon::Delete     => svg!("../icons/trash-2.svg"),
            Icon::Eye        => svg!("../icons/eye.svg"),
            Icon::EyeClosed  => svg!("../icons/eye-closed.svg"),
            Icon::Lock       => svg!("../icons/lock.svg"),
            Icon::LockOpen   => svg!("../icons/lock-open.svg"),
            Icon::Folder     => svg!("../icons/folder.svg"),
            Icon::FolderOpen => svg!("../icons/folder-open.svg"),
            Icon::Collapsed  => svg!("../icons/chevron-right.svg"),
            Icon::Expanded   => svg!("../icons/chevron-down.svg"),
            Icon::Clip       => svg!("../icons/link.svg"),
            Icon::Reference  => svg!("../icons/lighthouse.svg"),
            Icon::Draft      => svg!("../icons/pencil-line.svg"),
            Icon::FilterLayer => svg!("../icons/sliders-horizontal.svg"),
            Icon::Label      => svg!("../icons/tag.svg"),
            Icon::FilterBy   => svg!("../icons/funnel.svg"),
            Icon::Close      => svg!("../icons/x.svg"),
            Icon::Pencil     => svg!("../icons/pencil.svg"),
            Icon::Ink        => svg!("../icons/pen-tool.svg"),
            Icon::Charcoal   => svg!("../icons/charcoal.svg"),
            Icon::Paint      => svg!("../icons/paintbrush.svg"),
            Icon::Watercolor => svg!("../icons/droplets.svg"),
            Icon::Airbrush   => svg!("../icons/spray-can.svg"),
            Icon::Blend      => svg!("../icons/blend.svg"),
            Icon::Effects    => svg!("../icons/sparkles.svg"),
            Icon::Misc       => svg!("../icons/ellipsis.svg"),
            Icon::Plus       => svg!("../icons/plus.svg"),
            Icon::Shapes     => svg!("../icons/shapes.svg"),
            Icon::EllipseSelect => svg!("../icons/circle-dashed.svg"),
            Icon::PolySelect => svg!("../icons/vector-polygon.svg"),
            Icon::Wand       => svg!("../icons/wand-sparkles.svg"),
            Icon::Transform  => svg!("../icons/scaling.svg"),
            Icon::SelReplace => svg!("../icons/square-dashed.svg"),
            Icon::SelAdd     => svg!("../icons/squares-unite.svg"),
            Icon::SelSubtract => svg!("../icons/squares-subtract.svg"),
            Icon::SelIntersect => svg!("../icons/squares-intersect.svg"),
            Icon::Perspective => svg!("../icons/vector-square.svg"),
            Icon::Warp       => svg!("../icons/grid-3x3.svg"),
            Icon::RotateLeft => svg!("../icons/rotate-ccw-square.svg"),
            Icon::RotateRight => svg!("../icons/rotate-cw-square.svg"),
            Icon::FlipH      => svg!("../icons/flip-horizontal.svg"),
            Icon::FlipV      => svg!("../icons/flip-vertical.svg"),
            Icon::Film       => svg!("../icons/film.svg"),
            Icon::Minus      => svg!("../icons/minus.svg"),
            Icon::Camera     => svg!("../icons/video.svg"),
            Icon::Play       => svg!("../icons/play.svg"),
            Icon::Pause      => svg!("../icons/pause.svg"),
            Icon::SkipBack   => svg!("../icons/skip-back.svg"),
            Icon::SkipForward => svg!("../icons/skip-forward.svg"),
            Icon::StepBack   => svg!("../icons/chevron-left.svg"),
            Icon::StepForward => svg!("../icons/chevron-right.svg"),
            Icon::Repeat     => svg!("../icons/repeat.svg"),
            Icon::InsertFrame => svg!("../icons/between-horizontal-start.svg"),
            Icon::RemoveFrame => svg!("../icons/between-horizontal-end.svg"),
            Icon::SwitchView => svg!("../icons/refresh-cw.svg"),
            Icon::Folders    => svg!("../icons/folders.svg"),
        }
    }
}

/// The icons' pictures: one shared atlas, each icon drawn once per size
/// (and per look) instead of a texture each.
struct Icons {
    atlas:   Atlas,
    regions: HashMap<(Icon, u32, bool), Region>,
    /// An icon the atlas had no room for, and the pass to try again at
    /// (never for good: room comes back, and rasterizing every frame
    /// would cost).
    failed:  HashMap<(Icon, u32, bool), u64>,
    /// Screen scale the sizes in `regions` were made for.
    ppp:     f32,
    /// Last pass the atlas was tidied in.
    tidied:  u64,
}

/// Passes before an icon the atlas had no room for is tried again.
const RETRY_AFTER: u64 = 60;
/// Pages the icons may take (256 KB, then 1 MB each).
const MAX_PAGES: usize = 4;

impl Icons {
    fn new() -> Self {
        Self { atlas: Atlas::new("icons", egui::TextureOptions::LINEAR, MAX_PAGES), regions: HashMap::new(), failed: HashMap::new(), ppp: 0.0, tidied: 0 }
    }

    /// Everything made for another screen scale goes (a window moved from
    /// a 150 % screen to a 200 % one must not keep both).
    fn set_scale(&mut self, ppp: f32) {
        if (self.ppp - ppp).abs() > 1e-3 {
            *self = Self::new();
            self.ppp = ppp;
        }
    }
}

thread_local! {
    static ICONS: RefCell<Icons> = RefCell::new(Icons::new());
}

/// The SVG text of `icon`: whole, or with a thin stroke (its core).
fn svg_bytes(icon: Icon, core: bool) -> Vec<u8> {
    match icon.source() {
        ImageSource::Bytes { bytes, .. } => {
            if core { thin_stroke(&String::from_utf8_lossy(bytes.as_ref()), 0.8).into_bytes() } else { bytes.as_ref().to_vec() }
        }
        _ => Vec::new(),
    }
}

/// Draw `icon`, `size` points wide, centered in `rect`.
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, size: f32, tint: Color32) {
    draw(ui, icon, rect, size, tint, false);
}

fn draw(ui: &Ui, icon: Icon, rect: Rect, size: f32, tint: Color32, core: bool) {
    let ctx = ui.ctx();
    let ppp = ctx.pixels_per_point();
    let px = ((size * ppp).round() as u32).clamp(4, 256);
    let pass = ctx.cumulative_pass_nr();
    let key = (icon, px, core);
    let found = ICONS.with(|i| {
        let i = &mut *i.borrow_mut();
        i.set_scale(ppp);
        if i.tidied != pass { i.tidied = pass; i.atlas.tidy(); }
        if let Some(r) = i.regions.get(&key) { return Some(i.atlas.image(*r)); }
        if i.failed.get(&key).is_some_and(|retry| pass < *retry) { return None; }
        let image = egui_extras::image::load_svg_bytes_with_size(&svg_bytes(icon, core), Some(egui::load::SizeHint::Size(px, px)));
        let Ok(image) = image else {
            log::warn!("icon {icon:?} at {px} px does not rasterize");
            i.failed.insert(key, u64::MAX);
            return None;
        };
        match i.atlas.alloc(ctx, image.size[0], image.size[1]) {
            Some(r) => {
                i.atlas.write(r, image);
                i.failed.remove(&key);
                i.regions.insert(key, r);
                Some(i.atlas.image(r))
            }
            None => {
                if !i.failed.contains_key(&key) { log::warn!("icon atlas full: {icon:?} at {px} px waits"); }
                i.failed.insert(key, pass + RETRY_AFTER);
                None
            }
        }
    });
    if let Some((id, uv)) = found {
        ui.painter().image(id, Rect::from_center_size(rect.center(), vec2(size, size)), uv, tint);
    }
}

/// `svg` with its stroke-width set to `w`.
fn thin_stroke(svg: &str, w: f32) -> String {
    const KEY: &str = "stroke-width=\"";
    match svg.find(KEY) {
        Some(i) => {
            let start = i + KEY.len();
            let end = start + svg[start..].find('"').unwrap_or(0);
            format!("{}{w}{}", &svg[..start], &svg[end..])
        }
        None => svg.to_owned(),
    }
}

/// Draw `icon` in `color` with a soft inner glow: a lighter, thin core
/// along its strokes. `glow` 0..1 (0.5: felt, not looked at).
pub fn paint_glow(ui: &Ui, icon: Icon, rect: Rect, size: f32, color: Color32, glow: f32) {
    paint(ui, icon, rect, size, color);
    let lift = |c: u8| (c as f32 + (255.0 - c as f32) * 0.65) as u8;
    let core = Color32::from_rgba_unmultiplied(lift(color.r()), lift(color.g()), lift(color.b()), (glow.clamp(0.0, 1.0) * 150.0 * color.a() as f32 / 255.0) as u8);
    draw(ui, icon, rect, size, core, true);
}

/// A lock, unmistakable either way: locked is a closed sky-blue lock
/// (the clipping chain's blue) with an inner glow and a faint filled
/// body; locked by a parent, the same, dimmer; unlocked, an open lock,
/// faint and without glow. `rect` is the hit box (about 22 px).
pub fn paint_lock(ui: &Ui, rect: Rect, locked: bool, by_parent: bool, hovered: bool, on_accent: bool) {
    use crate::theme;
    if locked || by_parent {
        let k = if locked { 1.0 } else { 0.6 };
        // The body of the lock (lucide: x 3..21, y 11..22 of 24).
        let u = 14.0 / 24.0;
        let c = rect.center();
        let body = Rect::from_min_max(c + vec2(-9.0 * u + 0.6, 0.6 - u), c + vec2(9.0 * u - 0.6, 10.0 * u - 0.6));
        ui.painter().rect_filled(body, 1.5, theme::CLIP_BLUE.gamma_multiply(0.18 * k));
        paint_glow(ui, Icon::Lock, rect, 14.0, theme::CLIP_BLUE.gamma_multiply(k), 0.55 * k);
    } else {
        let tint = if hovered { theme::TEXT_WEAK } else if on_accent { Color32::from_white_alpha(120) } else { theme::TEXT_WEAK.gamma_multiply(0.45) };
        paint(ui, Icon::LockOpen, rect, 14.0, tint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used_regions() -> usize { ICONS.with(|i| i.borrow().regions.len()) }

    /// Every icon rasterizes, to pixels that are not all empty.
    #[test]
    fn every_icon_draws_something_at_each_size() {
        for px in [14u32, 17, 22] {
            for icon in all() {
                let image = egui_extras::image::load_svg_bytes_with_size(&svg_bytes(icon, false), Some(egui::load::SizeHint::Size(px, px))).unwrap_or_else(|e| panic!("{icon:?}: {e}"));
                assert!(image.pixels.iter().any(|p| p.a() > 0), "{icon:?} at {px}");
                let core = egui_extras::image::load_svg_bytes_with_size(&svg_bytes(icon, true), Some(egui::load::SizeHint::Size(px, px))).unwrap();
                assert!(core.pixels.iter().any(|p| p.a() > 0));
            }
        }
    }

    /// Icons painted twice take one place; a new size takes another; the
    /// whole set fits the atlas.
    #[test]
    fn an_icon_is_made_once_per_size_in_a_shared_atlas() {
        let ctx = egui::Context::default();
        let mut seen = (0, 0);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(20.0, 20.0));
                paint(ui, Icon::Eye, r, 14.0, Color32::WHITE);
                let a = used_regions();
                paint(ui, Icon::Eye, r, 14.0, Color32::RED);
                let b = used_regions();
                paint(ui, Icon::Eye, r, 16.0, Color32::WHITE);
                let c = used_regions();
                for icon in all() { paint(ui, icon, r, 14.0, Color32::WHITE); }
                seen = (b - a, c - b);
                assert!(ICONS.with(|i| i.borrow().atlas.pages()) <= 2);
            });
        });
        assert_eq!(seen, (0, 1));
    }

    fn all() -> Vec<Icon> {
        use Icon::*;
        vec![Brush, Eraser, Picker, Fill, Lasso, RectSelect, Line, Rectangle, Ellipse, Arc, NewLayer, NewFolder, Duplicate, Delete,
            Eye, EyeClosed, Lock, LockOpen, Folder, FolderOpen, Collapsed, Expanded, Clip, Reference, Draft, FilterLayer, Label, FilterBy,
            Close, FileNew, FileOpen, Save, Copy, Paste, Pencil, Ink, Charcoal, Paint, Watercolor, Airbrush, Blend,
            Effects, Misc, Plus, Shapes, EllipseSelect, PolySelect, Wand, Transform, SelReplace, SelAdd, SelSubtract, SelIntersect,
            Perspective, Warp, RotateLeft, RotateRight, FlipH, FlipV, Film, Minus, Camera,
            Play, Pause, SkipBack, SkipForward, StepBack, StepForward, Repeat, InsertFrame, RemoveFrame, SwitchView, Folders]
    }
}

#[cfg(test)]
mod more_tests {
    use super::*;

    fn used_regions() -> usize { ICONS.with(|i| i.borrow().regions.len()) }

    fn frame(ctx: &egui::Context, f: impl FnOnce(&Ui)) {
        let mut f = Some(f);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| { if let Some(f) = f.take() { f(ui); } });
        });
    }

    fn r() -> Rect { Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(20.0, 20.0)) }

    /// A full atlas hides an icon only for a while: when room comes back it shows.
    #[test]
    fn a_full_atlas_never_hides_an_icon_for_good() {
        let ctx = egui::Context::default();
        ICONS.with(|i| { *i.borrow_mut() = Icons { atlas: Atlas::new("icons", egui::TextureOptions::LINEAR, 1), ..Icons::new() }; i.borrow_mut().ppp = ctx.pixels_per_point(); });
        // Take the whole first page.
        let filler = ICONS.with(|i| i.borrow_mut().atlas.alloc(&ctx, 250, 250)).expect("the page");
        frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
        assert_eq!(used_regions(), 0, "no room");
        assert_eq!(ICONS.with(|i| i.borrow().failed.len()), 1);
        // Room comes back; after the wait it shows (not before: no work every frame).
        ICONS.with(|i| i.borrow_mut().atlas.free(filler));
        frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
        assert_eq!(used_regions(), 0, "still waiting");
        for _ in 0..RETRY_AFTER + 2 { frame(&ctx, |_| {}); }
        frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
        assert_eq!(used_regions(), 1, "back");
        assert!(ICONS.with(|i| i.borrow().failed.is_empty()));
    }

    /// Another screen scale starts the atlas over: sizes of the old one are gone.
    #[test]
    fn a_new_scale_resets_the_icon_atlas() {
        let ctx = egui::Context::default();
        ICONS.with(|i| *i.borrow_mut() = Icons::new());
        ctx.set_pixels_per_point(1.5);
        frame(&ctx, |ui| for icon in [Icon::Eye, Icon::Lock, Icon::Brush] { paint(ui, icon, r(), 14.0, Color32::WHITE); });
        assert_eq!(used_regions(), 3);
        let keys: Vec<_> = ICONS.with(|i| i.borrow().regions.keys().map(|k| k.1).collect());
        assert!(keys.iter().all(|px| *px == 21), "{keys:?}");
        ctx.set_pixels_per_point(2.0);
        frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
        let keys: Vec<_> = ICONS.with(|i| i.borrow().regions.keys().map(|k| k.1).collect());
        assert_eq!(keys, [28], "only the new scale");
        // And back: the atlas does not grow with each move.
        for _ in 0..4 {
            ctx.set_pixels_per_point(1.5);
            frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
            ctx.set_pixels_per_point(2.0);
            frame(&ctx, |ui| paint(ui, Icon::Eye, r(), 14.0, Color32::WHITE));
        }
        assert_eq!(ICONS.with(|i| i.borrow().atlas.pages()), 1);
        assert_eq!(used_regions(), 1);
    }
}
