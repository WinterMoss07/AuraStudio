//! A filter shown on a layer while its dialog is open: the layer as it was
//! (shared tiles, no copy), filtered again with other values, put back
//! (Cancel) or kept as one step (Apply).

use aurastudio_filters::Filter;
use aurastudio_raster::Image;
use aurastudio_selection::Selection;

use crate::document::Before;
use crate::region::{Done, Region};
use crate::Document;

pub(crate) struct FilterSession {
    layer:  u64,
    before: Image,
    /// What the last preview changed (`None`: nothing; the fill changed:
    /// everywhere).
    shown:  Region,
}

impl FilterSession {
    pub(crate) fn layer(&self) -> u64 { self.layer }
    pub(crate) fn before(&self) -> &Image { &self.before }
}

impl Document {
    pub(crate) fn filter_preview(&mut self, filter: Option<&Filter>) -> Done {
        if self.filter.is_none() {
            let Some(layer) = self.paint_id() else { return Done::refused("Layer is locked or is a folder") };
            let Some(before) = self.stack.image(layer).cloned() else { return Done::default() };
            self.filter = Some(FilterSession { layer, before, shown: Region::Nothing });
        }
        let Some(s) = self.filter.as_mut() else { return Done::default() };
        let mut img = s.before.clone();
        let mut now = Region::Nothing;
        if let Some(f) = filter {
            let sel = self.selection.as_ref().map(Selection::as_mask);
            if let Some(r) = f.apply_to(&mut img, sel) { now.add_rect(r); }
            if img.fill() != s.before.fill() { now = Region::Whole; }
        }
        let mut region = std::mem::replace(&mut s.shown, now.clone());
        region.add(now);
        let layer = s.layer;
        if let Some(i) = self.stack.image_mut(layer) { *i = img; }
        Done { region, layers: vec![layer], ..Done::default() }
    }

    pub(crate) fn filter_apply(&mut self, filter: &Filter) -> Done {
        let mut done = self.filter_preview(Some(filter));
        let Some(s) = self.filter.take() else { return done };
        let before = Before(s.before.snapshot(), s.before.fill());
        let mut d = Done::default();
        if self.end_paint(filter.kind.name(), s.layer, before, &mut d) { done.other = true; }
        done
    }

    pub(crate) fn filter_cancel(&mut self) -> Done {
        let Some(s) = self.filter.take() else { return Done::default() };
        if let Some(i) = self.stack.image_mut(s.layer) { *i = s.before; }
        Done { region: s.shown, layers: vec![s.layer], ..Done::default() }
    }
}

