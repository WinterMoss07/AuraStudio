//! Selection commands (the selection tools and the Select menu), each one
//! undo step.

use aurastudio_canvas::Rect;
use aurastudio_layers::{FillParams, Node, Refer, SelectMode, Selection};

use crate::Document;

/// A drawn selection.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Rect),
    /// Its box (x0, y0, x1, y1).
    Ellipse((f32, f32, f32, f32)),
    /// A closed outline (lasso, polygon).
    Polygon(Vec<(f32, f32)>),
}

/// The magic wand's settings: tolerance, what it looks at, how much it
/// grows (FillParams.expand); every like pixel or only the touching ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wand {
    pub fill:       FillParams,
    pub contiguous: bool,
}

impl Default for Wand {
    fn default() -> Self { Self { fill: FillParams { expand: 0, ..FillParams::default() }, contiguous: true } }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SelectOp {
    Shape { shape: Shape, mode: SelectMode, aa: bool, feather: f32 },
    Wand { x: u32, y: u32, wand: Wand, mode: SelectMode, aa: bool, feather: f32 },
    /// The active layer's opaque pixels.
    FromLayer(SelectMode),
    All,
    None,
    /// The selection last taken away.
    Reselect,
    Invert,
    Grow(f32),
    Shrink(f32),
    Feather(f32),
    Border(f32),
    /// The outline moved by whole pixels.
    Move(i64, i64),
}

impl SelectOp {
    pub fn label(&self) -> &'static str {
        match self {
            SelectOp::Shape { .. } | SelectOp::Wand { .. } | SelectOp::FromLayer(_) => "Select",
            SelectOp::All => "Select all",
            SelectOp::None => "Deselect",
            SelectOp::Reselect => "Reselect",
            SelectOp::Invert => "Invert selection",
            SelectOp::Grow(_) => "Grow selection",
            SelectOp::Shrink(_) => "Shrink selection",
            SelectOp::Feather(_) => "Feather selection",
            SelectOp::Border(_) => "Border selection",
            SelectOp::Move(..) => "Move selection",
        }
    }
}

impl Document {
    /// Change the selection, undoable as `label`. False if it is the same.
    fn set_selection(&mut self, sel: Option<Selection>, label: &'static str) -> bool {
        if sel == self.selection { return false; }
        // (A layer edit not recorded yet goes first.)
        self.record_tree("Layers");
        let before = std::mem::replace(&mut self.selection, sel);
        if self.selection.is_none() { self.deselected = before.clone(); }
        self.history.push_selection(label, before);
        self.touch();
        true
    }

    /// Do a selection command. False if nothing changed.
    pub fn select(&mut self, op: &SelectOp) -> bool {
        let (w, h) = (self.layers.width, self.layers.height);
        let cur = self.selection.as_ref();
        let soft = |s: Option<Selection>, feather: f32| s.and_then(|s| if feather > 0.0 { s.feathered(feather) } else { Some(s) });
        let new = match op {
            SelectOp::Shape { shape, mode, aa, feather } => {
                let s = match shape {
                    Shape::Rect(r) => Selection::rect(w, h, *r),
                    Shape::Ellipse(b) => Selection::ellipse(w, h, *b, *aa),
                    Shape::Polygon(p) => Selection::polygon(w, h, p, *aa),
                };
                Selection::combine(cur, soft(s, *feather), *mode)
            }
            SelectOp::Wand { x, y, wand, mode, aa, feather } => {
                let s = self.wand(*x, *y, wand, *aa);
                Selection::combine(cur, soft(s, *feather), *mode)
            }
            SelectOp::FromLayer(mode) => {
                let s = match self.layers.active_node() {
                    Node::Layer(l) => Selection::from_alpha(&l.tiles),
                    _ => return false,
                };
                Selection::combine(cur, s, *mode)
            }
            SelectOp::All => Some(Selection::all(w, h)),
            SelectOp::None => None,
            SelectOp::Reselect => match &self.deselected {
                Some(s) if s.size() == (w, h) && self.selection.is_none() => Some(s.clone()),
                _ => return false,
            },
            SelectOp::Invert => match cur {
                Some(s) => s.inverted(),
                None => Some(Selection::all(w, h)),
            },
            SelectOp::Grow(r) => match cur { Some(s) => s.grown(*r), None => return false },
            SelectOp::Shrink(r) => match cur { Some(s) => s.shrunk(*r), None => return false },
            SelectOp::Feather(r) => match cur { Some(s) => s.feathered(*r), None => return false },
            SelectOp::Border(r) => match cur { Some(s) => s.bordered(*r), None => return false },
            SelectOp::Move(dx, dy) => match cur { Some(s) => s.translated(*dx, *dy), None => return false },
        };
        self.set_selection(new, op.label())
    }

    /// What the magic wand picks at (x, y).
    fn wand(&self, x: u32, y: u32, wand: &Wand, aa: bool) -> Option<Selection> {
        let (w, h) = (self.layers.width, self.layers.height);
        if x >= w || y >= h { return None; }
        // A folder has no pixels of its own: it looks at everything (a
        // frame: its chosen cel).
        let own = self.layers.layer_of_active();
        let active = own.unwrap_or(self.layers.active_id());
        let mut p = wand.fill;
        if p.refer == Refer::Layer && own.is_none() { p.refer = Refer::All; }
        let grow = p.expand;
        p.expand = 0;
        let pick = p.pick(&self.layers, active);
        let layer = match own { Some(id) => id, None => first_layer(&self.layers.root)? };
        let area = if wand.contiguous {
            aurastudio_layers::flood_with(&self.layers, layer, x, y, p, None, pick)?
        } else {
            aurastudio_layers::similar_with(&self.layers, layer, x, y, p, pick)?
        };
        let mut s = Selection::from_area(w, h, &area)?;
        if aa { s = s.antialiased()?; }
        if grow > 0 { s = s.grown(grow as f32)?; }
        Some(s)
    }
}

fn first_layer(nodes: &[Node]) -> Option<u64> {
    nodes.iter().find_map(|n| match n {
        Node::Layer(l) => Some(l.id),
        Node::Folder(f) => first_layer(&f.children),
        Node::Filter(_) => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DocId;

    #[test]
    fn selections_are_undone_and_reselected() {
        let mut d = Document::new(DocId(1), "s", 100, 100);
        let rect = |r| SelectOp::Shape { shape: Shape::Rect(r), mode: SelectMode::Replace, aa: true, feather: 0.0 };
        assert!(d.select(&rect((10, 10, 50, 50))));
        let add = SelectOp::Shape { shape: Shape::Ellipse((40.0, 40.0, 90.0, 90.0)), mode: SelectMode::Add, aa: true, feather: 0.0 };
        assert!(d.select(&add));
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (10, 10, 90, 90));
        assert!(d.select(&SelectOp::None));
        assert!(d.selection.is_none());
        assert_eq!(d.history().undo_label(), Some("Deselect"));
        assert!(d.select(&SelectOp::Reselect));
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (10, 10, 90, 90));
        // Undo: back through reselect, deselect, add, the first one.
        d.undo();
        assert!(d.selection.is_none());
        d.undo();
        d.undo();
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (10, 10, 50, 50));
        d.redo();
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (10, 10, 90, 90));
        // The same again records nothing.
        let n = d.history().undo_len();
        assert!(!d.select(&add));
        assert_eq!(d.history().undo_len(), n);
        assert!(d.select(&SelectOp::Invert));
        assert!(d.selection.as_ref().unwrap().contains(5, 5) && !d.selection.as_ref().unwrap().contains(20, 20));
        assert!(d.select(&SelectOp::Move(-5, 0)));
        assert!(!d.select(&SelectOp::None) || d.selection.is_none());
    }

    #[test]
    fn the_wand_picks_like_pixels() {
        let mut d = Document::new(DocId(1), "w", 64, 64);
        let id = d.layers.add_layer("ink");
        // Two red squares, apart.
        for r in [(4u32, 4u32, 20u32, 20u32), (40, 40, 60, 60)] {
            let n = ((r.2 - r.0) * (r.3 - r.1)) as usize;
            d.layers.set_layer_rect(id, r, &[255u8, 0, 0, 255].repeat(n));
        }
        let mut wand = Wand::default();
        let op = |wand, x, y| SelectOp::Wand { x, y, wand, mode: SelectMode::Replace, aa: false, feather: 0.0 };
        assert!(d.select(&op(wand, 10, 10)));
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (4, 4, 20, 20));
        wand.contiguous = false;
        assert!(d.select(&op(wand, 10, 10)));
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (4, 4, 60, 60));
        // Grown, anti-aliased.
        wand.fill.expand = 2;
        d.select(&SelectOp::Wand { x: 10, y: 10, wand, mode: SelectMode::Replace, aa: true, feather: 0.0 });
        let s = d.selection.as_ref().unwrap();
        assert_eq!(s.coverage(10, 10), 255);
        assert!(s.coverage(2, 10) > 0);
        // From the layer: both squares.
        d.select(&SelectOp::FromLayer(SelectMode::Replace));
        assert_eq!(d.selection.as_ref().unwrap().bbox(), (4, 4, 60, 60));
    }
}
