//! Layer tree: paint layers, folders (groups) and filter layers, like
//! Krita / CSP.
//!
//! Stored bottom to top. Each node has a stable `id`; the active node is
//! tracked by id, so reordering or moving in and out of folders never
//! changes which layer you paint on.
//!
//! Every node also has `Props`: a color label (Krita's colors), clipping
//! to the node below (CSP / Photoshop clipping masks), and CSP's reference
//! and draft marks (see `compose.rs` and `fill.rs` for what they do).

mod compose;
pub mod fill;
mod selection;
mod stroke_cache;
mod tiles;

use aurastudio_canvas::blend::{blend_over, unit_to_u8, Premul};
use aurastudio_canvas::{BlendMode, PaintSurface, Rect, TILE};

pub use compose::Pick;
pub use fill::{flood, flood_with, FillParams, Refer};
pub use selection::Selection;
pub use tiles::{new_tile, Tile, Tiles, TILE_BYTES};

/// Straight-alpha "over": `src` painted on `dst`.
fn over(dst: &mut [u8], src: [u8; 4]) {
    let sa = src[3] as u32;
    if sa == 255 { dst.copy_from_slice(&src); return; }
    if sa == 0 { return; }
    let da = dst[3] as u32 * (255 - sa) / 255;
    let a = sa + da;
    for c in 0..3 {
        dst[c] = ((src[c] as u32 * sa + dst[c] as u32 * da) / a) as u8;
    }
    dst[3] = a as u8;
}

/// Color labels, Krita's eight: (name, color). Index 0: none.
pub const LABELS: [(&str, [u8; 3]); 9] = [
    ("None", [0, 0, 0]),
    ("Blue", [86, 137, 222]),
    ("Green", [102, 184, 90]),
    ("Yellow", [229, 196, 64]),
    ("Orange", [232, 135, 58]),
    ("Brown", [150, 104, 70]),
    ("Red", [220, 72, 72]),
    ("Purple", [154, 98, 204]),
    ("Gray", [140, 140, 148]),
];

/// What every node has besides its content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Props {
    /// Color label, an index into LABELS (0: none).
    pub label:     u8,
    /// Clipped to the node below (its base): shows only where the base
    /// shows. Consecutive clipped nodes share the same base.
    pub clip:      bool,
    /// A reference layer (CSP): what the fill tool and the brushes' "stay
    /// inside the lines" look at.
    pub reference: bool,
    /// A draft layer (CSP): shown, but left out of fills, references and
    /// exported images.
    pub draft:     bool,
}

/// A filter layer (Krita): the filter applied to what is under it, kept
/// editable. In a pass-through folder or at the top level: everything
/// under it; in an isolated folder: that folder's content under it;
/// clipped: only where its base shows.
pub struct FilterLayer {
    pub id:      u64,
    pub name:    String,
    pub visible: bool,
    /// How much of the filtered image shows over the unfiltered one.
    pub opacity: f32,
    pub locked:  bool,
    pub props:   Props,
    pub filter:  aurastudio_filters::Filter,
}

/// One paint layer: RGBA8, straight alpha, stored in tiles (only where
/// it differs from its fill color).
pub struct Layer {
    pub id:      u64,
    pub name:    String,
    pub visible: bool,
    pub opacity: f32,
    pub blend:   BlendMode,
    /// A locked layer can't be painted on or cleared.
    pub locked:  bool,
    pub tiles:   Tiles,
    /// The background layer is opaque white; clearing it resets it to
    /// white instead of transparent.
    pub background: bool,
    pub props:   Props,
}

/// How a folder combines its content with what is below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FolderBlend {
    /// Default (Krita / CSP): children blend straight onto the layers
    /// below, as if the folder weren't there; the folder opacity scales
    /// its children.
    #[default]
    PassThrough,
    /// Children are first merged together on their own, then the result
    /// is blended onto the layers below with this mode.
    Isolated(BlendMode),
}

pub struct Folder {
    pub id:        u64,
    pub name:      String,
    pub visible:   bool,
    pub opacity:   f32,
    pub blend:     FolderBlend,
    /// Locks every layer inside.
    pub locked:    bool,
    /// Children hidden in the layer list (UI only).
    pub collapsed: bool,
    /// Bottom to top.
    pub children:  Vec<Node>,
    pub props:     Props,
}

pub enum Node {
    Layer(Layer),
    Folder(Folder),
    Filter(FilterLayer),
}

/// `$e` on the node's struct, whatever its kind.
macro_rules! each {
    ($n:expr, $x:ident => $e:expr) => {
        match $n { Node::Layer($x) => $e, Node::Folder($x) => $e, Node::Filter($x) => $e }
    };
}

impl Node {
    pub fn id(&self) -> u64 { each!(self, n => n.id) }
    pub fn name(&self) -> &str { each!(self, n => &n.name) }
    pub fn name_mut(&mut self) -> &mut String { each!(self, n => &mut n.name) }
    pub fn visible(&self) -> bool { each!(self, n => n.visible) }
    pub fn visible_mut(&mut self) -> &mut bool { each!(self, n => &mut n.visible) }
    pub fn opacity(&self) -> f32 { each!(self, n => n.opacity) }
    pub fn opacity_mut(&mut self) -> &mut f32 { each!(self, n => &mut n.opacity) }
    pub fn locked(&self) -> bool { each!(self, n => n.locked) }
    pub fn locked_mut(&mut self) -> &mut bool { each!(self, n => &mut n.locked) }
    pub fn props(&self) -> Props { each!(self, n => n.props) }
    pub fn props_mut(&mut self) -> &mut Props { each!(self, n => &mut n.props) }
    pub fn is_folder(&self) -> bool {
        matches!(self, Node::Folder(_))
    }
    /// Short blend label for the UI ("Normal", "Pass Through"...).
    pub fn blend_name(&self) -> &'static str {
        match self {
            Node::Layer(l) => l.blend.name(),
            Node::Folder(f) => match f.blend {
                FolderBlend::PassThrough => "Pass Through",
                FolderBlend::Isolated(m) => m.name(),
            },
            Node::Filter(f) => f.filter.kind.name(),
        }
    }
}

/// Pixel a layer is cleared to: opaque white for the background,
/// transparent for the others. Transparent pixels keep a white RGB so a
/// compositing bug that ignores alpha shows white, not black.
fn clear_pixel(background: bool) -> [u8; 4] {
    if background { [255, 255, 255, 255] } else { [255, 255, 255, 0] }
}

impl Layer {
    pub fn new(width: u32, height: u32, background: bool) -> Self {
        Self {
            id: 0,
            name: "Layer".into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            locked: false,
            tiles: Tiles::new(width, height, clear_pixel(background)),
            background,
            props: Props::default(),
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
    }
}

/// Where a dragged node lands relative to the one it is dropped on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place { Above, Below, Inside }

/// One row of the layer list, top to bottom, as the UI shows it.
pub struct Row {
    pub id:        u64,
    pub depth:     usize,
    pub is_folder: bool,
    pub collapsed: bool,
    /// Locked itself or through a parent folder.
    pub locked_by_parent: bool,
    /// Clipped and with a base under it (a clipped node at the bottom of
    /// its folder has none: it shows as it is).
    pub clipped:   bool,
    /// Reference / draft through a parent folder.
    pub reference_by_parent: bool,
    pub draft_by_parent: bool,
}

/// Coverage of one tile in the stroke mask (one byte per pixel).
type MaskTile = Box<[u8; (TILE * TILE) as usize]>;

/// Per-stroke buffers of the layer being painted, by tile: what the layer
/// was before the stroke, and how much the stroke covers each pixel.
///
/// A tile gets them the first time the brush reaches it, and they go
/// when the stroke ends: a stroke costs memory only where it goes.
#[derive(Default)]
struct StrokeBuffers {
    layer: u64,
    base:  Vec<Option<Tile>>,
    mask:  Vec<Option<MaskTile>>,
    /// Merges under / over the layer, for the display (stroke_cache.rs).
    cache: stroke_cache::StrokeCache,
}

/// A paint layer seen as a paintable surface for the brush engines.
pub struct LayerSurface<'a> {
    pixels: &'a mut Tiles,
    base:   &'a mut [Option<Tile>],
    mask:   &'a mut [Option<MaskTile>],
}

impl PaintSurface for LayerSurface<'_> {
    fn width(&self)  -> u32 { self.pixels.width() }
    fn height(&self) -> u32 { self.pixels.height() }

    fn prepare(&mut self, rect: Rect) {
        let (w, h) = (self.pixels.width(), self.pixels.height());
        for ty in rect.1.min(h) / TILE..rect.3.min(h).div_ceil(TILE) {
            for tx in rect.0.min(w) / TILE..rect.2.min(w).div_ceil(TILE) {
                let i = self.pixels.index(tx, ty);
                if self.base[i].is_some() { continue; }
                // Not painted yet in this stroke: the layer is still as it
                // was before it.
                self.base[i] = Some(self.pixels.tile_mut(tx, ty).clone());
                self.mask[i] = Some(Box::new([0; (TILE * TILE) as usize]));
            }
        }
    }

    fn span_mut(&mut self, x: u32, y: u32, len: usize) -> (&mut [u8], &[u8], &mut [u8]) {
        debug_assert!(len > 0 && (x % TILE) as usize + len <= TILE as usize, "run across a tile");
        let (tx, ty) = (x / TILE, y / TILE);
        let i = self.pixels.index(tx, ty);
        if self.base[i].is_none() { self.prepare((x, y, x + 1, y + 1)); }
        let o = ((y % TILE) * TILE + x % TILE) as usize;
        let px = self.pixels.tile_mut(tx, ty);
        let base = self.base[i].as_ref().unwrap();
        let mask = self.mask[i].as_mut().unwrap();
        (&mut px[o * 4..(o + len) * 4], &base[o * 4..(o + len) * 4], &mut mask[o..o + len])
    }
}

/// Layer tree + active node, and the stroke buffers of the layer being
/// painted (kept here once, not in every layer).
pub struct LayerStack {
    /// Bottom to top.
    pub root:   Vec<Node>,
    active:     u64,
    pub width:  u32,
    pub height: u32,
    stroke:      StrokeBuffers,
    next_id:     u64,
}

/// Where a node sits: the path of indices from the root.
type Path = Vec<usize>;

fn find_path(nodes: &[Node], id: u64, path: &mut Path) -> bool {
    for (i, n) in nodes.iter().enumerate() {
        path.push(i);
        if n.id() == id { return true; }
        if let Node::Folder(f) = n {
            if find_path(&f.children, id, path) { return true; }
        }
        path.pop();
    }
    false
}

fn node_in(nodes: &[Node], id: u64) -> Option<&Node> {
    for n in nodes {
        if n.id() == id { return Some(n); }
        if let Node::Folder(f) = n {
            if let Some(found) = node_in(&f.children, id) { return Some(found); }
        }
    }
    None
}

fn siblings_mut<'a>(root: &'a mut Vec<Node>, parent: &[usize]) -> &'a mut Vec<Node> {
    let mut v = root;
    for &i in parent {
        v = match &mut v[i] {
            Node::Folder(f) => &mut f.children,
            _ => unreachable!("path goes through a layer"),
        };
    }
    v
}

fn count_layers(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| match n {
        Node::Layer(_) => 1,
        Node::Folder(f) => count_layers(&f.children),
        Node::Filter(_) => 0,
    }).sum()
}

fn first_layer_id(nodes: &[Node]) -> Option<u64> {
    for n in nodes.iter().rev() {
        match n {
            Node::Layer(l) => return Some(l.id),
            Node::Folder(f) => if let Some(id) = first_layer_id(&f.children) { return Some(id); },
            Node::Filter(_) => {}
        }
    }
    None
}

impl LayerStack {
    pub fn new(width: u32, height: u32) -> Self {
        let mut base = Layer::new(width, height, true);
        base.name = "Background".into();
        base.id = 1;
        Self {
            root: vec![Node::Layer(base)],
            active: 1,
            width,
            height,
            stroke: StrokeBuffers::default(),
            next_id: 2,
        }
    }

    /// A new document's layers (CSP): a locked white "Paper" at the
    /// bottom, so it can't be painted on by mistake, and an empty
    /// "Layer 1" above it, active.
    pub fn new_document(width: u32, height: u32) -> Self {
        let mut s = Self::new(width, height);
        if let Some(Node::Layer(paper)) = s.root.first_mut() {
            paper.name = "Paper".into();
            paper.locked = true;
        }
        let id = s.add_layer("Layer 1");
        s.set_active(id);
        s
    }

    /// A new document from a picture (an opened PNG / JPG...): the picture
    /// is the bottom layer, as it is (no Paper under it), and an empty
    /// "Layer 1" above it is active, so painting never lands on the picture
    /// by mistake.
    pub fn from_image(width: u32, height: u32, name: &str, rgba: &[u8]) -> Self {
        let mut s = Self::new(width, height);
        if let Some(Node::Layer(l)) = s.root.first_mut() {
            l.background = false;
            l.name = name.to_owned();
            l.tiles = Tiles::from_rgba(width, height, rgba, clear_pixel(false));
        }
        let id = s.add_layer("Layer 1");
        s.set_active(id);
        s
    }

    /// Rebuild a stack read from a file. Checked, never trusted: every
    /// layer must have `width * height` pixels, ids must be unique, and
    /// there must be at least one paint layer. An unknown active id falls
    /// back to the topmost layer.
    pub fn from_parts(width: u32, height: u32, root: Vec<Node>, active: u64) -> Result<Self, String> {
        fn walk(nodes: &[Node], size: (u32, u32), ids: &mut std::collections::HashSet<u64>) -> Result<(), String> {
            for n in nodes {
                if n.id() == 0 || !ids.insert(n.id()) { return Err(format!("duplicate or zero layer id {}", n.id())); }
                match n {
                    Node::Layer(l) if (l.tiles.width(), l.tiles.height()) != size => {
                        return Err(format!("layer {} is {}x{}, not the canvas size", l.id, l.tiles.width(), l.tiles.height()));
                    }
                    Node::Layer(_) | Node::Filter(_) => {}
                    Node::Folder(f) => walk(&f.children, size, ids)?,
                }
            }
            Ok(())
        }
        if width == 0 || height == 0 { return Err("empty canvas".into()); }
        let mut ids = std::collections::HashSet::new();
        walk(&root, (width, height), &mut ids)?;
        if count_layers(&root) == 0 { return Err("no paint layer".into()); }
        let next_id = ids.iter().max().copied().unwrap_or(0) + 1;
        let mut s = Self { root, active: 0, width, height, stroke: StrokeBuffers::default(), next_id };
        s.active = if ids.contains(&active) { active } else { first_layer_id(&s.root).unwrap_or(0) };
        Ok(s)
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn path(&self, id: u64) -> Option<Path> {
        let mut p = Vec::new();
        find_path(&self.root, id, &mut p).then_some(p)
    }

    pub fn active_id(&self) -> u64 { self.active }

    pub fn set_active(&mut self, id: u64) {
        if self.path(id).is_some() { self.active = id; }
    }

    pub fn node(&self, id: u64) -> Option<&Node> {
        node_in(&self.root, id)
    }

    pub fn node_mut(&mut self, id: u64) -> Option<&mut Node> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        siblings_mut(&mut self.root, parent).get_mut(*last)
    }

    pub fn active_node(&self) -> &Node {
        self.node(self.active).expect("active node exists")
    }

    pub fn active_node_mut(&mut self) -> &mut Node {
        let id = self.active;
        self.node_mut(id).expect("active node exists")
    }

    /// True if the active node or one of its parent folders is locked.
    pub fn is_locked(&self, id: u64) -> bool {
        let Some(path) = self.path(id) else { return false };
        let mut nodes = &self.root;
        for &i in &path {
            let n = &nodes[i];
            if n.locked() { return true; }
            if let Node::Folder(f) = n { nodes = &f.children; }
        }
        false
    }

    /// The active node is a paint layer that can be painted on.
    pub fn can_paint(&self) -> bool {
        matches!(self.active_node(), Node::Layer(_)) && !self.is_locked(self.active)
    }

    pub fn layer_count(&self) -> usize { count_layers(&self.root) }

    /// Where a filter on the active layer works: the selection's bounds,
    /// else what is painted (a layer whose empty parts are transparent),
    /// else the whole canvas (the paper changes everywhere); grown by
    /// `margin` px for the filters that reach out (a blur, a shadow).
    /// None: the active layer can't be painted; an empty rect: nothing
    /// painted.
    pub fn filter_area(&self, selection: Option<&Selection>, margin: u32) -> Option<(u64, Rect)> {
        if !self.can_paint() { return None; }
        let id = self.active;
        let Node::Layer(l) = self.node(id)? else { return None };
        let (w, h) = (self.width, self.height);
        // What shows counts, not the tiles in memory: a layer painted then
        // erased, or a stroke undone, keeps transparent tiles.
        let transparent = l.tiles.fill()[3] == 0;
        let area = match (selection, transparent) {
            (Some(s), true) if !l.tiles.any_visible_in(s.bbox()) => return Some((id, (0, 0, 0, 0))),
            (Some(s), _) => s.bbox(),
            (None, true) => match l.tiles.visible_bounds() { Some(b) => b, None => return Some((id, (0, 0, 0, 0))) },
            (None, false) => (0, 0, w, h),
        };
        if area.2 <= area.0 || area.3 <= area.1 { return Some((id, (0, 0, 0, 0))); }
        let m = margin;
        Some((id, (area.0.saturating_sub(m), area.1.saturating_sub(m), (area.2 + m).min(w), (area.3 + m).min(h))))
    }


    /// Where a new node goes: right above the active node in the same
    /// folder, or at the top inside the active folder.
    fn insert_point(&mut self) -> (Path, usize) {
        let path = self.path(self.active).expect("active node exists");
        if let Node::Folder(f) = self.active_node_mut() {
            f.collapsed = false;
            let len = f.children.len();
            return (path, len);
        }
        let (last, parent) = path.split_last().unwrap();
        (parent.to_vec(), last + 1)
    }

    fn insert(&mut self, node: Node) -> u64 {
        let id = node.id();
        let (parent, index) = self.insert_point();
        siblings_mut(&mut self.root, &parent).insert(index, node);
        self.active = id;
        id
    }

    pub fn add_layer(&mut self, name: &str) -> u64 {
        let mut layer = Layer::new(self.width, self.height, false);
        layer.id = self.new_id();
        layer.name = name.into();
        self.insert(Node::Layer(layer))
    }

    pub fn add_folder(&mut self, name: &str) -> u64 {
        let folder = Folder {
            id: self.new_id(),
            name: name.into(),
            visible: true,
            opacity: 1.0,
            blend: FolderBlend::PassThrough,
            locked: false,
            collapsed: false,
            children: Vec::new(),
            props: Props::default(),
        };
        self.insert(Node::Folder(folder))
    }

    /// A filter layer with `filter`, above the active node (or at the top
    /// of the active folder), made active.
    pub fn add_filter_layer(&mut self, filter: aurastudio_filters::Filter) -> u64 {
        let layer = FilterLayer {
            id: self.new_id(),
            name: filter.kind.name().into(),
            visible: true,
            opacity: 1.0,
            locked: false,
            props: Props::default(),
            filter,
        };
        self.insert(Node::Filter(layer))
    }

    /// Put the active node into a new folder in its place (Ctrl+G), the
    /// folder active. Returns its id.
    pub fn group_active(&mut self, name: &str) -> u64 {
        let path = self.path(self.active).expect("active node exists");
        let (last, parent) = path.split_last().unwrap();
        let siblings = siblings_mut(&mut self.root, parent);
        let node = siblings.remove(*last);
        let id = self.new_id();
        let folder = Folder {
            id, name: name.into(), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough,
            locked: false, collapsed: false, children: vec![node], props: Props::default(),
        };
        siblings_mut(&mut self.root, parent).insert(*last, Node::Folder(folder));
        self.active = id;
        id
    }

    /// Take the active folder's content out, in its place, and remove the
    /// folder (its opacity and blend mode are lost, as in Krita). False if
    /// the active node isn't a folder.
    pub fn ungroup_active(&mut self) -> bool {
        let path = self.path(self.active).expect("active node exists");
        let (last, parent) = path.split_last().unwrap();
        let siblings = siblings_mut(&mut self.root, parent);
        if !siblings[*last].is_folder() { return false; }
        let Node::Folder(f) = siblings.remove(*last) else { unreachable!() };
        let top = f.children.last().map(|n| n.id());
        for (k, child) in f.children.into_iter().enumerate() { siblings.insert(last + k, child); }
        self.active = top.or_else(|| first_layer_id(&self.root)).unwrap_or(0);
        true
    }

    /// The node `id` is clipped and shows only where its base does: the
    /// base, or None if it isn't clipped or has nothing under it.
    pub fn clip_base(&self, id: u64) -> Option<u64> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        let mut nodes = &self.root;
        for &i in parent { if let Node::Folder(f) = &nodes[i] { nodes = &f.children; } }
        if !nodes[*last].props().clip { return None; }
        nodes[..*last].iter().rev().find(|n| !n.props().clip).map(|n| n.id())
    }

    /// Ids of the nodes with a reference mark (themselves or through a
    /// folder), visible.
    pub fn has_reference(&self) -> bool {
        fn any(nodes: &[Node]) -> bool {
            nodes.iter().any(|n| n.visible() && (n.props().reference || matches!(n, Node::Folder(f) if any(&f.children))))
        }
        any(&self.root)
    }

    fn clone_node(&mut self, n: &Node) -> Node {
        match n {
            Node::Layer(l) => Node::Layer(Layer {
                id: self.new_id(),
                name: l.name.clone(),
                visible: l.visible,
                opacity: l.opacity,
                blend: l.blend,
                locked: l.locked,
                // A copy of the Paper is a plain layer of white pixels.
                tiles: if l.background {
                    Tiles::from_rgba(l.tiles.width(), l.tiles.height(), &l.tiles.to_rgba(), clear_pixel(false))
                } else {
                    l.tiles.clone()
                },
                background: false,
                props: l.props,
            }),
            Node::Folder(f) => {
                let children = f.children.iter().map(|c| self.clone_node(c)).collect();
                Node::Folder(Folder {
                    id: self.new_id(),
                    name: f.name.clone(),
                    visible: f.visible,
                    opacity: f.opacity,
                    blend: f.blend,
                    locked: f.locked,
                    collapsed: f.collapsed,
                    children,
                    props: f.props,
                })
            }
            Node::Filter(f) => Node::Filter(FilterLayer {
                id: self.new_id(),
                name: f.name.clone(),
                visible: f.visible,
                opacity: f.opacity,
                locked: f.locked,
                props: f.props,
                filter: f.filter.clone(),
            }),
        }
    }

    /// Copy of the active node (a folder is copied with its content),
    /// placed right above it and made active.
    pub fn duplicate_active(&mut self) -> u64 {
        let path = self.path(self.active).unwrap();
        let (last, parent) = path.split_last().unwrap();
        // Temporarily take the node out to clone it while borrowing self.
        let original = siblings_mut(&mut self.root, parent).remove(*last);
        let mut copy = self.clone_node(&original);
        *copy.name_mut() = format!("{} copy", original.name());
        let siblings = siblings_mut(&mut self.root, parent);
        siblings.insert(*last, original);
        let id = copy.id();
        siblings.insert(last + 1, copy);
        self.active = id;
        id
    }

    /// Why the active node can't be deleted: locked (itself, a parent, or
    /// something inside a folder), or the last paint layer.
    pub fn removal_refused(&self) -> Option<&'static str> {
        fn any_locked(n: &Node) -> bool {
            n.locked() || matches!(n, Node::Folder(f) if f.children.iter().any(any_locked))
        }
        let node = self.active_node();
        if self.is_locked(self.active) || any_locked(node) { return Some("Locked: unlock it first"); }
        let removed = match node { Node::Layer(_) => 1, Node::Folder(f) => count_layers(&f.children), Node::Filter(_) => 0 };
        (self.layer_count() == removed).then_some("Can't delete the last layer")
    }

    /// Remove the active node (a folder with its content), unless
    /// `removal_refused`.
    pub fn remove_active(&mut self) -> bool {
        if self.removal_refused().is_some() { return false; }
        let path = self.path(self.active).unwrap();
        let (last, parent) = path.split_last().unwrap();
        let siblings = siblings_mut(&mut self.root, parent);
        siblings.remove(*last);
        // Next active: the node that was below, else above, else the parent.
        self.active = if *last > 0 {
            siblings[last - 1].id()
        } else if let Some(n) = siblings.first() {
            n.id()
        } else if !parent.is_empty() {
            let mut nodes = &self.root;
            let mut id = 0;
            for &i in parent {
                id = nodes[i].id();
                if let Node::Folder(f) = &nodes[i] { nodes = &f.children; }
            }
            id
        } else {
            first_layer_id(&self.root).unwrap()
        };
        true
    }

    /// Move the active node one step up or down. Moving onto a folder goes
    /// inside it; moving past the top / bottom of a folder goes out of it.
    pub fn move_active(&mut self, up: bool) {
        let path = self.path(self.active).unwrap();
        let (last, parent) = path.split_last().unwrap();
        let last = *last;
        let len = siblings_mut(&mut self.root, parent).len();

        let neighbor = if up { (last + 1 < len).then_some(last + 1) } else { last.checked_sub(1) };
        match neighbor {
            Some(n) => {
                let siblings = siblings_mut(&mut self.root, parent);
                if siblings[n].is_folder() {
                    // Enter the folder: at its bottom when going up, at its
                    // top when going down.
                    let node = siblings.remove(last);
                    let n = if n > last { n - 1 } else { n };
                    if let Node::Folder(f) = &mut siblings[n] {
                        f.collapsed = false;
                        if up { f.children.insert(0, node) } else { f.children.push(node) }
                    }
                } else {
                    siblings.swap(last, n);
                }
            }
            None if !parent.is_empty() => {
                // Leave the folder, landing right above / below it.
                let node = siblings_mut(&mut self.root, parent).remove(last);
                let (folder_index, grand) = parent.split_last().unwrap();
                let outer = siblings_mut(&mut self.root, grand);
                outer.insert(if up { folder_index + 1 } else { *folder_index }, node);
            }
            None => {}
        }
    }

    /// Move node `id` next to `target` (drag and drop in the layer list),
    /// and make it active. False if it can't go there (into itself).
    pub fn move_node(&mut self, id: u64, target: u64, place: Place) -> bool {
        if id == target { return false; }
        let (Some(from), Some(to)) = (self.path(id), self.path(target)) else { return false };
        // A folder can't go inside itself.
        if to.starts_with(&from) { return false; }
        let (last, parent) = from.split_last().unwrap();
        let node = siblings_mut(&mut self.root, parent).remove(*last);
        let to = self.path(target).unwrap();
        let (t_last, t_parent) = to.split_last().unwrap();
        match place {
            Place::Inside => {
                let folder = &mut siblings_mut(&mut self.root, t_parent)[*t_last];
                if let Node::Folder(f) = folder {
                    f.collapsed = false;
                    f.children.push(node);
                } else {
                    siblings_mut(&mut self.root, t_parent).insert(t_last + 1, node);
                }
            }
            Place::Above => siblings_mut(&mut self.root, t_parent).insert(t_last + 1, node),
            Place::Below => siblings_mut(&mut self.root, t_parent).insert(*t_last, node),
        }
        self.active = id;
        true
    }

    /// Erase the active layer's content (Delete key). No-op on folders and
    /// locked layers. Returns true if something was cleared.
    pub fn clear_active(&mut self) -> bool {
        if !self.can_paint() { return false; }
        if let Node::Layer(l) = self.active_node_mut() { l.clear(); }
        true
    }

    /// A stroke starts on paint layer `id` (by id: another may get selected
    /// while it is painted): fresh snapshot and coverage, so it never blends
    /// with itself. False if `id` is not a paint layer.
    pub fn begin_stroke(&mut self, id: u64) -> bool {
        if !matches!(node_in(&self.root, id), Some(Node::Layer(_))) { return false; }
        let tiles = (self.width.div_ceil(TILE) * self.height.div_ceil(TILE)) as usize;
        self.stroke = StrokeBuffers {
            layer: id,
            base: (0..tiles).map(|_| None).collect(),
            mask: (0..tiles).map(|_| None).collect(),
            cache: Default::default(),
        };
        true
    }

    /// The stroke is over (recorded for undo, or abandoned): free its
    /// buffers, and the tiles it left all fill color (erased).
    pub fn end_stroke(&mut self) {
        let s = std::mem::take(&mut self.stroke);
        let Some(Node::Layer(l)) = self.node_mut(s.layer) else { return };
        let cols = l.tiles.cols();
        for (i, b) in s.base.iter().enumerate() {
            if b.is_some() { l.tiles.drop_if_fill(i as u32 % cols, i as u32 / cols); }
        }
    }

    /// Pixels of `rect` in the stroke's layer as it was when the stroke
    /// began (the undo data of the stroke). RGBA rows, tightly packed.
    pub fn stroke_base_rect(&self, rect: Rect) -> Vec<u8> {
        let s = &self.stroke;
        let Some(Node::Layer(l)) = node_in(&self.root, s.layer) else { return Vec::new() };
        let t = &l.tiles;
        let fill = t.fill();
        let mut out = Vec::with_capacity((rect.2 - rect.0) as usize * (rect.3 - rect.1) as usize * 4);
        for y in rect.1..rect.3 {
            t.for_row(y, rect.0, rect.2, |x, n, px| {
                // Tiles the stroke reached have their snapshot; the others
                // are still as they were.
                let i = t.index(x / TILE, y / TILE);
                let o = (((y % TILE) * TILE + x % TILE) * 4) as usize;
                match (s.base.get(i).and_then(|b| b.as_ref()), px) {
                    (Some(b), _) => out.extend_from_slice(&b[o..o + n * 4]),
                    (None, Some(px)) => out.extend_from_slice(px),
                    (None, None) => for _ in 0..n { out.extend_from_slice(&fill); },
                }
            });
        }
        out
    }

    /// Pixels of `rect` in paint layer `id`, RGBA rows, tightly packed.
    pub fn layer_rect(&self, id: u64, rect: Rect) -> Option<Vec<u8>> {
        match self.node(id)? {
            Node::Layer(l) => Some(l.tiles.read_rect(rect)),
            Node::Folder(_) | Node::Filter(_) => None,
        }
    }

    /// Write `data` (from layer_rect / stroke_base_rect) back into `rect`
    /// of paint layer `id`. False if the layer is gone or the size is off.
    pub fn set_layer_rect(&mut self, id: u64, rect: Rect, data: &[u8]) -> bool {
        let (rw, rh) = ((rect.2.saturating_sub(rect.0)) as usize, (rect.3.saturating_sub(rect.1)) as usize);
        if rect.2 > self.width || rect.3 > self.height || data.len() != rw * rh * 4 { return false; }
        let Some(Node::Layer(l)) = self.node_mut(id) else { return false };
        l.tiles.write_rect(rect, data);
        true
    }

    /// Paint `color` over the pixels of `area` in paint layer `id` (the
    /// fill tool). Returns the rectangle changed.
    pub fn fill_area(&mut self, id: u64, area: &fill::Area, color: [u8; 4]) -> Option<Rect> {
        let r = area.bbox;
        let mut px = self.layer_rect(id, r)?;
        let mut i = 0;
        for y in r.1..r.3 {
            for x in r.0..r.2 {
                if area.contains(x, y) { over(&mut px[i..i + 4], color); }
                i += 4;
            }
        }
        self.set_layer_rect(id, r, &px).then_some(r)
    }

    /// Clear the selected pixels of paint layer `id` to its fill color.
    pub fn clear_selected(&mut self, id: u64, sel: &Selection) -> Option<Rect> {
        let r = sel.bbox();
        let Some(Node::Layer(l)) = self.node(id) else { return None };
        let fill = l.tiles.fill();
        let mut px = l.tiles.read_rect(r);
        let mut i = 0;
        for y in r.1..r.3 {
            for x in r.0..r.2 {
                if sel.contains(x, y) { px[i..i + 4].copy_from_slice(&fill); }
                i += 4;
            }
        }
        self.set_layer_rect(id, r, &px).then_some(r)
    }

    /// Undo what the stroke painted in `rect` outside `sel`: painting only
    /// changes selected pixels. Call after each engine call.
    pub fn clip_stroke(&mut self, rect: Rect, sel: &Selection) {
        let (w, h) = (self.width, self.height);
        let (x0, y0, x1, y1) = (rect.0.min(w), rect.1.min(h), rect.2.min(w), rect.3.min(h));
        if x0 >= x1 || y0 >= y1 { return; }
        let id = self.stroke.layer;
        let Some(path) = self.path(id) else { return };
        let Some((last, parent)) = path.split_last() else { return };
        let Node::Layer(l) = &mut siblings_mut(&mut self.root, parent)[*last] else { return };
        let sb = sel.bbox();
        for ty in y0 / TILE..y1.div_ceil(TILE) {
            for tx in x0 / TILE..x1.div_ceil(TILE) {
                let (tx0, ty0) = (tx * TILE, ty * TILE);
                // Tiles all inside a rectangle selection need nothing.
                let i = l.tiles.index(tx, ty);
                let Some(base) = self.stroke.base.get(i).and_then(|b| b.as_ref()) else { continue };
                let inside_box = tx0 >= sb.0 && ty0 >= sb.1 && tx0 + TILE <= sb.2 && ty0 + TILE <= sb.3;
                if inside_box && sel.is_rect() { continue; }
                let px = l.tiles.tile_mut(tx, ty);
                for y in y0.max(ty0)..y1.min(ty0 + TILE) {
                    for x in x0.max(tx0)..x1.min(tx0 + TILE) {
                        if sel.contains(x, y) { continue; }
                        let o = (((y - ty0) * TILE + x - tx0) * 4) as usize;
                        px[o..o + 4].copy_from_slice(&base[o..o + 4]);
                    }
                }
            }
        }
    }

    /// Paint layer `id` as a brush surface, with the stroke buffers of
    /// begin_stroke; None if it is gone, a folder, or not the layer the
    /// stroke began on.
    pub fn layer_surface(&mut self, id: u64) -> Option<LayerSurface<'_>> {
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        let Node::Layer(l) = &mut siblings_mut(&mut self.root, parent)[*last] else { return None };
        // Only the layer the stroke began on has stroke buffers.
        if self.stroke.layer != id || self.stroke.base.is_empty() { return None; }
        Some(LayerSurface {
            pixels: &mut l.tiles,
            base:   &mut self.stroke.base,
            mask:   &mut self.stroke.mask,
        })
    }

    /// Ids of every paint layer (thumbnails).
    pub fn layer_ids(&self) -> Vec<u64> {
        fn walk(nodes: &[Node], out: &mut Vec<u64>) {
            for n in nodes {
                match n {
                    Node::Layer(l) => out.push(l.id),
                    Node::Folder(f) => walk(&f.children, out),
                    Node::Filter(_) => {}
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }

    /// Rows of the layer list, top to bottom, skipping collapsed content.
    pub fn rows(&self) -> Vec<Row> {
        #[derive(Clone, Copy)]
        struct Up { locked: bool, reference: bool, draft: bool }
        fn walk(nodes: &[Node], depth: usize, up: Up, out: &mut Vec<Row>) {
            for (i, n) in nodes.iter().enumerate().rev() {
                let (is_folder, collapsed) = match n {
                    Node::Folder(f) => (true, f.collapsed),
                    _ => (false, false),
                };
                let clipped = n.props().clip && nodes[..i].iter().any(|b| !b.props().clip);
                out.push(Row { id: n.id(), depth, is_folder, collapsed, locked_by_parent: up.locked, clipped,
                               reference_by_parent: up.reference, draft_by_parent: up.draft });
                if let Node::Folder(f) = n {
                    if !f.collapsed {
                        let p = f.props;
                        walk(&f.children, depth + 1, Up { locked: up.locked || f.locked, reference: up.reference || p.reference, draft: up.draft || p.draft }, out);
                    }
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, 0, Up { locked: false, reference: false, draft: false }, &mut v);
        v
    }

    /// Change the canvas size, keeping the content centered (Krita /
    /// Photoshop "canvas size" with the default center anchor): growing
    /// adds empty margins, shrinking crops the borders. Nothing is scaled.
    pub fn resize(&mut self, width: u32, height: u32) {
        fn walk(nodes: &mut [Node], f: &mut dyn FnMut(&mut Layer)) {
            for n in nodes {
                match n {
                    Node::Layer(l) => f(l),
                    Node::Folder(fo) => walk(&mut fo.children, f),
                    Node::Filter(_) => {}
                }
            }
        }
        // A stroke in progress can't go on at the new size.
        self.end_stroke();
        // Position of the old canvas's top-left corner in the new one.
        let dx = (width as i64 - self.width as i64) / 2;
        let dy = (height as i64 - self.height as i64) / 2;
        walk(&mut self.root, &mut |layer: &mut Layer| {
            layer.tiles = layer.tiles.resized(width, height, dx, dy);
        });
        self.width  = width;
        self.height = height;
    }

    /// The merged image of `rect` (clipped to the canvas), RGBA rows
    /// tightly packed.
    pub fn composite_rect(&self, rect: Rect) -> Vec<u8> {
        let r = (rect.0, rect.1, rect.2.min(self.width), rect.3.min(self.height));
        if r.0 >= r.2 || r.1 >= r.3 { return Vec::new(); }
        let mut out = vec![0u8; (r.2 - r.0) as usize * (r.3 - r.1) as usize * 4];
        self.composite_rect_to(r, &mut out);
        out
    }

    /// Composite of `rect` (inside the canvas) into `out`, RGBA rows tightly
    /// packed. Always opaque (white under everything): blended straight
    /// into the bytes, Normal in integer math; only isolated folders take
    /// the premultiplied path. ~5 ns/px (examples/bench_composite.rs).
    pub fn composite_rect_to(&self, rect: Rect, out: &mut [u8]) {
        let (x0, y0, x1, y1) = rect;
        if x0 >= x1 || y0 >= y1 { return; }
        // Clipping or filter layers: the general merge (compose.rs).
        if self.needs_general_merge() { return self.general_to(rect, Pick::ALL, false, out); }
        let n = (x1 - x0) as usize;
        debug_assert_eq!(out.len(), n * (y1 - y0) as usize * 4);
        let mut group: Vec<Premul> = Vec::new();
        for (y, dst) in (y0..y1).zip(out.chunks_exact_mut(n * 4)) {
            // White behind everything.
            dst.fill(255);
            composite_row_opaque(&self.root, y, x0, dst, 1.0, &mut group);
        }
    }

    /// Small merged image (at most `max` px), without merging the whole
    /// canvas: one row in every few is merged, then shrunk across. For file
    /// previews; a big canvas takes a few ms instead of a full merge.
    pub fn composite_preview(&self, max: u32) -> (u32, u32, Vec<u8>) {
        let (w, h) = (self.width, self.height);
        let k = (max as f32 / w.max(h) as f32).min(1.0);
        let (tw, th) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
        let mut out = Vec::with_capacity(tw as usize * th as usize * 4);
        let mut row = vec![0u8; w as usize * 4];
        // Filter layers read around each pixel: one merge of everything
        // costs less than every sampled row with its margin.
        let full = self.needs_general_merge().then(|| self.composite_rect((0, 0, w, h)));
        for ty in 0..th {
            let y = ((ty as u64 * 2 + 1) * h as u64 / (th as u64 * 2)) as u32;
            match &full {
                Some(f) => row.copy_from_slice(&f[(y * w * 4) as usize..((y + 1) * w * 4) as usize]),
                None => self.composite_rect_to((0, y, w, y + 1), &mut row),
            }
            for tx in 0..tw {
                let (a, b) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
                let mut sum = [0u32; 4];
                for p in row[a as usize * 4..b as usize * 4].chunks_exact(4) {
                    for c in 0..4 { sum[c] += p[c] as u32; }
                }
                let n = b - a;
                out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
        (tw, th, out)
    }
}

/// Blend `nodes` (bottom to top) onto an opaque RGBA row `dst`, for canvas
/// row `y` from column `x0`. `opacity_scale` carries pass-through folder
/// opacity down to the children.
fn composite_row_opaque(
    nodes: &[Node], y: u32, x0: u32, dst: &mut [u8], opacity_scale: f32, group: &mut Vec<Premul>,
) {
    let n = dst.len() / 4;
    for node in nodes {
        if !node.visible() || node.opacity() <= 0.0 { continue; }
        match node {
            Node::Layer(l) => {
                // Tile by tile; nothing to do where an empty layer has no
                // tiles (an empty layer costs no time here either).
                let (op, fill) = (l.opacity * opacity_scale, l.tiles.fill());
                let run = fill_run(fill);
                l.tiles.for_row(y, x0, x0 + n as u32, |x, len, px| {
                    let d = &mut dst[(x - x0) as usize * 4..((x - x0) as usize + len) * 4];
                    match px {
                        Some(src) => blend_layer_opaque(d, src, op, l.blend),
                        None if fill[3] == 0 => {}
                        None => blend_layer_opaque(d, &run[..len * 4], op, l.blend),
                    }
                });
            }
            // (Trees with filter layers take the general merge.)
            Node::Filter(_) => {}
            Node::Folder(f) => match f.blend {
                FolderBlend::PassThrough => {
                    composite_row_opaque(&f.children, y, x0, dst, opacity_scale * f.opacity, group);
                }
                FolderBlend::Isolated(mode) => {
                    // Children merged on their own (transparent start, so
                    // the general premultiplied path), then blended down.
                    group.clear();
                    group.resize(n, [0.0; 4]);
                    composite_row(&f.children, y, x0, group, 1.0);
                    let op = f.opacity * opacity_scale;
                    for (d, g) in dst.chunks_exact_mut(4).zip(group.iter()) {
                        if g[3] <= 0.0 { continue; }
                        let inv = 1.0 / g[3];
                        blend_px_opaque(d, [g[0] * inv, g[1] * inv, g[2] * inv], g[3] * op, mode);
                    }
                }
            },
        }
    }
}

/// Blend one straight-alpha RGBA layer row onto an opaque row.
#[inline]
fn blend_layer_opaque(dst: &mut [u8], src: &[u8], opacity: f32, mode: BlendMode) {
    let op8 = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    if op8 == 0 { return; }
    if mode == BlendMode::Normal {
        // dst += (src - dst) * a, a = alpha * opacity, in blocks: skip a
        // fully transparent block, else plain u16 math over every byte
        // (the compiler vectorizes it; a per-pixel branch doesn't).
        const B: usize = 64;
        for (dc, sc) in dst.chunks_mut(B * 4).zip(src.chunks(B * 4)) {
            // OR of whole pixels (vectorizes, a strided byte read doesn't).
            let mut or = 0u32;
            for p in sc.chunks_exact(4) { or |= u32::from_le_bytes([p[0], p[1], p[2], p[3]]); }
            if or >> 24 == 0 { continue; }
            // Each pixel's alpha for its color bytes, 0 for its alpha byte
            // (the backdrop stays opaque).
            let mut a4 = [0u8; B * 4];
            let a4 = &mut a4[..sc.len()];
            for (d, p) in a4.chunks_exact_mut(4).zip(sc.chunks_exact(4)) {
                let a = if op8 == 255 { p[3] } else { ((p[3] as u32 * op8 + 127) / 255) as u8 };
                d.copy_from_slice(&[a, a, a, 0]);
            }
            for ((d, &s), &a) in dc.iter_mut().zip(sc).zip(a4.iter()) {
                let a = a as u16;
                *d = ((*d as u16 * (255 - a) + s as u16 * a + 127) / 255) as u8;
            }
        }
    } else {
        let op = op8 as f32 / 255.0;
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            if s[3] == 0 { continue; }
            let cs = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
            blend_px_opaque(d, cs, s[3] as f32 / 255.0 * op, mode);
        }
    }
}

/// Blend straight color `cs` at alpha `a` onto one opaque pixel: with an
/// opaque backdrop, W3C compositing reduces to cb + a * (B(cb, cs) - cb).
#[inline]
fn blend_px_opaque(d: &mut [u8], cs: [f32; 3], a: f32, mode: BlendMode) {
    let a = a.clamp(0.0, 1.0);
    for c in 0..3 {
        let cb = d[c] as f32 / 255.0;
        let b = mode.channel(cb, cs[c]).clamp(0.0, 1.0);
        d[c] = unit_to_u8(cb + a * (b - cb));
    }
}

/// Composite `nodes` (bottom to top) onto `row` (premultiplied), for canvas
/// row `y` starting at column `x0`. `opacity_scale` carries pass-through
/// folder opacity down to the children.
fn composite_row(nodes: &[Node], y: u32, x0: u32, row: &mut [Premul], opacity_scale: f32) {
    for node in nodes {
        if !node.visible() || node.opacity() <= 0.0 { continue; }
        match node {
            Node::Layer(l) => {
                let (op, fill) = (l.opacity * opacity_scale, l.tiles.fill());
                let run = fill_run(fill);
                l.tiles.for_row(y, x0, x0 + row.len() as u32, |x, len, px| {
                    let src = match px {
                        Some(src) => src,
                        None if fill[3] == 0 => return,
                        None => &run[..len * 4],
                    };
                    let i = (x - x0) as usize;
                    for (dst, s) in row[i..i + len].iter_mut().zip(src.chunks_exact(4)) {
                        if s[3] == 0 { continue; }
                        let a = s[3] as f32 / 255.0 * op;
                        let cs = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
                        blend_over(dst, cs, a, l.blend);
                    }
                });
            }
            Node::Filter(_) => {}
            Node::Folder(f) => match f.blend {
                FolderBlend::PassThrough => {
                    composite_row(&f.children, y, x0, row, opacity_scale * f.opacity);
                }
                FolderBlend::Isolated(mode) => {
                    let mut group: Vec<Premul> = vec![[0.0; 4]; row.len()];
                    composite_row(&f.children, y, x0, &mut group, 1.0);
                    let op = f.opacity * opacity_scale;
                    for (dst, g) in row.iter_mut().zip(&group) {
                        if g[3] <= 0.0 { continue; }
                        let cs = [g[0] / g[3], g[1] / g[3], g[2] / g[3]];
                        blend_over(dst, cs, g[3] * op, mode);
                    }
                }
            },
        }
    }
}

/// One tile row of a fill color (blended where a layer has no tile).
pub(crate) fn fill_run(fill: [u8; 4]) -> [u8; (TILE * 4) as usize] {
    let mut run = [0u8; (TILE * 4) as usize];
    for p in run.chunks_exact_mut(4) { p.copy_from_slice(&fill); }
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_and_drop_moves_nodes() {
        let mut s = LayerStack::new(8, 8);
        let a = s.add_layer("A");
        let f = s.add_folder("F");
        let b = s.add_layer("B"); // inside F
        let ids = |s: &LayerStack| s.rows().iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(&s), vec![f, b, a, 1]);
        assert!(s.move_node(a, f, Place::Inside));
        assert_eq!(ids(&s), vec![f, a, b, 1]);
        assert!(s.move_node(a, 1, Place::Below));
        assert_eq!(ids(&s), vec![f, b, 1, a]);
        assert!(s.move_node(a, f, Place::Above));
        assert_eq!(ids(&s), vec![a, f, b, 1]);
        assert!(!s.move_node(f, b, Place::Above), "a folder into itself");
        assert_eq!(s.active_id(), a);
    }

    fn layer_px(stack: &LayerStack, id: u64, x: u32, y: u32) -> [u8; 4] {
        match stack.node(id).unwrap() {
            Node::Layer(l) => l.tiles.pixel(x, y),
            _ => panic!("not a layer"),
        }
    }

    fn set_px(stack: &mut LayerStack, id: u64, rgba: [u8; 4]) {
        let (w, h) = (stack.width, stack.height);
        assert!(stack.set_layer_rect(id, (0, 0, w, h), &rgba.repeat((w * h) as usize)));
    }

    fn composite(stack: &LayerStack) -> [u8; 4] {
        stack.composite_rect((0, 0, 1, 1)).try_into().unwrap()
    }

    fn names_top_down(stack: &LayerStack) -> Vec<String> {
        stack.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), stack.node(r.id).unwrap().name())).collect()
    }

    #[test]
    fn new_document_has_locked_paper_and_layer_1() {
        let s = LayerStack::new_document(2, 2);
        assert_eq!(names_top_down(&s), ["Layer 1", "Paper"]);
        assert!(s.can_paint());
        let paper = s.rows().last().unwrap().id;
        assert!(s.is_locked(paper));
        assert_eq!(composite(&s), [255, 255, 255, 255]);
    }

    #[test]
    fn composite_flattens_opaque_base() {
        assert_eq!(composite(&LayerStack::new(2, 2)), [255, 255, 255, 255]);
    }

    #[test]
    fn transparent_layer_does_not_hide_base() {
        let mut s = LayerStack::new(2, 2);
        s.add_layer("L");
        assert_eq!(composite(&s), [255, 255, 255, 255]);
    }

    #[test]
    fn hidden_layer_is_skipped() {
        let mut s = LayerStack::new(2, 2);
        let l = s.add_layer("L");
        set_px(&mut s, l, [0, 0, 0, 255]);
        *s.node_mut(l).unwrap().visible_mut() = false;
        assert_eq!(composite(&s), [255, 255, 255, 255]);
    }

    #[test]
    fn layer_blend_mode_is_used() {
        let mut s = LayerStack::new(2, 2);
        set_px(&mut s, 1, [128, 128, 128, 255]);
        let l = s.add_layer("L");
        set_px(&mut s, l, [255, 0, 0, 255]);
        if let Some(Node::Layer(layer)) = s.node_mut(l) { layer.blend = BlendMode::Multiply; }
        assert_eq!(composite(&s), [128, 0, 0, 255]);
    }

    #[test]
    fn isolated_folder_blends_its_merged_content() {
        // Folder in Multiply holding an opaque red layer over mid gray.
        let mut s = LayerStack::new(2, 2);
        set_px(&mut s, 1, [128, 128, 128, 255]);
        let f = s.add_folder("F");
        let l = s.add_layer("L"); // goes inside the active folder
        set_px(&mut s, l, [255, 0, 0, 255]);
        if let Some(Node::Folder(folder)) = s.node_mut(f) { folder.blend = FolderBlend::Isolated(BlendMode::Multiply); }
        assert_eq!(composite(&s), [128, 0, 0, 255]);
        // Pass-through with 50% folder opacity scales the child.
        if let Some(Node::Folder(folder)) = s.node_mut(f) {
            folder.blend = FolderBlend::PassThrough;
            folder.opacity = 0.5;
        }
        assert_eq!(composite(&s), [192, 64, 64, 255]);
    }

    #[test]
    fn new_layer_goes_inside_active_folder_then_above_active() {
        let mut s = LayerStack::new(2, 2);
        s.add_folder("F");
        s.add_layer("A");
        s.add_layer("B");
        assert_eq!(names_top_down(&s), ["F", "  B", "  A", "Background"]);
    }

    #[test]
    fn move_enters_and_leaves_folders() {
        let mut s = LayerStack::new(2, 2);
        s.add_folder("F");
        s.set_active(1);
        let a = s.add_layer("A"); // root, above Background, below F
        assert_eq!(names_top_down(&s), ["F", "A", "Background"]);
        s.move_active(true); // onto F: enters it
        assert_eq!(names_top_down(&s), ["F", "  A", "Background"]);
        s.move_active(true); // top of F: leaves it, lands above F
        assert_eq!(names_top_down(&s), ["A", "F", "Background"]);
        s.move_active(false); // onto F from above: enters at its top
        assert_eq!(names_top_down(&s), ["F", "  A", "Background"]);
        s.move_active(false); // bottom of F: leaves it, lands below F
        assert_eq!(names_top_down(&s), ["F", "A", "Background"]);
        assert_eq!(s.active_id(), a);
    }

    #[test]
    fn lock_blocks_painting_and_clearing_also_through_folder() {
        let mut s = LayerStack::new(2, 2);
        let f = s.add_folder("F");
        let l = s.add_layer("L");
        set_px(&mut s, l, [0, 0, 0, 255]);
        assert!(s.can_paint());
        *s.node_mut(f).unwrap().locked_mut() = true;
        assert!(!s.can_paint());
        assert!(!s.clear_active());
        assert_eq!(layer_px(&s, l, 0, 0), [0, 0, 0, 255]);
        *s.node_mut(f).unwrap().locked_mut() = false;
        *s.node_mut(l).unwrap().locked_mut() = true;
        assert!(!s.can_paint());
        assert!(s.rows().iter().all(|r| !r.locked_by_parent));
    }

    #[test]
    fn folders_cannot_be_painted_on() {
        let mut s = LayerStack::new(2, 2);
        s.add_folder("F");
        assert!(!s.can_paint());
        assert!(s.layer_surface(s.active_id()).is_none());
        assert!(!s.begin_stroke(s.active_id()));
    }

    #[test]
    fn duplicate_folder_copies_content_with_new_ids() {
        let mut s = LayerStack::new(2, 2);
        let f = s.add_folder("F");
        let l = s.add_layer("L");
        set_px(&mut s, l, [1, 2, 3, 255]);
        s.set_active(f);
        s.duplicate_active();
        assert_eq!(names_top_down(&s), ["F copy", "  L", "F", "  L", "Background"]);
        let ids = s.layer_ids();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
        let copy_l = s.rows()[1].id;
        assert_eq!(layer_px(&s, copy_l, 0, 0), [1, 2, 3, 255]);
    }

    #[test]
    fn remove_keeps_a_valid_active_and_at_least_one_layer() {
        let mut s = LayerStack::new(2, 2);
        assert!(!s.remove_active()); // last paint layer: refused
        let f = s.add_folder("F");
        s.add_layer("L");
        s.set_active(f);
        assert!(s.remove_active()); // folder with its content

        // Locked: refused, also through a parent folder.
        let a = s.add_layer("A");
        *s.node_mut(a).unwrap().locked_mut() = true;
        assert_eq!(s.removal_refused(), Some("Locked: unlock it first"));
        assert!(!s.remove_active());
        *s.node_mut(a).unwrap().locked_mut() = false;
        assert!(s.remove_active());
        assert_eq!(names_top_down(&s), ["Background"]);
        assert_eq!(s.active_id(), 1);
        assert!(s.begin_stroke(1));
    }

    #[test]
    fn clear_resets_to_white_or_transparent() {
        let mut s = LayerStack::new(2, 2);
        set_px(&mut s, 1, [0, 0, 0, 255]);
        s.clear_active();
        assert_eq!(layer_px(&s, 1, 0, 0), [255, 255, 255, 255]);
        let l = s.add_layer("L");
        set_px(&mut s, l, [0, 0, 0, 255]);
        s.clear_active();
        assert_eq!(layer_px(&s, l, 0, 0), [255, 255, 255, 0]);
    }

    #[test]
    fn resize_keeps_content_centered_inside_folders_too() {
        let mut s = LayerStack::new(4, 4);
        s.add_folder("F");
        let l = s.add_layer("L");
        assert!(s.set_layer_rect(l, (1, 1, 2, 2), &[0, 0, 0, 255]));
        s.resize(8, 8);
        assert_eq!(layer_px(&s, l, 3, 3), [0, 0, 0, 255]);
        assert_eq!(layer_px(&s, l, 0, 0), [255, 255, 255, 0]);
        assert_eq!(layer_px(&s, 1, 0, 0), [255, 255, 255, 255]);
        assert!(s.begin_stroke(l));
        assert_eq!(s.layer_surface(l).unwrap().width(), 8);
    }

    #[test]
    fn stroke_snapshot_is_filled_only_where_painted() {
        // An L-shaped stroke: its bounding box covers tiles it never
        // reached. The undo data must still be the layer as it was.
        let mut s = LayerStack::new(256, 256);
        let id = s.add_layer("ink");
        assert!(s.begin_stroke(id));
        {
            let mut surf = s.layer_surface(id).unwrap();
            for r in [(0, 0, 10, 200), (0, 190, 200, 200)] {
                surf.prepare(r);
                for y in r.1..r.3 {
                    for x in r.0..r.2 {
                        let (px, _, _) = surf.span_mut(x, y, 1);
                        px.copy_from_slice(&[255, 0, 0, 255]);
                    }
                }
            }
        }
        let before = s.stroke_base_rect((0, 0, 200, 200));
        assert!(before.chunks_exact(4).all(|p| p == [255, 255, 255, 0]), "undo data is not the original layer");
        let Some(Node::Layer(l)) = s.node(id) else { panic!() };
        // Only the tiles the L went through exist: 4 down the left column,
        // and its foot (y 190..200 crosses 192) 2 rows × 3 more across;
        // not the 16 of its bounding box.
        assert_eq!(l.tiles.stored(), 10);
        s.end_stroke();
        assert!(s.layer_surface(id).is_none(), "buffers kept after the stroke");
    }

    #[test]
    fn pieces_and_preview_match_the_whole_merge() {
        let mut s = LayerStack::new_document(300, 200);
        let id = s.active_id();
        assert!(s.set_layer_rect(id, (50, 40, 250, 160), &[10, 20, 30, 255].repeat(200 * 120)));
        let mut whole = vec![0u8; 300 * 200 * 4];
        whole.copy_from_slice(&s.composite_rect((0, 0, s.width, s.height)));
        let piece = s.composite_rect((40, 30, 400, 100));
        assert_eq!(piece.len(), 260 * 70 * 4, "clipped to the canvas");
        for (row, y) in piece.chunks_exact(260 * 4).zip(30..100) {
            let a = (y * 300 + 40) * 4;
            assert_eq!(row, &whole[a..a + 260 * 4]);
        }
        let (w, h, px) = s.composite_preview(150);
        assert_eq!((w, h), (150, 100));
        // Middle: the painted color; corner: the white paper.
        assert_eq!(&px[((50 * 150 + 75) * 4) as usize..][..4], &[10, 20, 30, 255]);
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn empty_layers_take_no_memory_and_erased_tiles_go() {
        let mut s = LayerStack::new_document(2480, 3508);
        let id = s.add_layer("ink");
        let total: usize = s.layer_ids().iter().map(|&i| match s.node(i) { Some(Node::Layer(l)) => l.tiles.bytes(), _ => 0 }).sum();
        assert_eq!(total, 0, "Paper, Layer 1 and an empty layer hold no pixels");
        // Paint a dot, then erase it within a stroke: the tile goes away.
        assert!(s.begin_stroke(id));
        {
            let mut surf = s.layer_surface(id).unwrap();
            surf.prepare((100, 100, 101, 101));
            surf.span_mut(100, 100, 1).0.copy_from_slice(&[0, 0, 0, 255]);
            surf.span_mut(100, 100, 1).0.copy_from_slice(&[255, 255, 255, 0]);
        }
        s.end_stroke();
        let Some(Node::Layer(l)) = s.node(id) else { panic!() };
        assert_eq!(l.tiles.stored(), 0);
    }
}

#[cfg(test)]
mod filter_area_tests {
    use super::*;

    #[test]
    fn an_erased_layer_counts_as_empty() {
        let mut l = LayerStack::new_document(256, 256);
        let id = l.add_layer("ink");
        l.set_active(id);
        assert_eq!(l.filter_area(None, 0), Some((id, (0, 0, 0, 0))), "new layer: empty");
        let ink: Vec<u8> = [0u8, 0, 0, 255].repeat(10 * 10);
        l.set_layer_rect(id, (100, 100, 110, 110), &ink);
        let (_, r) = l.filter_area(None, 0).unwrap();
        assert!(r.2 > r.0 && r.0 <= 100 && r.2 >= 110, "{r:?}");
        // Erased: transparent pixels that keep a color, tile still stored.
        let erased: Vec<u8> = [0u8, 0, 0, 0].repeat(10 * 10);
        l.set_layer_rect(id, (100, 100, 110, 110), &erased);
        assert_eq!(l.filter_area(None, 0), Some((id, (0, 0, 0, 0))), "erased: empty");
        // A selection: what shows inside it counts.
        l.set_layer_rect(id, (100, 100, 110, 110), &ink);
        let off = Selection::rect(256, 256, (0, 0, 50, 50)).unwrap();
        let on = Selection::rect(256, 256, (105, 105, 200, 200)).unwrap();
        assert_eq!(l.filter_area(Some(&off), 0), Some((id, (0, 0, 0, 0))));
        assert_eq!(l.filter_area(Some(&on), 0), Some((id, (105, 105, 200, 200))));
        // The paper shows everywhere.
        let paper = l.add_layer("paper");
        if let Some(Node::Layer(p)) = l.node_mut(paper) { p.tiles = Tiles::new(256, 256, [255, 255, 255, 255]); }
        l.set_active(paper);
        assert_eq!(l.filter_area(Some(&off), 0), Some((paper, (0, 0, 50, 50))));
    }
}

#[cfg(test)]
mod layer_tree_tests {
    use super::*;

    #[test]
    fn group_ungroup_and_clipped_rows() {
        let mut s = LayerStack::new_document(64, 64);
        let a = s.active_id();
        let b = s.add_layer("b");
        s.node_mut(b).unwrap().props_mut().clip = true;
        let rows = s.rows();
        assert!(rows.iter().find(|r| r.id == b).unwrap().clipped);
        assert!(!rows.iter().find(|r| r.id == a).unwrap().clipped);
        // Clipped at the bottom of a folder: no base.
        s.set_active(b);
        let g = s.group_active("g");
        assert_eq!(s.active_id(), g);
        assert!(s.node(g).unwrap().is_folder());
        assert!(!s.rows().iter().find(|r| r.id == b).unwrap().clipped);
        assert_eq!(s.clip_base(b), None);
        // A reference folder marks what it holds.
        s.node_mut(g).unwrap().props_mut().reference = true;
        assert!(s.rows().iter().find(|r| r.id == b).unwrap().reference_by_parent);
        assert!(s.has_reference());
        assert!(s.ungroup_active());
        assert_eq!(s.active_id(), b);
        assert!(s.node(g).is_none());
        assert_eq!(s.clip_base(b), Some(a));
        assert!(!s.ungroup_active(), "not a folder");
        // Filter layers: no pixels, not paintable, duplicated with their filter.
        let f = s.add_filter_layer(aurastudio_filters::Filter::new(aurastudio_filters::Kind::Invert));
        assert!(!s.can_paint() && s.layer_rect(f, (0, 0, 1, 1)).is_none());
        let d = s.duplicate_active();
        assert!(matches!(s.node(d), Some(Node::Filter(x)) if x.filter.kind == aurastudio_filters::Kind::Invert));
        assert!(!s.layer_ids().contains(&f));
    }

    #[test]
    fn fill_refers_to_the_reference_layers() {
        let mut s = LayerStack::new(64, 64);
        if let Some(Node::Layer(bg)) = s.root.first_mut() { bg.tiles = Tiles::new(64, 64, [255, 255, 255, 255]); }
        // A wall on the ink layer, a sketch line (draft) and a color patch.
        let ink = s.add_layer("ink");
        s.set_layer_rect(ink, (32, 0, 34, 64), &[0u8, 0, 0, 255].repeat(128));
        let sketch = s.add_layer("sketch");
        s.set_layer_rect(sketch, (16, 0, 17, 64), &[0u8, 0, 255, 255].repeat(64));
        s.node_mut(sketch).unwrap().props_mut().draft = true;
        let color = s.add_layer("color");
        let p = FillParams { tolerance: 10, expand: 0, ..FillParams::default() };
        // All layers, drafts skipped: the ink stops it, not the sketch.
        let a = flood(&s, color, 5, 5, p, None).unwrap();
        assert!(a.contains(20, 5) && !a.contains(40, 5));
        // Drafts counted: the sketch stops it too.
        let a = flood(&s, color, 5, 5, FillParams { skip_drafts: false, ..p }, None).unwrap();
        assert!(!a.contains(20, 5));
        // References only, the ink marked: same as ink alone.
        s.node_mut(ink).unwrap().props_mut().reference = true;
        let a = flood(&s, color, 5, 5, FillParams { refer: Refer::References, skip_drafts: false, ..p }, None).unwrap();
        assert!(a.contains(20, 5) && !a.contains(40, 5));
        // A brush limit: the area on the line art, as a selection.
        let sel = Selection::from_area(64, 64, &a).unwrap();
        assert!(sel.contains(5, 5) && !sel.contains(40, 5));
    }
}
