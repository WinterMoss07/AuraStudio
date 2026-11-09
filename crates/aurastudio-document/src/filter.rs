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
    /// The filter the layer shows now, and the selection's stamp then
    /// (Apply keeps it without filtering again).
    last:   Option<(Filter, u64)>,
}

impl FilterSession {
    pub(crate) fn layer(&self) -> u64 { self.layer }
    pub(crate) fn before(&self) -> &Image { &self.before }
}

impl Document {
    /// Stopped (`stop`): nothing changes, the next preview comes.
    pub(crate) fn filter_preview(&mut self, filter: Option<&Filter>, stop: aurastudio_filters::StopFn<'_>) -> Done {
        if self.filter.is_none() {
            let Some(layer) = self.paint_id() else { return Done::refused("Layer is locked or is a folder") };
            let Some(before) = self.stack.image(layer).cloned() else { return Done::default() };
            self.filter = Some(FilterSession { layer, before, shown: Region::Nothing, last: None });
        }
        let stamp = self.selection.as_ref().map_or(0, Selection::stamp);
        let Some(s) = self.filter.as_mut() else { return Done::default() };
        let mut img = s.before.clone();
        let mut now = Region::Nothing;
        if let Some(f) = filter {
            let sel = self.selection.as_ref().map(Selection::as_mask);
            match f.apply_until(&mut img, sel, stop) {
                Ok(r) => if let Some(r) = r { now.add_rect(r); },
                Err(_) => return Done::default(),
            }
            if img.fill() != s.before.fill() { now = Region::Whole; }
        }
        s.last = filter.map(|f| (f.clone(), stamp));
        let mut region = std::mem::replace(&mut s.shown, now.clone());
        region.add(now);
        let layer = s.layer;
        if let Some(i) = self.stack.image_mut(layer) { *i = img; }
        Done { region, layers: vec![layer], ..Done::default() }
    }

    pub(crate) fn filter_apply(&mut self, filter: &Filter) -> Done {
        // Shown already with these values: kept as it is.
        let stamp = self.selection.as_ref().map_or(0, Selection::stamp);
        let shown = self.filter.as_ref().is_some_and(|s| s.last.as_ref().is_some_and(|(f, k)| f == filter && *k == stamp));
        let mut done = if shown { Done::default() } else { self.filter_preview(Some(filter), &|| false) };
        let Some(s) = self.filter.take() else { return done };
        if shown { done.layers.push(s.layer); }
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

