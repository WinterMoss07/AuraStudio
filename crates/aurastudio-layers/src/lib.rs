//! Layer tree: paint layers and folders (groups), like Krita / CSP.
//!
//! Stored bottom to top. Each node has a stable `id`; the active node is
//! tracked by id, so reordering or moving in and out of folders never
//! changes which layer you paint on.

use aurastudio_canvas::blend::{blend_over, unit_to_u8, Premul};
use aurastudio_canvas::{BlendMode, PaintSurface, Rect};

/// One paint layer: RGBA8, straight alpha, `width * height * 4` bytes.
pub struct Layer {
    pub id:      u64,
    pub name:    String,
    pub visible: bool,
    pub opacity: f32,
    pub blend:   BlendMode,
    /// A locked layer can't be painted on or cleared.
    pub locked:  bool,
    pub pixels:  Vec<u8>,
    /// The background layer is opaque white; clearing it resets it to
    /// white instead of transparent.
    pub background: bool,
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
}

pub enum Node {
    Layer(Layer),
    Folder(Folder),
}

impl Node {
    pub fn id(&self) -> u64 {
        match self { Node::Layer(l) => l.id, Node::Folder(f) => f.id }
    }
    pub fn name(&self) -> &str {
        match self { Node::Layer(l) => &l.name, Node::Folder(f) => &f.name }
    }
    pub fn name_mut(&mut self) -> &mut String {
        match self { Node::Layer(l) => &mut l.name, Node::Folder(f) => &mut f.name }
    }
    pub fn visible(&self) -> bool {
        match self { Node::Layer(l) => l.visible, Node::Folder(f) => f.visible }
    }
    pub fn visible_mut(&mut self) -> &mut bool {
        match self { Node::Layer(l) => &mut l.visible, Node::Folder(f) => &mut f.visible }
    }
    pub fn opacity(&self) -> f32 {
        match self { Node::Layer(l) => l.opacity, Node::Folder(f) => f.opacity }
    }
    pub fn opacity_mut(&mut self) -> &mut f32 {
        match self { Node::Layer(l) => &mut l.opacity, Node::Folder(f) => &mut f.opacity }
    }
    pub fn locked(&self) -> bool {
        match self { Node::Layer(l) => l.locked, Node::Folder(f) => f.locked }
    }
    pub fn locked_mut(&mut self) -> &mut bool {
        match self { Node::Layer(l) => &mut l.locked, Node::Folder(f) => &mut f.locked }
    }
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
        let pixels = clear_pixel(background).repeat(width as usize * height as usize);
        Self {
            id: 0,
            name: "Layer".into(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            locked: false,
            pixels,
            background,
        }
    }

    pub fn clear(&mut self) {
        let px = clear_pixel(self.background);
        for p in self.pixels.chunks_exact_mut(4) { p.copy_from_slice(&px); }
    }
}

/// One row of the layer list, top to bottom, as the UI shows it.
pub struct Row {
    pub id:        u64,
    pub depth:     usize,
    pub is_folder: bool,
    pub collapsed: bool,
    /// Locked itself or through a parent folder.
    pub locked_by_parent: bool,
}

/// Side of the squares the stroke snapshot is filled by.
const STROKE_TILE: u32 = 64;

/// Per-stroke buffers of the layer being painted: what the layer was
/// before the stroke, and how much the stroke covers each pixel.
///
/// Allocated zeroed when a stroke begins (the OS hands out zero pages on
/// first touch: nothing is copied or cleared up front) and freed when it
/// ends. The snapshot is filled tile by tile, the first time the brush
/// reaches a tile. Before, every stroke started by copying the whole layer
/// (16 MB at 2048 px, 35 MB on an A4 at 300 dpi) and the buffers stayed
/// allocated all the time.
#[derive(Default)]
struct StrokeBuffers {
    layer: u64,
    base:  Vec<u8>,
    mask:  Vec<u8>,
    /// Tiles whose snapshot is filled, row by row (STROKE_TILE squares).
    ready: Vec<bool>,
}

/// A paint layer seen as a paintable surface for the brush engines.
pub struct LayerSurface<'a> {
    pixels: &'a mut [u8],
    base:   &'a mut [u8],
    mask:   &'a mut [u8],
    ready:  &'a mut [bool],
    width:  u32,
    height: u32,
}

impl PaintSurface for LayerSurface<'_> {
    fn width(&self)  -> u32 { self.width }
    fn height(&self) -> u32 { self.height }
    fn surfaces_mut(&mut self) -> (&mut [u8], &[u8], &mut [u8]) {
        (self.pixels, self.base, self.mask)
    }

    fn prepare(&mut self, rect: Rect) {
        let (w, h) = (self.width, self.height);
        let tiles_x = w.div_ceil(STROKE_TILE);
        let (x0, y0) = (rect.0.min(w) / STROKE_TILE, rect.1.min(h) / STROKE_TILE);
        let (x1, y1) = (rect.2.min(w).div_ceil(STROKE_TILE), rect.3.min(h).div_ceil(STROKE_TILE));
        for ty in y0..y1 {
            for tx in x0..x1 {
                let t = (ty * tiles_x + tx) as usize;
                if self.ready[t] { continue; }
                self.ready[t] = true;
                // Not painted yet in this stroke: the layer is still as it
                // was before it.
                let (px0, px1) = ((tx * STROKE_TILE) as usize * 4, ((tx + 1) * STROKE_TILE).min(w) as usize * 4);
                for y in ty * STROKE_TILE..((ty + 1) * STROKE_TILE).min(h) {
                    let row = y as usize * w as usize * 4;
                    self.base[row + px0..row + px1].copy_from_slice(&self.pixels[row + px0..row + px1]);
                }
            }
        }
    }
}

/// Layer tree + active node. Composite is on the CPU for now.
///
/// Memory: the per-stroke buffers (snapshot of the layer before the
/// stroke + stroke coverage) are only needed by the layer being painted,
/// so they live here once instead of in every layer.
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
            Node::Layer(_) => unreachable!("path goes through a layer"),
        };
    }
    v
}

fn count_layers(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| match n {
        Node::Layer(_) => 1,
        Node::Folder(f) => count_layers(&f.children),
    }).sum()
}

fn first_layer_id(nodes: &[Node]) -> Option<u64> {
    for n in nodes.iter().rev() {
        match n {
            Node::Layer(l) => return Some(l.id),
            Node::Folder(f) => if let Some(id) = first_layer_id(&f.children) { return Some(id); },
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

    /// Rebuild a stack read from a file. Checked, never trusted: every
    /// layer must have `width * height` pixels, ids must be unique, and
    /// there must be at least one paint layer. An unknown active id falls
    /// back to the topmost layer.
    pub fn from_parts(width: u32, height: u32, root: Vec<Node>, active: u64) -> Result<Self, String> {
        fn walk(nodes: &[Node], len: usize, ids: &mut std::collections::HashSet<u64>) -> Result<(), String> {
            for n in nodes {
                if n.id() == 0 || !ids.insert(n.id()) { return Err(format!("duplicate or zero layer id {}", n.id())); }
                match n {
                    Node::Layer(l) if l.pixels.len() != len => {
                        return Err(format!("layer {} has {} bytes, expected {len}", l.id, l.pixels.len()));
                    }
                    Node::Layer(_) => {}
                    Node::Folder(f) => walk(&f.children, len, ids)?,
                }
            }
            Ok(())
        }
        if width == 0 || height == 0 { return Err("empty canvas".into()); }
        let len = width as usize * height as usize * 4;
        let mut ids = std::collections::HashSet::new();
        walk(&root, len, &mut ids)?;
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
        };
        self.insert(Node::Folder(folder))
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
                pixels: l.pixels.clone(),
                background: false,
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
                })
            }
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

    /// Remove the active node (a folder with its content). Refused if it
    /// would leave the document without any paint layer.
    pub fn remove_active(&mut self) -> bool {
        let path = self.path(self.active).unwrap();
        let (last, parent) = path.split_last().unwrap();
        let removed_layers = match &siblings_mut(&mut self.root, parent)[*last] {
            Node::Layer(_) => 1,
            Node::Folder(f) => count_layers(&f.children),
        };
        if self.layer_count() - removed_layers == 0 { return false; }

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

    /// Erase the active layer's content (Delete key). No-op on folders and
    /// locked layers. Returns true if something was cleared.
    pub fn clear_active(&mut self) -> bool {
        if !self.can_paint() { return false; }
        if let Node::Layer(l) = self.active_node_mut() { l.clear(); }
        true
    }

    /// Call when a stroke starts on paint layer `id`: snapshot it and reset
    /// the stroke coverage, so the new stroke never blends with itself but
    /// still paints over earlier strokes. False if `id` is not a paint layer.
    ///
    /// A stroke targets a layer by id, not "the active layer": strokes are
    /// painted on another thread, possibly a little after the pen, and the
    /// user may select or delete another layer meanwhile.
    pub fn begin_stroke(&mut self, id: u64) -> bool {
        // Borrow `root` and `stroke_base` as separate fields.
        if !matches!(node_in(&self.root, id), Some(Node::Layer(_))) { return false; }
        let n = self.width as usize * self.height as usize;
        let tiles = (self.width.div_ceil(STROKE_TILE) * self.height.div_ceil(STROKE_TILE)) as usize;
        self.stroke = StrokeBuffers { layer: id, base: vec![0; n * 4], mask: vec![0; n], ready: vec![false; tiles] };
        true
    }

    /// The stroke is over (recorded for undo, or abandoned): free its
    /// buffers.
    pub fn end_stroke(&mut self) {
        self.stroke = StrokeBuffers::default();
    }

    /// Pixels of `rect` in the stroke's layer as it was when the stroke
    /// began (the undo data of the stroke). RGBA rows, tightly packed.
    pub fn stroke_base_rect(&self, rect: Rect) -> Vec<u8> {
        let s = &self.stroke;
        let Some(Node::Layer(l)) = node_in(&self.root, s.layer) else { return copy_rect(&s.base, self.width, rect) };
        if s.ready.is_empty() { return copy_rect(&l.pixels, self.width, rect); }
        // Tiles the stroke never reached were never copied: the layer
        // there is still as it was.
        let (w, tiles_x) = (self.width as usize, self.width.div_ceil(STROKE_TILE) as usize);
        let mut out = Vec::with_capacity((rect.2 - rect.0) as usize * (rect.3 - rect.1) as usize * 4);
        for y in rect.1..rect.3 {
            let mut x = rect.0;
            while x < rect.2 {
                let tx = x / STROKE_TILE;
                let end = ((tx + 1) * STROKE_TILE).min(rect.2);
                let t = (y / STROKE_TILE) as usize * tiles_x + tx as usize;
                let src = if s.ready[t] { &s.base } else { &l.pixels };
                let row = y as usize * w;
                out.extend_from_slice(&src[(row + x as usize) * 4..(row + end as usize) * 4]);
                x = end;
            }
        }
        out
    }

    /// Pixels of `rect` in paint layer `id`, RGBA rows, tightly packed.
    pub fn layer_rect(&self, id: u64, rect: Rect) -> Option<Vec<u8>> {
        match self.node(id)? {
            Node::Layer(l) => Some(copy_rect(&l.pixels, self.width, rect)),
            Node::Folder(_) => None,
        }
    }

    /// Write `data` (from layer_rect / stroke_base_rect) back into `rect`
    /// of paint layer `id`. False if the layer is gone or the size is off.
    pub fn set_layer_rect(&mut self, id: u64, rect: Rect, data: &[u8]) -> bool {
        let w = self.width;
        let (rw, rh) = ((rect.2 - rect.0) as usize, (rect.3 - rect.1) as usize);
        if rect.2 > self.width || rect.3 > self.height || data.len() != rw * rh * 4 { return false; }
        let Some(Node::Layer(l)) = self.node_mut(id) else { return false };
        for (row, src) in data.chunks_exact(rw * 4).enumerate() {
            let start = ((rect.1 as usize + row) * w as usize + rect.0 as usize) * 4;
            l.pixels[start..start + rw * 4].copy_from_slice(src);
        }
        true
    }

    /// Paint layer `id` as a brush surface, with the stroke buffers of
    /// begin_stroke; None if it is gone, a folder, or not the layer the
    /// stroke began on.
    pub fn layer_surface(&mut self, id: u64) -> Option<LayerSurface<'_>> {
        let (w, h) = (self.width, self.height);
        let path = self.path(id)?;
        let (last, parent) = path.split_last()?;
        let Node::Layer(l) = &mut siblings_mut(&mut self.root, parent)[*last] else { return None };
        // Only the layer the stroke began on has stroke buffers.
        if self.stroke.layer != id || self.stroke.ready.is_empty() { return None; }
        Some(LayerSurface {
            pixels: &mut l.pixels,
            base:   &mut self.stroke.base,
            mask:   &mut self.stroke.mask,
            ready:  &mut self.stroke.ready,
            width:  w,
            height: h,
        })
    }

    /// Ids of every paint layer (thumbnails).
    pub fn layer_ids(&self) -> Vec<u64> {
        fn walk(nodes: &[Node], out: &mut Vec<u64>) {
            for n in nodes {
                match n {
                    Node::Layer(l) => out.push(l.id),
                    Node::Folder(f) => walk(&f.children, out),
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, &mut v);
        v
    }

    /// Rows of the layer list, top to bottom, skipping collapsed content.
    pub fn rows(&self) -> Vec<Row> {
        fn walk(nodes: &[Node], depth: usize, locked: bool, out: &mut Vec<Row>) {
            for n in nodes.iter().rev() {
                let (is_folder, collapsed) = match n {
                    Node::Folder(f) => (true, f.collapsed),
                    Node::Layer(_) => (false, false),
                };
                out.push(Row { id: n.id(), depth, is_folder, collapsed, locked_by_parent: locked });
                if let Node::Folder(f) = n {
                    if !f.collapsed {
                        walk(&f.children, depth + 1, locked || f.locked, out);
                    }
                }
            }
        }
        let mut v = Vec::new();
        walk(&self.root, 0, false, &mut v);
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
                }
            }
        }
        let (ow, oh) = (self.width as i64, self.height as i64);
        let (nw, nh) = (width as i64, height as i64);
        // Position of the old canvas's top-left corner in the new one.
        let (dx, dy) = ((nw - ow) / 2, (nh - oh) / 2);
        // Overlap, in old-canvas coordinates.
        let sx0 = (-dx).max(0);
        let sy0 = (-dy).max(0);
        let sx1 = (nw - dx).min(ow);
        let sy1 = (nh - dy).min(oh);

        walk(&mut self.root, &mut |layer: &mut Layer| {
            let mut out = clear_pixel(layer.background).repeat(width as usize * height as usize);
            if sx0 < sx1 {
                for sy in sy0..sy1 {
                    let src = ((sy * ow + sx0) * 4) as usize..((sy * ow + sx1) * 4) as usize;
                    let ny = sy + dy;
                    let dst_start = ((ny * nw + sx0 + dx) * 4) as usize;
                    out[dst_start..dst_start + src.len()].copy_from_slice(&layer.pixels[src]);
                }
            }
            layer.pixels = out;
        });

        self.width  = width;
        self.height = height;
        // A stroke in progress can't go on at the new size.
        self.end_stroke();
    }

    /// Composite every visible layer into `out` (`width * height * 4`).
    pub fn composite_into(&self, out: &mut [u8]) {
        self.composite_rect_into(out, (0, 0, self.width, self.height));
    }

    /// Composite limited to `rect`, row by row, straight into `out`
    /// (indexed as the full canvas).
    ///
    /// The result is always opaque (white under everything), so the main
    /// path blends straight into the output bytes: Normal layers in integer
    /// math, other modes simplified for an opaque backdrop. No temporary
    /// row, no un-premultiply per pixel. Only isolated folders go through
    /// the general premultiplied path. (Was ~27 ns/px, see
    /// examples/bench_composite.rs.)
    pub fn composite_rect_into(&self, out: &mut [u8], rect: Rect) {
        debug_assert_eq!(out.len(), self.width as usize * self.height as usize * 4);
        let (x0, y0) = (rect.0, rect.1);
        let x1 = rect.2.min(self.width);
        let y1 = rect.3.min(self.height);
        if x0 >= x1 || y0 >= y1 { return; }
        let n = (x1 - x0) as usize;
        let mut group: Vec<Premul> = Vec::new();

        for y in y0..y1 {
            let start = (y as usize * self.width as usize + x0 as usize) * 4;
            let dst = &mut out[start..start + n * 4];
            // White behind everything.
            dst.fill(255);
            composite_row_opaque(&self.root, self.width, y, x0, dst, 1.0, &mut group);
        }
    }
}

/// Blend `nodes` (bottom to top) onto an opaque RGBA row `dst`, for canvas
/// row `y` from column `x0`. `opacity_scale` carries pass-through folder
/// opacity down to the children.
fn composite_row_opaque(
    nodes: &[Node], width: u32, y: u32, x0: u32, dst: &mut [u8], opacity_scale: f32, group: &mut Vec<Premul>,
) {
    let n = dst.len() / 4;
    for node in nodes {
        if !node.visible() || node.opacity() <= 0.0 { continue; }
        match node {
            Node::Layer(l) => {
                let start = (y as usize * width as usize + x0 as usize) * 4;
                let src = &l.pixels[start..start + n * 4];
                blend_layer_opaque(dst, src, l.opacity * opacity_scale, l.blend);
            }
            Node::Folder(f) => match f.blend {
                FolderBlend::PassThrough => {
                    composite_row_opaque(&f.children, width, y, x0, dst, opacity_scale * f.opacity, group);
                }
                FolderBlend::Isolated(mode) => {
                    // Children merged on their own (transparent start, so
                    // the general premultiplied path), then blended down.
                    group.clear();
                    group.resize(n, [0.0; 4]);
                    composite_row(&f.children, width, y, x0, group, 1.0);
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
        // dst += (src - dst) * a, with a = alpha * opacity in 0..=255*255.
        const FULL: u32 = 255 * 255;
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = s[3] as u32 * op8;
            if a == 0 { continue; }
            if a == FULL {
                d[..3].copy_from_slice(&s[..3]);
                continue;
            }
            let ia = FULL - a;
            for c in 0..3 {
                d[c] = ((d[c] as u32 * ia + s[c] as u32 * a + FULL / 2) / FULL) as u8;
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
fn composite_row(nodes: &[Node], width: u32, y: u32, x0: u32, row: &mut [Premul], opacity_scale: f32) {
    for node in nodes {
        if !node.visible() || node.opacity() <= 0.0 { continue; }
        match node {
            Node::Layer(l) => {
                let op = l.opacity * opacity_scale;
                let start = (y as usize * width as usize + x0 as usize) * 4;
                let src = &l.pixels[start..start + row.len() * 4];
                for (dst, s) in row.iter_mut().zip(src.chunks_exact(4)) {
                    if s[3] == 0 { continue; }
                    let a = s[3] as f32 / 255.0 * op;
                    let cs = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
                    blend_over(dst, cs, a, l.blend);
                }
            }
            Node::Folder(f) => match f.blend {
                FolderBlend::PassThrough => {
                    composite_row(&f.children, width, y, x0, row, opacity_scale * f.opacity);
                }
                FolderBlend::Isolated(mode) => {
                    let mut group: Vec<Premul> = vec![[0.0; 4]; row.len()];
                    composite_row(&f.children, width, y, x0, &mut group, 1.0);
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

/// Copy `rect` out of a `width`-wide RGBA buffer, rows tightly packed.
fn copy_rect(src: &[u8], width: u32, rect: Rect) -> Vec<u8> {
    let rw = (rect.2 - rect.0) as usize * 4;
    let mut out = Vec::with_capacity(rw * (rect.3 - rect.1) as usize);
    for y in rect.1..rect.3 {
        let start = (y as usize * width as usize + rect.0 as usize) * 4;
        out.extend_from_slice(&src[start..start + rw]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer_px(stack: &LayerStack, id: u64, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * stack.width + x) * 4) as usize;
        match stack.node(id).unwrap() {
            Node::Layer(l) => l.pixels[i..i + 4].try_into().unwrap(),
            _ => panic!("not a layer"),
        }
    }

    fn set_px(stack: &mut LayerStack, id: u64, rgba: [u8; 4]) {
        if let Some(Node::Layer(l)) = stack.node_mut(id) {
            for p in l.pixels.chunks_exact_mut(4) { p.copy_from_slice(&rgba); }
        }
    }

    fn composite(stack: &LayerStack) -> [u8; 4] {
        let mut out = vec![0u8; (stack.width * stack.height * 4) as usize];
        stack.composite_into(&mut out);
        out[0..4].try_into().unwrap()
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
        if let Some(Node::Layer(layer)) = s.node_mut(l) {
            let i = (4 + 1) * 4;
            layer.pixels[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
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
                let (px, _, _) = surf.surfaces_mut();
                for y in r.1..r.3 {
                    for x in r.0..r.2 {
                        let i = ((y * 256 + x) * 4) as usize;
                        px[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
                    }
                }
            }
        }
        let before = s.stroke_base_rect((0, 0, 200, 200));
        assert!(before.chunks_exact(4).all(|p| p == [255, 255, 255, 0]), "undo data is not the original layer");
        s.end_stroke();
        assert!(s.layer_surface(id).is_none(), "buffers kept after the stroke");
    }
}
