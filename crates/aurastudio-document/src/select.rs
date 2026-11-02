//! Selection commands (the selection tools and the Select menu), each one
//! undo step.

use aurastudio_core::{Point, Rect};
use aurastudio_layers::{FillParams, Refer};
use aurastudio_selection::{Mode, Selection};

use crate::region::Done;
use crate::Document;

/// A drawn selection.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Rect),
    /// Its box (x0, y0, x1, y1).
    Ellipse([f64; 4]),
    /// A closed outline (lasso, polygon).
    Polygon(Vec<Point>),
}

/// The magic wand's settings: tolerance, what it looks at, how much it
/// grows; every like pixel or only the touching ones.
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
    Shape { shape: Shape, mode: Mode, aa: bool, feather: f32 },
    Wand { x: i32, y: i32, wand: Wand, mode: Mode, aa: bool, feather: f32 },
    /// The active layer's visible pixels.
    FromLayer(Mode),
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
    Move(i32, i32),
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
    pub(crate) fn select(&mut self, op: &SelectOp) -> Done {
        aurastudio_core::zone!("select");
        let space = self.space();
        let cur = self.selection.as_ref();
        let soft = |s: Option<Selection>, feather: f32| s.and_then(|s| if feather > 0.0 { s.feathered(feather) } else { Some(s) });
        let new = match op {
            SelectOp::Shape { shape, mode, aa, feather } => {
                let s = match shape {
                    Shape::Rect(r) => Selection::rect(space, *r),
                    Shape::Ellipse(b) => Selection::ellipse(space, *b, *aa),
                    Shape::Polygon(p) => Selection::polygon(space, p, *aa),
                };
                Selection::combine(cur, soft(s, *feather), *mode)
            }
            SelectOp::Wand { x, y, wand, mode, aa, feather } => {
                let s = self.wand(*x, *y, wand, *aa);
                Selection::combine(self.selection.as_ref(), soft(s, *feather), *mode)
            }
            SelectOp::FromLayer(mode) => {
                let Some(img) = self.timeline.layer_of_active(&self.stack).and_then(|id| self.stack.image(id)) else { return Done::default() };
                Selection::combine(cur, Selection::from_alpha(img), *mode)
            }
            SelectOp::All => Some(Selection::all(space)),
            SelectOp::None => None,
            SelectOp::Reselect => match &self.deselected {
                Some(s) if s.space() == space && self.selection.is_none() => Some(s.clone()),
                _ => return Done::default(),
            },
            SelectOp::Invert => match cur { Some(s) => s.inverted(), None => Some(Selection::all(space)) },
            SelectOp::Grow(r) => match cur { Some(s) => s.grown(*r), None => return Done::default() },
            SelectOp::Shrink(r) => match cur { Some(s) => s.shrunk(*r), None => return Done::default() },
            SelectOp::Feather(r) => match cur { Some(s) => s.feathered(*r), None => return Done::default() },
            SelectOp::Border(r) => match cur { Some(s) => s.bordered(*r), None => return Done::default() },
            SelectOp::Move(dx, dy) => match cur { Some(s) => s.translated(*dx, *dy), None => return Done::default() },
        };
        self.set_selection(new, op.label())
    }

    /// What the magic wand picks at (x, y): a folder has no pixels of its
    /// own, it looks at everything.
    fn wand(&self, x: i32, y: i32, wand: &Wand, aa: bool) -> Option<Selection> {
        let own = self.timeline.layer_of_active(&self.stack);
        let mut p = wand.fill;
        if p.refer == Refer::Layer && own.is_none() { p.refer = Refer::All; }
        let grow = p.expand;
        p.expand = 0;
        let layer = own.unwrap_or(self.stack.active_id());
        let area = {
            // (No selection limits the wand.)
            let frame = self.stack.frame();
            let pick = p.pick(&self.stack, layer);
            let merged;
            let src: &dyn aurastudio_layers::FillSource = match pick {
                Some(pick) => { merged = aurastudio_compose::Merged { stack: &self.stack, pick }; &merged }
                None => self.stack.image(layer)?,
            };
            if wand.contiguous { aurastudio_layers::fill::flood(src, frame, x, y, p.tolerance, 0, None) } else { aurastudio_layers::fill::similar_all(src, frame, x, y, p.tolerance, 0) }
        }?;
        let mut s = Selection::from_predicate(self.space(), area.bbox(), |x, y| area.contains(x, y))?;
        if aa { s = s.antialiased()?; }
        if grow > 0 { s = s.grown(grow as f32)?; }
        Some(s)
    }
}
