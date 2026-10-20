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
        }
    }
}

/// Draw `icon`, `size` points wide, centered in `rect`.
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, size: f32, tint: Color32) {
    egui::Image::new(icon.source())
        .tint(tint)
        .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
}
