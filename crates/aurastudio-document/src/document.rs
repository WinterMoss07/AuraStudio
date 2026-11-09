//! An open document: the tree, its time, its history, the selection, the
//! sessions under way. Read freely; changed only by `apply` (and `stroke`,
//! the one edit that paints over time).

use std::path::PathBuf;

use aurastudio_animation::{Camera, SheetEdit, Sheets, Timeline};
use aurastudio_color::Profile;
use aurastudio_core::{DocType, PixelDepth, Rgba8};
use aurastudio_layers::{Area, FillParams, LayerStack, Node, Start, Target, TreeState};
use aurastudio_raster::Stroke;
use aurastudio_selection::{Selection, Space};

use crate::edit::{apply_tree, Edit};
use crate::history::{History, Step, Target as HistTarget, DEFAULT_BUDGET};
use crate::region::{Done, Region};
use crate::{filter::FilterSession, transform::Session};

/// Stable id of an open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocId(pub u64);

/// Resolution of a new document until the New dialog sets one.
pub const DEFAULT_DPI: f32 = 300.0;

pub struct Document {
    pub(crate) id:        DocId,
    pub name:             String,
    pub(crate) kind:      DocType,
    pub(crate) depth:     PixelDepth,
    pub(crate) stack:     LayerStack,
    pub(crate) timeline:  Timeline,
    pub(crate) camera:    Camera,
    pub(crate) profile:   Profile,
    pub(crate) history:   History,
    pub(crate) dpi:       f32,
    /// File the document was opened from / last saved to.
    pub path:             Option<PathBuf>,
    /// Bumped by every change; compared with `saved_revision`.
    pub(crate) revision:  u64,
    saved_revision:       u64,
    /// Pixels painting, filling and clearing may change; `None`: all.
    pub(crate) selection: Option<Selection>,
    /// The selection last taken away (Reselect).
    pub(crate) deselected: Option<Selection>,
    pub(crate) transform: Option<Session>,
    pub(crate) filter:    Option<FilterSession>,
    /// The tree as the history last saw it.
    pub(crate) tree:      TreeState,
    /// A sheet was made for the stroke under way: the history's serial
    /// right after (its first stroke joins it as one undo step).
    new_sheet:            Option<u64>,
    /// A stroke under way (painted over many pen samples, the document let
    /// go in between).
    stroke:               Option<StrokeSession>,
}

pub(crate) struct StrokeSession {
    layer: u64,
    label: String,
    state: aurastudio_raster::StrokeState<Rgba8>,
    /// The layer's alpha is locked: strokes change only the color.
    keep_alpha: bool,
}

impl Document {
    /// A new document: the locked white Paper and "Layer 1" above it; an
    /// Animation document starts with an animation folder instead.
    pub fn new(id: DocId, name: &str, width: u32, height: u32, kind: DocType) -> Self {
        let mut stack = LayerStack::new(width.max(1), height.max(1), kind.default_tile_size(), Start::Paper);
        if kind == DocType::Animation { stack.start_animation(); }
        Self::from_parts(id, name, stack, kind, History::new(DEFAULT_BUDGET))
    }

    /// A document from its parts (a file being opened): counts as saved.
    /// A document of layers made elsewhere (an image opened).
    pub fn from_stack(id: DocId, name: &str, stack: LayerStack, kind: DocType) -> Self {
        Self::from_parts(id, name, stack, kind, History::new(DEFAULT_BUDGET))
    }

    pub(crate) fn from_parts(id: DocId, name: &str, mut stack: LayerStack, kind: DocType, history: History) -> Self {
        let mut timeline = Timeline::default();
        timeline.settle(&mut stack);
        timeline.take_dirty();
        let tree = stack.tree_state();
        Self {
            id, name: name.to_owned(), kind, depth: PixelDepth::default(), stack, timeline, camera: Camera::default(), profile: Profile::srgb(),
            history, dpi: DEFAULT_DPI, path: None, revision: 0, saved_revision: 0, selection: None, deselected: None,
            transform: None, filter: None, tree, new_sheet: None, stroke: None,
        }
    }

    // --- Reading -----------------------------------------------------------

    pub fn id(&self) -> DocId { self.id }
    pub fn kind(&self) -> DocType { self.kind }
    pub fn depth(&self) -> PixelDepth { self.depth }
    pub fn stack(&self) -> &LayerStack { &self.stack }
    pub fn timeline(&self) -> &Timeline { &self.timeline }
    pub fn camera(&self) -> &Camera { &self.camera }
    pub fn profile(&self) -> &Profile { &self.profile }
    pub fn history(&self) -> &History { &self.history }
    pub fn dpi(&self) -> f32 { self.dpi }

    /// Made at this resolution (an image that says its own), not a change.
    pub fn with_dpi(mut self, dpi: f32) -> Self {
        if dpi.is_finite() && (1.0..=10_000.0).contains(&dpi) { self.dpi = dpi; }
        self
    }
    pub fn selection(&self) -> Option<&Selection> { self.selection.as_ref() }
    pub fn revision(&self) -> u64 { self.revision }
    pub fn is_modified(&self) -> bool { self.revision != self.saved_revision }
    /// The document as of `revision` is now on disk.
    pub fn mark_saved(&mut self, revision: u64) { self.saved_revision = revision; }
    /// Unsaved from now on (recovered work: not in its own file yet).
    pub fn touch(&mut self) { self.saved_revision = self.revision.wrapping_sub(1); }
    pub fn filtering(&self) -> bool { self.filter.is_some() }
    pub fn transforming(&self) -> bool { self.transform.is_some() }
    /// What the transform under way moves.
    pub fn transform_source(&self) -> Option<aurastudio_core::Rect> { self.transform.as_ref().map(|s| s.src) }

    /// The selection's frame and tile size.
    pub fn space(&self) -> Space { Space { width: self.stack.width, height: self.stack.height, tile: self.stack.tile } }

    /// The paint layer edits go to (clear, filters, transform...).
    pub fn paint_id(&self) -> Option<u64> { self.timeline.paint_id(&self.stack) }

    /// A stroke can start: on a paint layer, or on an animation folder's
    /// empty frame (the stroke makes the sheet).
    pub fn can_paint(&self) -> bool { self.timeline.paint_target(&self.stack).is_some() }

    /// The layer a filter would change and where, around what it shows
    /// (inside the selection), grown by `margin`: an empty rect on an
    /// empty layer. `None`: nothing can be filtered now.
    pub fn filter_area(&self, margin: u32) -> Option<(u64, aurastudio_core::Rect)> {
        let id = self.paint_id()?;
        let l = self.stack.layer(id)?;
        let content = if l.background { Some(self.stack.frame()) } else { l.surface.pixel_bounds() };
        let r = match (content, &self.selection) {
            (None, _) => aurastudio_core::Rect::EMPTY,
            (Some(c), Some(s)) => c.intersect(&s.bbox()),
            (Some(c), None) => c,
        };
        Some((id, if r.is_empty() { r } else { r.grown(margin as i32) }))
    }

    /// The merged image of `rect` of the nodes `pick` takes (a filter
    /// layer's dialog shows what is under it).
    pub fn merged(&self, rect: aurastudio_core::Rect, pick: aurastudio_layers::Pick) -> Vec<Rgba8> {
        aurastudio_compose::merge(&self.stack, rect, aurastudio_compose::Opts { pick, ..Default::default() })
    }

    /// `rect` as the screen shows it: every visible layer, the onion
    /// skin's ghosts too.
    pub fn screen(&self, rect: aurastudio_core::Rect) -> Vec<Rgba8> {
        aurastudio_compose::merge(&self.stack, rect, aurastudio_compose::Opts { ghosts: true, ..Default::default() })
    }

    // --- Changing ----------------------------------------------------------

    /// Do `e`. The only way the document changes.
    pub fn apply(&mut self, e: Edit) -> Done { self.apply_until(e, &|| false) }

    /// `apply`; a filter preview asks `stop` between its pieces and, when
    /// stopped, leaves the layer as the last preview made it.
    pub fn apply_until(&mut self, e: Edit, stop: aurastudio_filters::StopFn<'_>) -> Done {
        aurastudio_core::zone!("document apply");
        let label = e.label();
        // A stroke still open ends first (its patch is of the layer as it
        // is now), unless the edit only reads or moves the tree.
        let mut first = Done::default();
        if self.stroke.is_some() && (e.changes_pixels() || self.touches_stroke(&e)) { first = self.end_stroke(); }
        // How it is viewed (the current frame, onion look/pause): not a
        // change to save.
        let view = matches!(e, Edit::SetFrame(_) | Edit::SetOnionLook(_) | Edit::PauseOnion(_));
        let mut done = match e {
            Edit::SetFrame(n) => Done { other: self.timeline.frame() != n.clamp(1, aurastudio_layers::MAX_FRAME), region: self.time(|t, s| t.set_frame(s, n)), ..Done::default() },
            Edit::SetFps(f) => {
                let changed = self.timeline.fps != f && f.num > 0;
                if changed { self.timeline.fps = f; }
                Done { other: changed, ..Done::default() }
            }
            Edit::SetOnion(o) => self.time_edit(|t, s| t.set_onion(s, o)),
            Edit::SetOnionLook(l) => self.time_edit(|t, s| t.set_look(s, l)),
            Edit::PauseOnion(p) => self.time_edit(|t, s| t.pause_onion(s, p)),
            Edit::SelectionToMask(name) => match self.selection.clone() {
                Some(sel) => { let id = self.stack.add_mask(&name, sel.into_mask()); self.after_tree(label, Done { made: Some(id), ..Done::default() }) }
                None => Done::refused("Nothing is selected"),
            },
            Edit::MaskToSelection(id) => match self.stack.node(id) {
                Some(Node::Mask(m)) => { let sel = Selection::of(m.mask.clone()); self.set_selection(sel, label) }
                _ => Done::default(),
            },
            Edit::ClearActive => self.clear_active(),
            Edit::Fill { x, y, color, params } => self.fill(x, y, color, params),
            Edit::Paste(clip) => self.paste(&clip),
            Edit::Select(op) => self.select(&op),
            Edit::FilterPreview(f) => self.filter_preview(f.as_ref(), stop),
            Edit::FilterApply(f) => self.filter_apply(&f),
            Edit::FilterCancel => self.filter_cancel(),
            Edit::TransformBegin(ids) => self.transform_begin(&ids),
            Edit::TransformPreview { warp, interp } => self.transform_preview(&warp, interp),
            Edit::TransformApply { warp, interp } => self.transform_apply(&warp, interp),
            Edit::TransformCancel => self.transform_cancel(),
            Edit::Undo => self.undo(),
            Edit::Redo => self.redo(),
            Edit::Resize { width, height } => self.resize(width, height),
            Edit::SetMargin(m) => self.set_margin(m),
            Edit::SetDpi(d) => {
                let ok = d.is_finite() && (1.0..=10_000.0).contains(&d) && d != self.dpi;
                if ok { self.dpi = d; }
                Done { other: ok, ..Done::default() }
            }
            Edit::AssignProfile(p) => {
                if p == self.profile { return Done::default(); }
                let old = std::mem::replace(&mut self.profile, p);
                self.history.push(label, Step::Profile(old));
                Done { region: Region::Whole, other: true, ..Done::default() }
            }
            e => {
                // A tree edit: bounded by the footprints of what it touches,
                // before and after.
                let ids = e.nodes(&self.stack);
                let mut region = Region::Nothing;
                for &id in &ids { region.add(Region::of_footprint(self.stack.footprint(id))); }
                let key = e.joins();
                let (changed, made) = apply_tree(e, &mut self.stack);
                if !changed { return Done::default(); }
                for id in ids.iter().copied().chain(made) { region.add(Region::of_footprint(self.stack.footprint(id))); }
                self.settle_into(&mut region);
                let tree = self.record_tree_as(label, key);
                Done { region, made, tree, ..Done::default() }
            }
        };
        if done.changed() && !view {
            self.revision += 1;
            debug_assert!(self.stack.check_animation(self.timeline.frame()).is_ok(), "{:?}", self.stack.check_animation(self.timeline.frame()));
        }
        if done.refused.is_some() && !done.changed() { done.region = Region::Nothing; }
        first.join(done);
        first
    }

    /// `e` takes away or moves the stroke's layer or a folder it is in
    /// (the stroke would end unrecorded).
    fn touches_stroke(&self, e: &Edit) -> bool {
        let Some(s) = &self.stroke else { return false };
        if !matches!(e, Edit::RemoveActive | Edit::Ungroup | Edit::Group { .. } | Edit::GroupAnimationFolders { .. } | Edit::Move { .. }) { return false; }
        let ids = e.nodes(&self.stack);
        let mut at = Some(s.layer);
        while let Some(id) = at {
            if ids.contains(&id) { return true; }
            at = self.stack.parent_of(id);
        }
        false
    }

    /// A change of time (the frame, the onion skin): what it reshows.
    fn time(&mut self, f: impl FnOnce(&mut Timeline, &mut LayerStack) -> bool) -> Region {
        f(&mut self.timeline, &mut self.stack);
        let mut r = Region::Nothing;
        if let Some(d) = self.timeline.take_dirty() { r.add_rect(d); }
        // (Settling may rename sheets: not a step of its own.)
        self.tree = self.stack.tree_state();
        r
    }

    fn time_edit(&mut self, f: impl FnOnce(&mut Timeline, &mut LayerStack) -> bool) -> Done {
        let region = self.time(f);
        Done { other: true, region, ..Done::default() }
    }

    /// The time settled after the tree changed; what it reshows into
    /// `region`.
    fn settle_into(&mut self, region: &mut Region) {
        self.timeline.settle(&mut self.stack);
        if let Some(d) = self.timeline.take_dirty() { region.add_rect(d); }
    }

    /// After the tree changed: settle the time, record the step.
    pub(crate) fn after_tree(&mut self, label: &str, mut done: Done) -> Done {
        self.settle_into(&mut done.region);
        done.tree |= self.record_tree(label);
        done
    }

    /// Record what was done to the tree since the last time as one step.
    /// False if nothing changed (the active node, visibility and folding
    /// don't count).
    pub(crate) fn record_tree(&mut self, label: &str) -> bool { self.record_tree_as(label, None) }

    /// The same; `key`: a setting of that node, joined with the last step
    /// when it changed the same one.
    fn record_tree_as(&mut self, label: &str, key: Option<u64>) -> bool {
        let now = self.stack.tree_state();
        let removed = self.stack.take_removed();
        if now == self.tree { return !removed.is_empty(); }
        let before = std::mem::replace(&mut self.tree, now);
        self.history.push_tree(label, key, before, removed);
        true
    }

    fn hist_target(&mut self) -> (HistTarget<'_>, &mut History) {
        (HistTarget { stack: &mut self.stack, selection: &mut self.selection, profile: &mut self.profile }, &mut self.history)
    }

    /// (A filter or transform under way is put back first: what it showed
    /// is shown again too.)
    fn undo(&mut self) -> Done {
        let mut done = self.end_sessions();
        let (mut t, h) = self.hist_target();
        let d = h.undo(&mut t);
        done.join(self.after_history(d));
        done
    }

    fn redo(&mut self) -> Done {
        let mut done = self.end_sessions();
        let (mut t, h) = self.hist_target();
        let d = h.redo(&mut t);
        done.join(self.after_history(d));
        done
    }

    fn after_history(&mut self, d: Option<Done>) -> Done {
        let Some(mut d) = d else { return Done::default() };
        // (A step that could not be applied: dropped, nothing changed.)
        if d.refused.is_some() { return d; }
        d.other = true;
        if d.tree {
            self.settle_into(&mut d.region);
            self.tree = self.stack.tree_state();
        }
        d
    }

    /// A filter or transform under way is dropped (its saved pixels may no
    /// longer fit).
    fn end_sessions(&mut self) -> Done {
        let mut d = self.filter_cancel();
        d.join(self.transform_cancel());
        d
    }

    fn resize(&mut self, width: u32, height: u32) -> Done {
        if width == 0 || height == 0 || (width, height) == (self.stack.width, self.stack.height) { return Done::default(); }
        self.reframe("Canvas size", |s| s.resize(width, height))
    }

    fn set_margin(&mut self, margin: u32) -> Done {
        let margin = margin.min(aurastudio_layers::MAX_MARGIN);
        if margin == self.stack.margin { return Done::default(); }
        self.reframe("Canvas margin", |s| s.set_margin(margin))
    }

    /// A new frame or margin made by `f`: what falls out is cut. Undone,
    /// the stack as it was comes back whole (the steps before fit it).
    fn reframe(&mut self, label: &str, f: impl FnOnce(&mut LayerStack)) -> Done {
        let mut done = self.end_sessions();
        let before = Box::new(self.stack.clone());
        f(&mut self.stack);
        self.stack.take_removed();
        self.tree = self.stack.tree_state();
        let selection = self.selection.take();
        self.deselected = None;
        self.history.push(label, Step::Frame { stack: before, selection });
        done.join(Done { region: Region::Whole, other: true, selection: true, ..Done::default() });
        done
    }

    // --- Painting ------------------------------------------------------------

    /// The layer a stroke or a fill goes to. In an animation folder with no
    /// sheet exposed from now, a sheet is made first (one undo step with
    /// what is painted). `None`: nothing to paint on.
    fn begin_paint(&mut self, done: &mut Done) -> Option<u64> {
        self.new_sheet = None;
        match self.timeline.paint_target(&self.stack)? {
            Target::Layer(id) => Some(id),
            Target::NewSheet { folder, frame } => {
                let active = self.stack.active_id();
                let id = self.stack.make_sheet(folder, frame)?;
                // Its own kind of step (files keep it with the stroke).
                let sheet = match self.stack.parent_of(id) { Some(p) if p != folder => p, _ => id };
                let place = self.stack.place_of(sheet)?;
                self.settle_into(&mut done.region);
                self.stack.take_removed();
                self.tree = self.stack.tree_state();
                self.history.push("New sheet", Step::Sheet { id: sheet, place, node: None, active });
                done.tree = true;
                self.new_sheet = Some(self.history.serial());
                Some(id)
            }
        }
    }

    /// Record what was painted; with the sheet just made for it, as one
    /// step. Nothing painted: the sheet goes.
    pub(crate) fn end_paint(&mut self, label: &str, layer: u64, before: Before, done: &mut Done) -> bool {
        let Some(img) = self.stack.image(layer) else { return false };
        let patch = img.diff(&before.0);
        let fill = (img.fill() != before.1).then_some(before.1);
        self.record_paint(label, layer, patch, fill, done)
    }

    /// The step of a paint (`fill`: the color before, if it changed); with
    /// the sheet just made for it, as one step. Nothing changed: the sheet
    /// goes.
    fn record_paint(&mut self, label: &str, layer: u64, patch: aurastudio_raster::Patch<Rgba8>, fill: Option<Rgba8>, done: &mut Done) -> bool {
        if patch.is_empty() && fill.is_none() {
            self.drop_new_sheet(done);
            return false;
        }
        done.join(Done::rects(patch.rects()));
        done.layers.push(layer);
        if fill.is_some() { done.region = Region::Whole; }
        let join = self.new_sheet.take() == Some(self.history.serial());
        let s0 = self.history.serial();
        self.history.push_pixels(label, layer, patch, fill);
        if join && self.history.serial() == s0 + 1 { self.history.join_last(label); }
        true
    }

    /// The sheet made for a paint that painted nothing goes (no step left):
    /// then nothing happened at all.
    fn drop_new_sheet(&mut self, done: &mut Done) {
        let Some(s) = self.new_sheet.take() else { return };
        if s != self.history.serial() { return; }
        let (mut t, h) = self.hist_target();
        let d = h.undo(&mut t);
        self.history.forget_redo();
        self.after_history(d);
        *done = Done::default();
    }

    /// A stroke in one go: `paint` draws through it (the brush engine;
    /// the selection limits it at the source), then it is one undo step of
    /// the tiles it changed. Painting nothing leaves no step.
    pub fn stroke(&mut self, label: &str, paint: impl FnOnce(&mut Stroke<'_, Rgba8>)) -> Done {
        let mut done = self.begin_stroke(label, None);
        if done.refused.is_some() { return done; }
        self.paint(paint);
        let end = self.end_stroke();
        // Nothing painted: nothing happened (a sheet made for it went).
        if !end.changed() { return Done::default(); }
        done.join(end);
        done
    }

    /// Start a stroke on the layer strokes go to now (in an animation
    /// folder with nothing exposed from now: a new sheet, one undo step
    /// with the stroke). `limit`: a coverage it stays in besides the
    /// selection (stay inside the lines). A stroke still open ends first.
    pub fn begin_stroke(&mut self, label: &str, limit: Option<Selection>) -> Done {
        aurastudio_core::zone!("document begin stroke");
        let mut done = self.end_stroke();
        let Some(layer) = self.begin_paint(&mut done) else { done.refused = Some("Layer is locked or is a folder"); return done };
        let limit = match (self.selection.as_ref(), limit) {
            (Some(s), Some(l)) => Selection::combine(Some(s), Some(l), aurastudio_selection::Mode::Intersect).map(Selection::into_mask).or_else(|| Some(aurastudio_raster::Mask::new(self.stack.width, self.stack.height, self.stack.tile, 0))),
            (Some(s), None) => Some(s.as_mask().clone()),
            (None, l) => l.map(Selection::into_mask),
        };
        let Some(img) = self.stack.image(layer) else {
            self.drop_new_sheet(&mut done);
            done.refused = Some("Nothing to paint on");
            return done;
        };
        let state = img.stroke_state(limit.as_ref()).clipped(self.stack.area());
        let keep_alpha = self.stack.node(layer).is_some_and(|n| n.props().alpha_lock);
        self.stroke = Some(StrokeSession { layer, label: label.to_owned(), state, keep_alpha });
        done
    }

    /// "Stay inside the lines": the area a stroke starting at (x, y) keeps
    /// to, the region of the reference layers around it (all the other
    /// layers when none is marked), as the fill finds it. `None` (free):
    /// started on a line, off the frame, or nothing around.
    pub fn stay_inside(&self, x: f32, y: f32, p: FillParams) -> Option<Selection> {
        use aurastudio_layers::Pick;
        aurastudio_core::zone!("stay inside");
        let (xi, yi) = (x.floor() as i32, y.floor() as i32);
        if !self.stack.frame().contains(xi, yi) { return None; }
        let layer = self.timeline.layer_of_active(&self.stack).unwrap_or(0);
        let reference = self.stack.reference_for(layer);
        let pick = Pick { skip_drafts: p.skip_drafts, reference, skip: layer, ..Pick::ALL };
        // On a line itself: free (CSP: the brush's center decides).
        let under = aurastudio_compose::merge(&self.stack, aurastudio_core::Rect::new(xi, yi, xi + 1, yi + 1), aurastudio_compose::Opts { pick, backdrop: aurastudio_compose::Backdrop::Transparent, ..Default::default() });
        if under.first().is_some_and(|c| c[3] >= 128) { return None; }
        let src = aurastudio_compose::Merged { stack: &self.stack, pick };
        let limit = self.selection.as_ref().map(Selection::as_mask);
        let area = aurastudio_layers::fill::flood(&src, self.stack.frame(), xi, yi, p.tolerance, p.expand, limit)?;
        Selection::from_predicate(self.space(), area.bbox(), |x, y| area.contains(x, y))
    }

    /// A stroke is under way.
    pub fn stroking(&self) -> bool { self.stroke.is_some() }

    /// The layer the stroke under way paints on.
    pub fn stroke_layer(&self) -> Option<u64> { self.stroke.as_ref().map(|s| s.layer) }

    /// Paint more of the stroke under way. `None` if there is none, or its
    /// layer is gone (the stroke stops; an edit taking the layer away ends
    /// the stroke first, recorded).
    pub fn paint<R>(&mut self, f: impl FnOnce(&mut Stroke<'_, Rgba8>) -> R) -> Option<R> {
        let StrokeSession { layer, label, state, keep_alpha } = self.stroke.take()?;
        let Some(img) = self.stack.image_mut(layer) else { self.new_sheet = None; return None };
        let mut stroke = state.on(img);
        let r = f(&mut stroke);
        self.stroke = Some(StrokeSession { layer, label, state: stroke.detach(), keep_alpha });
        Some(r)
    }

    /// `paint` for a brush giving where it painted: on an alpha-locked
    /// layer, that area keeps its alpha at once (shown as it will be).
    pub fn paint_area(&mut self, f: impl FnOnce(&mut Stroke<'_, Rgba8>) -> Option<aurastudio_core::Rect>) -> Option<Option<aurastudio_core::Rect>> {
        let keep = self.stroke.as_ref().is_some_and(|s| s.keep_alpha);
        self.paint(|s| {
            let r = f(s);
            if let (true, Some(r)) = (keep, r) { s.keep_alpha(r); }
            r
        })
    }

    /// End the stroke under way: one undo step of the tiles it changed
    /// (none if it painted nothing: a sheet made for it goes too).
    pub fn end_stroke(&mut self) -> Done {
        let Some(st) = self.stroke.take() else { return Done::default() };
        aurastudio_core::zone!("document end stroke");
        let mut done = Done::default();
        let Some(img) = self.stack.image_mut(st.layer) else { self.drop_new_sheet(&mut done); return done };
        let mut stroke = st.state.on(img);
        // (Whatever painted without saying where: kept now.)
        if st.keep_alpha { stroke.keep_alpha_all(); }
        let patch = stroke.end();
        if self.record_paint(&st.label, st.layer, patch, None, &mut done) {
            self.revision += 1;
            done.other = true;
        }
        done
    }

    fn clear_active(&mut self) -> Done {
        let Some(id) = self.paint_id() else { return Done::refused("Layer is locked or is a folder") };
        let Some(before) = self.before(id) else { return Done::default() };
        match &self.selection {
            Some(s) => { self.stack.clear_selected(id, s.as_mask()); }
            None => if let Some(l) = self.stack.layer_mut(id) { l.clear(); },
        }
        let mut done = Done::default();
        self.end_paint("Clear layer", id, before, &mut done);
        done
    }

    fn fill(&mut self, x: i32, y: i32, color: Rgba8, p: FillParams) -> Done {
        aurastudio_core::zone!("document fill");
        let mut done = Done::default();
        let Some(id) = self.begin_paint(&mut done) else { return Done::refused("Layer is locked or is a folder") };
        let area = self.fill_area(id, x, y, p, true);
        let Some(area) = area else { self.drop_new_sheet(&mut done); return done };
        let Some(before) = self.before(id) else { return done };
        let sel = self.selection.as_ref().map(Selection::as_mask);
        // Filling an empty part of the layer (the lines on it, or on
        // another): behind its pixels, so a line's soft edge shades into
        // the fill. Recoloring a painted area: over.
        let behind = self.stack.image(id).is_some_and(|i| i.pixel(x, y)[3] < 255);
        self.stack.fill_area(id, &area, color, sel, behind);
        self.end_paint("Fill", id, before, &mut done);
        done
    }

    /// What the fill tool (or the wand: `contiguous` false) picks at (x, y)
    /// for layer `id`, in the frame.
    pub(crate) fn fill_area(&self, id: u64, x: i32, y: i32, p: FillParams, contiguous: bool) -> Option<Area> {
        let frame = self.stack.frame();
        let limit = self.selection.as_ref().map(Selection::as_mask).filter(|_| contiguous);
        let pick = p.pick(&self.stack, id);
        let merged;
        let src: &dyn aurastudio_layers::FillSource = match pick {
            Some(pick) => { merged = aurastudio_compose::Merged { stack: &self.stack, pick }; &merged }
            None => self.stack.image(id)?,
        };
        if contiguous {
            aurastudio_layers::fill::flood(src, frame, x, y, p.tolerance, p.expand, limit)
        } else {
            aurastudio_layers::fill::similar_all(src, frame, x, y, p.tolerance, p.expand)
        }
    }

    /// Change the selection, undoable as `label`.
    pub(crate) fn set_selection(&mut self, sel: Option<Selection>, label: &str) -> Done {
        if sel == self.selection { return Done::default(); }
        let before = std::mem::replace(&mut self.selection, sel);
        if self.selection.is_none() { self.deselected = before.clone(); }
        self.history.push(label, Step::Select(before));
        Done { selection: true, ..Done::default() }
    }
}

/// A layer as it was before a pixel edit: its tiles and fill.
pub(crate) struct Before(pub aurastudio_raster::Snapshot<Rgba8>, pub Rgba8);

impl Document {
    pub(crate) fn before(&self, id: u64) -> Option<Before> { self.stack.image(id).map(|i| Before(i.snapshot(), i.fill())) }
}
