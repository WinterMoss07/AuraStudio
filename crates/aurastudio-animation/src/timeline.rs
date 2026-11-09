//! The current frame and what it shows: held by the document beside its
//! tree; `settle` after every edit and every frame change (sheets shown
//! by time, named by rank, the active row a sheet, the ghosts), with the
//! area of the canvas that changed.

use aurastudio_core::{Fps, Rect};
use aurastudio_layers::{LayerStack, Node, Target};

use crate::onion::{ghosts, Onion, OnionLook};
use crate::sheets::{child_of, Sheets};

/// Everywhere (a change the merge can't bound: filters, clipping, a cel
/// with a visible fill): intersect with what is shown.
pub const EVERYWHERE: Rect = Rect { x0: -(1 << 28), y0: -(1 << 28), x1: 1 << 28, y1: 1 << 28 };

/// Time of a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    /// From 1.
    frame:      u32,
    pub fps:    Fps,
    pub onion:  Onion,
    pub look:   OnionLook,
    /// Ghosts off for a while (playback), marks kept.
    paused:     bool,
    /// Where the merge changed since `take_dirty`.
    dirty:      Option<Rect>,
}

impl Default for Timeline {
    fn default() -> Self { Self { frame: 1, fps: Fps::new(24, 1), onion: Onion::default(), look: OnionLook::default(), paused: false, dirty: None } }
}

/// A filter or a clipped node reads around: a sheet shown can change
/// pixels outside its own.
fn reads_around(nodes: &[Node]) -> bool {
    nodes.iter().enumerate().any(|(i, n)| n.visible() && match n {
        Node::Filter(_) => true,
        _ if n.props().clip && i > 0 => true,
        Node::Folder(f) => reads_around(&f.children),
        _ => false,
    })
}

impl Timeline {
    pub fn frame(&self) -> u32 { self.frame }

    /// Go to frame `n` and settle. True if what the merge shows may have
    /// changed (false inside a hold, or with no animation folder).
    pub fn set_frame(&mut self, stack: &mut LayerStack, n: u32) -> bool {
        self.frame = n.clamp(1, aurastudio_layers::MAX_FRAME);
        // A sheet or cel chosen moves to its folder's sheet of now.
        if let Some((folder, Some(_))) = stack.anim_of(stack.active_id()) {
            let to = stack.shown_at(folder, self.frame).unwrap_or(folder);
            stack.set_active(to);
        }
        self.settle(stack)
    }

    pub fn set_onion(&mut self, stack: &mut LayerStack, marks: Onion) -> bool { self.onion = marks; self.settle(stack) }

    pub fn set_look(&mut self, stack: &mut LayerStack, look: OnionLook) -> bool { self.look = look; self.settle(stack) }

    /// Ghosts off (playback) or back on.
    pub fn pause_onion(&mut self, stack: &mut LayerStack, paused: bool) -> bool {
        if self.paused == paused { return false; }
        self.paused = paused;
        self.settle(stack)
    }

    /// After any edit of the tree or of the time: each animation folder
    /// shows only its sheet of now, its sheets named in time order; the
    /// active node is a row of the layer list (a sheet, not one of its
    /// cels); groups left empty go; the ghosts follow. True if the merge
    /// may have changed (the area is in `take_dirty`).
    pub fn settle(&mut self, stack: &mut LayerStack) -> bool {
        fn walk(nodes: &mut [Node], time: u32, flipped: &mut Vec<u64>) {
            for n in nodes {
                let Node::Folder(f) = n else { continue };
                if f.anim().is_some() {
                    let mut starts: Vec<u32> = f.children.iter().filter_map(Node::sheet_start).collect();
                    starts.sort_unstable();
                    let shown = starts.iter().copied().filter(|k| *k <= time).max();
                    for c in &mut f.children {
                        let Some(k) = c.sheet_start() else { continue };
                        let show = Some(k) == shown;
                        if c.visible() != show { *c.visible_mut() = show; flipped.push(c.id()); }
                        // Named by rank in time: "Sheet 1", "Sheet 2"...
                        let rank = starts.partition_point(|x| *x < k) + 1;
                        if c.name().strip_prefix("Sheet ").and_then(|r| r.parse::<usize>().ok()) != Some(rank) { *c.name_mut() = format!("Sheet {rank}"); }
                    }
                } else {
                    walk(&mut f.children, time, flipped);
                }
            }
        }
        aurastudio_core::zone!("timeline settle");
        let mut flipped = Vec::new();
        walk(&mut stack.root, self.frame, &mut flipped);
        stack.tidy();
        // (A cel chosen: its sheet, by `set_active`'s rule.)
        stack.set_active(stack.active_id());
        let g = if self.paused { Vec::new() } else { ghosts(stack, &self.onion, self.look, self.frame) };
        let old: Vec<u64> = stack.ghosts().iter().flat_map(|x| x.1.iter().map(|g| g.sheet)).collect();
        let ghost_area = stack.set_ghosts(g);
        if flipped.is_empty() && ghost_area.is_none() { return false; }
        let d = if reads_around(&stack.root) {
            EVERYWHERE
        } else {
            let mut ids = flipped;
            if ghost_area.is_some() { ids.extend(old); ids.extend(stack.ghosts().iter().flat_map(|x| x.1.iter().map(|g| g.sheet))); }
            ids.iter().filter_map(|&id| stack.extent(id)).fold(Rect::EMPTY, |a, r| a.union(&r))
        };
        if !d.is_empty() { self.dirty = Some(self.dirty.map_or(d, |o| o.union(&d))); }
        true
    }

    /// The area the merge changed in since the last call (sheets shown or
    /// hidden by time, ghosts), `None` if nothing.
    pub fn take_dirty(&mut self) -> Option<Rect> { self.dirty.take() }

    /// Where a stroke on the active node goes, or `None` if it can't be
    /// painted (a folder, a filter layer, locked). In an animation folder:
    /// its sheet exposed from now, else a new one there (also over a held
    /// sheet), on the folder's cel.
    pub fn paint_target(&self, stack: &LayerStack) -> Option<Target> {
        let a = stack.active_id();
        match stack.anim_of(a) {
            None => stack.paint_id().map(Target::Layer),
            Some((folder, _)) => {
                let anim = stack.anim(folder)?;
                let rank = anim.cel.min(anim.template.len().checked_sub(1)?);
                match stack.sheet_at(folder, self.frame) {
                    Some(sheet) => {
                        let f = stack.node(sheet)?.as_folder()?;
                        let cel = f.children.get(child_of(f.children.len(), rank))?.id();
                        (!stack.is_locked(cel)).then_some(Target::Layer(cel))
                    }
                    None => (!stack.is_locked(folder) && !anim.template[rank].locked).then_some(Target::NewSheet { folder, frame: self.frame }),
                }
            }
        }
    }

    /// The paint layer edits (clear, filters, transform...) go to: the
    /// active layer, or the cel of the sheet exposed from now.
    pub fn paint_id(&self, stack: &LayerStack) -> Option<u64> {
        match self.paint_target(stack)? { Target::Layer(id) => Some(id), Target::NewSheet { .. } => None }
    }

    /// The paint layer the active row stands for, to read from (copy, the
    /// wand): a layer, a sheet's chosen cel, or for an animation folder the
    /// one of the sheet it shows now. Locks don't matter.
    pub fn layer_of_active(&self, stack: &LayerStack) -> Option<u64> {
        let a = stack.active_id();
        let sheet = match stack.node(a)? {
            Node::Layer(_) => return Some(a),
            n if n.is_animation() => stack.shown_at(a, self.frame)?,
            n if n.sheet_start().is_some() => a,
            _ => return None,
        };
        let rank = stack.anim(sheet)?.cel;
        let f = stack.node(sheet)?.as_folder()?;
        f.children.get(child_of(f.children.len(), rank.min(f.children.len().checked_sub(1)?))).map(Node::id)
    }

    /// The sheet `folder` shows now.
    pub fn exposed(&self, stack: &LayerStack, folder: u64) -> Option<u64> { stack.shown_at(folder, self.frame) }
}
