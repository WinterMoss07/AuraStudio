//! Lucide icons (ISC, see icons/LICENSE-lucide), white, tinted when drawn.

use egui::{vec2, Color32, ImageSource, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    // Toolbox.
    Brush, Eraser, Picker, Fill, Lasso, RectSelect,
    Line, Rectangle, Ellipse, Arc,
    // Layers docker.
    NewLayer, NewFolder, Duplicate, Delete,
    Eye, EyeClosed, Lock, LockOpen, Folder, FolderOpen, Collapsed, Expanded,
    Clip, Reference, Draft, FilterLayer, Label, FilterBy, Group, Ungroup, Close,
    // Toolbar.
    FileNew, FileOpen, Save, Copy, Paste,
    // Tools docker: the brush mediums.
    Pencil, Ink, Charcoal, Paint, Watercolor, Airbrush, Blend, Effects, Misc, Plus,
    // Toolbox groups.
    Shapes,
    // Selection and transform tools.
    EllipseSelect, PolySelect, Wand, Transform,
    SelReplace, SelAdd, SelSubtract, SelIntersect,
    Perspective, Warp, RotateLeft, RotateRight, FlipH, FlipV, Check, Move,
    // Animation.
    Film, Minus, ArrowUp, ArrowDown, Camera, Comics,
    Play, Pause, SkipBack, SkipForward, StepBack, StepForward, Repeat, XSheet, Timeline, InsertFrame, RemoveFrame,
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
            Icon::Group      => svg!("../icons/folder-input.svg"),
            Icon::Ungroup    => svg!("../icons/folder-output.svg"),
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
            Icon::Check      => svg!("../icons/check.svg"),
            Icon::Move       => svg!("../icons/move.svg"),
            Icon::Film       => svg!("../icons/film.svg"),
            Icon::Minus      => svg!("../icons/minus.svg"),
            Icon::ArrowUp    => svg!("../icons/arrow-up.svg"),
            Icon::ArrowDown  => svg!("../icons/arrow-down.svg"),
            Icon::Camera     => svg!("../icons/video.svg"),
            Icon::Comics     => svg!("../icons/book-open.svg"),
            Icon::Play       => svg!("../icons/play.svg"),
            Icon::Pause      => svg!("../icons/pause.svg"),
            Icon::SkipBack   => svg!("../icons/skip-back.svg"),
            Icon::SkipForward => svg!("../icons/skip-forward.svg"),
            Icon::StepBack   => svg!("../icons/chevron-left.svg"),
            Icon::StepForward => svg!("../icons/chevron-right.svg"),
            Icon::Repeat     => svg!("../icons/repeat.svg"),
            Icon::XSheet     => svg!("../icons/columns-2.svg"),
            Icon::Timeline   => svg!("../icons/rows-2.svg"),
            Icon::InsertFrame => svg!("../icons/between-horizontal-start.svg"),
            Icon::RemoveFrame => svg!("../icons/between-horizontal-end.svg"),
            Icon::SwitchView => svg!("../icons/refresh-cw.svg"),
            Icon::Folders    => svg!("../icons/folders.svg"),
        }
    }
}

/// Draw `icon`, `size` points wide, centered in `rect`.
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, size: f32, tint: Color32) {
    egui::Image::new(icon.source())
        .tint(tint)
        .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
}

/// A lock, unmistakable either way: locked is a closed lock on a filled
/// orange chip; locked by a parent, the closed lock in orange alone;
/// unlocked, an open lock, faint. `rect` is the hit box (about 22 px).
pub fn paint_lock(ui: &Ui, rect: Rect, locked: bool, by_parent: bool, hovered: bool, on_accent: bool) {
    use crate::theme;
    let p = ui.painter();
    if locked {
        p.rect_filled(rect.shrink2(vec2(2.0, 3.0)), 3.0, theme::LOCKED);
        paint(ui, Icon::Lock, rect, 14.0, Color32::from_rgb(0x1d, 0x1d, 0x1f));
    } else if by_parent {
        paint(ui, Icon::Lock, rect, 14.0, theme::LOCKED);
    } else {
        let tint = if hovered { theme::TEXT_WEAK } else if on_accent { Color32::from_white_alpha(120) } else { theme::TEXT_WEAK.gamma_multiply(0.45) };
        paint(ui, Icon::LockOpen, rect, 14.0, tint);
    }
}
