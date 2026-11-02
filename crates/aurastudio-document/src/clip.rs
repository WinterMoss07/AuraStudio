//! Copy and paste of pixels.

use aurastudio_core::{math, Rect, Rgba8};

use crate::region::Done;
use crate::Document;

/// Pixels copied from a layer and where they were (canvas pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub rect: Rect,
    /// Straight RGBA rows of `rect`.
    pub px:   Vec<Rgba8>,
}

impl Document {
    /// The selected pixels of the active layer (outside the selection:
    /// transparent), or all it shows. `None` on a folder, or with nothing
    /// there to copy.
    pub fn copy(&self) -> Option<Clip> {
        let id = self.timeline.layer_of_active(&self.stack)?;
        let l = self.stack.layer(id)?;
        // Only where the layer has content (the Paper: the frame).
        let content = if l.background { Some(self.stack.frame()) } else { l.surface.pixel_bounds() }?;
        let rect = match &self.selection { Some(s) => content.intersect(&s.bbox()), None => content };
        if rect.is_empty() { return None; }
        let mut px = l.surface.read(rect);
        if let Some(sel) = &self.selection {
            for (p, c) in px.iter_mut().zip(sel.mask(rect)) { p[3] = math::mul255(p[3], c); }
        }
        Some(Clip { rect, px })
    }

    /// On a new layer above the active one (made active): one step, undone
    /// with its layer.
    pub(crate) fn paste(&mut self, clip: &Clip) -> Done {
        if clip.px.len() as u64 != clip.rect.area() || clip.rect.is_empty() { return Done::default(); }
        let id = self.stack.add_layer("Pasted");
        self.stack.set_active(id);
        if let Some(img) = self.stack.image_mut(id) {
            img.write(clip.rect, &clip.px);
            img.compact(clip.rect.snapped_out(img.tile_size()));
        }
        let mut done = Done { made: Some(id), ..Done::default() };
        done.region.add_rect(clip.rect);
        self.after_tree("Paste", done)
    }
}
