//! The pixels of paint layers: what may be painted, strokes (through
//! `raster`'s stroke, limited by a selection at the source), reading and
//! writing rects, the fill, clearing, undo patches, the canvas size.

use aurastudio_core::{Rect, Rgba8};
use aurastudio_raster::{Image, Mask, Patch, Stroke};

use crate::fill::Area;
use crate::node::{Footprint, Node};
use crate::stack::{siblings, LayerStack, MAX_MARGIN};

impl LayerStack {
    /// The paint layer a stroke on the active node goes to: the active
    /// node itself when it is a paint layer nothing locks. (In an
    /// animation folder `animation` decides: the sheet of now, the cel.)
    pub fn paint_id(&self) -> Option<u64> {
        match self.active_node()? {
            Node::Layer(l) if !self.is_locked(l.id) => Some(l.id),
            _ => None,
        }
    }

    pub fn can_paint(&self) -> bool { self.paint_id().is_some() }

    pub fn image(&self, id: u64) -> Option<&Image> { Some(&self.layer(id)?.surface) }

    pub fn image_mut(&mut self, id: u64) -> Option<&mut Image> { Some(&mut self.layer_mut(id)?.surface) }

    /// A stroke on paint layer `id` (by id: another may get selected
    /// while it is painted), limited to `limit` (a selection's coverage).
    /// Its `end()` gives the undo patch. `None` if `id` is no paint layer.
    pub fn begin_stroke<'s>(&'s mut self, id: u64, limit: Option<&'s Mask>) -> Option<Stroke<'s, Rgba8>> {
        Some(self.image_mut(id)?.begin_stroke(limit))
    }

    /// Erase the active layer's content (Delete key): transparent, white
    /// for the Paper. False on folders and locked layers.
    pub fn clear_active(&mut self) -> bool {
        let Some(id) = self.paint_id() else { return false };
        let Some(l) = self.layer_mut(id) else { return false };
        l.clear();
        true
    }

    /// Undo / redo of pixels on paint layer `id`: its tiles and the
    /// patch's trade places. False if the layer is gone or the patch
    /// doesn't fit it.
    pub fn swap_patch(&mut self, id: u64, patch: &mut Patch<Rgba8>) -> bool {
        match self.image_mut(id) { Some(s) => patch.swap(s), None => false }
    }

    /// Pixels of `rect` in paint layer `id`, rows tightly packed.
    pub fn layer_rect(&self, id: u64, rect: Rect) -> Option<Vec<Rgba8>> { Some(self.image(id)?.read(rect)) }

    /// Write `data` (rows of `rect`) into paint layer `id`. False if the
    /// layer is gone or the size is off.
    pub fn set_layer_rect(&mut self, id: u64, rect: Rect, data: &[Rgba8]) -> bool {
        if data.len() != rect.area() as usize { return false; }
        let Some(s) = self.image_mut(id) else { return false };
        s.write(rect, data);
        true
    }

    /// Paint `color` over (`behind`: behind) the pixels of `area` in paint
    /// layer `id` (the fill tool), as much as `sel` selects them. Returns
    /// what changed.
    pub fn fill_area(&mut self, id: u64, area: &Area, color: Rgba8, sel: Option<&Mask>, behind: bool) -> Option<Rect> {
        let r = area.bbox();
        if r.is_empty() { return None; }
        let mut cov = area.coverage(r);
        if let Some(m) = sel {
            for (c, s) in cov.iter_mut().zip(m.read(r)) { *c = aurastudio_core::math::mul255(*c, s); }
        }
        let img = self.image_mut(id)?;
        if behind { img.fill_mask_behind(r, &cov, color) } else { img.fill_mask(r, &cov, color) }
        Some(r)
    }

    /// Clear the selected pixels of paint layer `id` to its cleared color
    /// (a soft selection clears them partly). Returns what changed.
    pub fn clear_selected(&mut self, id: u64, sel: &Mask) -> Option<Rect> {
        let r = sel.pixel_bounds()?;
        let l = self.layer(id)?;
        let clear = Self::clear_pixel(l.background);
        let s = self.image_mut(id)?;
        for c in s.chunks(r, 256) {
            let m = sel.read(c);
            if m.iter().all(|v| *v == 0) { continue; }
            if m.iter().all(|v| *v == 255) { s.fill_rect(c, clear); continue; }
            let px: Vec<Rgba8> = s.read(c).into_iter().zip(&m).map(|(p, &k)| aurastudio_filters::mix(p, clear, k)).collect();
            s.write(c, &px);
        }
        Some(r)
    }

    /// What showing, hiding, removing or copying node `id` can change on
    /// the canvas: its painted area (a folder: its content's, hidden
    /// sheets included for the onion skin), and that of the nodes clipped
    /// onto it (removed, they would clip to another base). A layer with a
    /// visible fill (Paper) or a filter layer: everything.
    pub fn footprint(&self, id: u64) -> Footprint {
        let Some(path) = self.path(id) else { return Footprint::All };
        let Some((last, parent)) = path.split_last() else { return Footprint::All };
        let Some(nodes) = siblings(&self.root, parent) else { return Footprint::All };
        let mut f = nodes[*last].footprint();
        for n in nodes[*last + 1..].iter().take_while(|n| n.props().clip) { f = f.union(n.footprint()); }
        f
    }

    /// Change the frame, the content centered in it (Krita / Photoshop
    /// "canvas size", center anchor); what falls outside the new frame and
    /// its margin is cut.
    pub fn resize(&mut self, width: u32, height: u32) {
        let dx = (width as i64 - self.width as i64) / 2;
        let dy = (height as i64 - self.height as i64) / 2;
        let (dx, dy) = (dx as i32, dy as i32);
        self.walk_mut(|n| match n {
            Node::Layer(l) => l.surface = l.surface.resized(width, height, dx, dy),
            Node::Mask(m) => m.mask = m.mask.resized(width, height, dx, dy),
            _ => {}
        });
        self.width = width;
        self.height = height;
        self.crop_to_area();
    }

    /// Another margin (percent per side); what falls outside is cut.
    pub fn set_margin(&mut self, margin: u32) {
        self.margin = margin.min(MAX_MARGIN);
        self.crop_to_area();
    }

    /// Ids of every paint layer, bottom to top (thumbnails).
    pub fn layer_ids(&self) -> Vec<u64> {
        let mut v = Vec::new();
        for n in &self.root { n.push_layer_ids(&mut v); }
        v
    }
}
