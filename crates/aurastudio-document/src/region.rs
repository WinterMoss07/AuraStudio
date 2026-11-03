//! What an edit changed on the canvas, and what it did.

use aurastudio_core::Rect;
use aurastudio_layers::Footprint;

/// Where the merged image may have changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Region {
    #[default]
    Nothing,
    /// These rects (they may overlap).
    Rects(Vec<Rect>),
    /// Anywhere (a filter layer, the Paper, a resize, a document opened).
    Whole,
}

impl Region {
    pub fn is_nothing(&self) -> bool { *self == Region::Nothing }

    pub fn add_rect(&mut self, r: Rect) {
        if r.is_empty() { return; }
        match self {
            Region::Nothing => *self = Region::Rects(vec![r]),
            Region::Rects(v) => v.push(r),
            Region::Whole => {}
        }
    }

    pub fn add(&mut self, o: Region) {
        match o {
            Region::Nothing => {}
            Region::Whole => *self = Region::Whole,
            Region::Rects(v) => for r in v { self.add_rect(r); },
        }
    }

    pub fn of_footprint(f: Footprint) -> Region {
        match f { Footprint::Nothing => Region::Nothing, Footprint::Area(r) => Region::Rects(vec![r]), Footprint::All => Region::Whole }
    }

    /// Around all of it (`None`: nothing; `Some(None)`: whole).
    pub fn bounds(&self) -> Option<Option<Rect>> {
        match self {
            Region::Nothing => None,
            Region::Rects(v) => Some(Some(v.iter().fold(Rect::EMPTY, |a, r| if a.is_empty() { *r } else { a.union(r) }))),
            Region::Whole => Some(None),
        }
    }
}

/// What `Document::apply` did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    /// Where the merged image changed.
    pub region:    Region,
    /// The node it made, if any.
    pub made:      Option<u64>,
    /// Paint layers whose pixels changed (their thumbnails to redo).
    pub layers:    Vec<u64>,
    /// The layer tree changed (the layer list to refresh).
    pub tree:      bool,
    /// The selection changed.
    pub selection: bool,
    /// Something else changed (the frame, a setting, the history).
    pub other:     bool,
    /// Why it did nothing, when the user should know.
    pub refused:   Option<&'static str>,
}

impl Done {
    pub fn changed(&self) -> bool { !self.region.is_nothing() || self.made.is_some() || !self.layers.is_empty() || self.tree || self.selection || self.other }

    pub(crate) fn refused(why: &'static str) -> Done { Done { refused: Some(why), ..Done::default() } }

    pub(crate) fn rects(rects: Vec<Rect>) -> Done {
        let mut region = Region::Nothing;
        for r in rects { region.add_rect(r); }
        Done { region, ..Done::default() }
    }

    pub fn join(&mut self, o: Done) {
        self.region.add(o.region);
        self.made = self.made.or(o.made);
        for l in o.layers { if !self.layers.contains(&l) { self.layers.push(l); } }
        self.tree |= o.tree;
        self.selection |= o.selection;
        self.other |= o.other;
        self.refused = self.refused.or(o.refused);
    }
}
