//! Interface icons: Lucide (https://lucide.dev, ISC license, see
//! icons/LICENSE-lucide), white in the SVG files and tinted when drawn, so
//! one file serves every state (selected, disabled...). The SVGs are
//! rasterized by egui_extras at the screen's resolution: sharp at any
//! interface scale.

use egui::{vec2, Color32, ImageSource, Rect, Ui};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    // Toolbox.
    Brush, Eraser, Picker, Fill, Lasso, RectSelect,
    Line, Rectangle, Ellipse, Arc,
    // Layers docker.
    NewLayer, NewFolder, Duplicate, Up, Down, Delete,
    Eye, EyeClosed, Lock, LockOpen, Folder, FolderOpen, Collapsed, Expanded,
    // Toolbar.
    FileNew, FileOpen, Save, Copy, Paste,
    // Brushes docker: the mediums.
    Pencil, Ink, Charcoal, Paint, Watercolor, Airbrush, Blend, Effects, Misc,
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
            Icon::Up         => svg!("../icons/arrow-up.svg"),
            Icon::Down       => svg!("../icons/arrow-down.svg"),
            Icon::Delete     => svg!("../icons/trash-2.svg"),
            Icon::Eye        => svg!("../icons/eye.svg"),
            Icon::EyeClosed  => svg!("../icons/eye-closed.svg"),
            Icon::Lock       => svg!("../icons/lock.svg"),
            Icon::LockOpen   => svg!("../icons/lock-open.svg"),
            Icon::Folder     => svg!("../icons/folder.svg"),
            Icon::FolderOpen => svg!("../icons/folder-open.svg"),
            Icon::Collapsed  => svg!("../icons/chevron-right.svg"),
            Icon::Expanded   => svg!("../icons/chevron-down.svg"),
            Icon::Pencil     => svg!("../icons/pencil.svg"),
            Icon::Ink        => svg!("../icons/pen-tool.svg"),
            Icon::Charcoal   => svg!("../icons/charcoal.svg"),
            Icon::Paint      => svg!("../icons/paintbrush.svg"),
            Icon::Watercolor => svg!("../icons/droplets.svg"),
            Icon::Airbrush   => svg!("../icons/spray-can.svg"),
            Icon::Blend      => svg!("../icons/blend.svg"),
            Icon::Effects    => svg!("../icons/sparkles.svg"),
            Icon::Misc       => svg!("../icons/shapes.svg"),
        }
    }
}

/// Draw `icon`, `size` points wide, centered in `rect`.
pub fn paint(ui: &Ui, icon: Icon, rect: Rect, size: f32, tint: Color32) {
    egui::Image::new(icon.source())
        .tint(tint)
        .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
}
