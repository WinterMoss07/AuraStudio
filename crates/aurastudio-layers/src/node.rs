//! The nodes of the tree and what every node has.

use aurastudio_core::{BlendMode, HeapSize, Rect, Rgba8};
use aurastudio_raster::{Image, Mask};

use crate::anim::FolderKind;

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

/// What a cleared layer holds: white for the background, else
/// transparent (white RGB: a bug ignoring alpha shows white).
pub(crate) fn clear_pixel(background: bool) -> Rgba8 { if background { [255; 4] } else { [255, 255, 255, 0] } }

/// One paint layer: straight RGBA on an unbounded tiled surface.
#[derive(Clone, Debug)]
pub struct Layer {
    pub id:         u64,
    pub name:       String,
    pub visible:    bool,
    /// Applied when merging, never in the pixels.
    pub opacity:    f32,
    pub blend:      BlendMode,
    /// A locked layer can't be painted on or cleared.
    pub locked:     bool,
    pub surface:    Image,
    /// The Paper: opaque white everywhere; clearing it resets it to white.
    pub background: bool,
    pub props:      Props,
}

impl Layer {
    /// An empty layer (white when `background`) of a `width` × `height`
    /// frame with tiles of `tile`.
    pub fn new(width: u32, height: u32, tile: u32, background: bool) -> Self {
        Self {
            id: 0, name: "Layer".into(), visible: true, opacity: 1.0, blend: BlendMode::Normal, locked: false,
            surface: Image::new(width, height, tile, clear_pixel(background)), background, props: Props::default(),
        }
    }

    /// Back to nothing: transparent, or white for the background.
    pub fn clear(&mut self) {
        self.surface.clear();
        self.surface.set_fill(clear_pixel(self.background));
    }
}

/// A filter layer (Krita): its filter over what is under it (in an
/// isolated folder: the folder's content; clipped: its base).
#[derive(Clone, Debug)]
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

/// A saved selection (Krita's selection mask): a coverage kept in the
/// tree, never drawn; loaded back as the selection.
#[derive(Clone, Debug)]
pub struct MaskLayer {
    pub id:      u64,
    pub name:    String,
    pub visible: bool,
    pub locked:  bool,
    pub props:   Props,
    pub mask:    Mask,
}

/// How a folder combines its content with what is below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FolderBlend {
    /// Children blend onto the layers below as if the folder weren't there
    /// (Krita / CSP default).
    #[default]
    PassThrough,
    /// Children are first merged together on their own, then the result
    /// is blended onto the layers below with this mode.
    Isolated(BlendMode),
}

#[derive(Clone, Debug)]
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
    /// Plain, animation folder, one of its sheets, or a group.
    pub kind:      FolderKind,
}

impl Folder {
    /// An empty plain folder (folded: a long list stays readable).
    pub fn new(name: &str) -> Self {
        Folder { id: 0, name: name.into(), visible: true, opacity: 1.0, blend: FolderBlend::PassThrough, locked: false, collapsed: true, children: Vec::new(), props: Props::default(), kind: FolderKind::Plain }
    }
}

#[derive(Clone, Debug)]
pub enum Node {
    Layer(Layer),
    Folder(Folder),
    Filter(FilterLayer),
    Mask(MaskLayer),
}

/// The part of the canvas a node can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Footprint {
    /// No pixel (an empty layer or folder, a mask).
    Nothing,
    Area(Rect),
    /// Everywhere (a layer with a visible fill, a filter layer).
    All,
}

impl Footprint {
    pub fn union(self, o: Footprint) -> Footprint {
        match (self, o) {
            (Footprint::All, _) | (_, Footprint::All) => Footprint::All,
            (Footprint::Nothing, x) | (x, Footprint::Nothing) => x,
            (Footprint::Area(a), Footprint::Area(b)) => Footprint::Area(a.union(&b)),
        }
    }
}

/// Which nodes a merge takes (the merge itself is `compose`'s).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    /// Leave out the draft layers (and what draft folders hold).
    pub skip_drafts: bool,
    /// Only this node (a reference layer or folder, and what is clipped
    /// to it); `Some(0)`: nothing (no reference to look at).
    pub reference:   Option<u64>,
    /// Leave out this node (0: none): the layer being painted, which
    /// mustn't stop its own strokes (CSP's "exclude editing layer").
    pub skip:        u64,
    /// Stop at this node (0: never): the merge of what is under it (a
    /// filter layer's dialog shows what it filters).
    pub until:       u64,
}

impl Pick {
    pub const ALL: Pick = Pick { skip_drafts: false, reference: None, skip: 0, until: 0 };
    /// What an exported image shows (CSP leaves drafts out).
    pub const EXPORT: Pick = Pick { skip_drafts: true, ..Pick::ALL };
}

/// One sheet of an animation folder shown as an onion-skin ghost: which,
/// its tint and opacity. `animation` decides them; `compose` draws them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ghost {
    pub sheet:   u64,
    pub tint:    [f32; 3],
    pub opacity: f32,
}

impl Ghost {
    /// `cs` tinted for this ghost (straight color, 0..1): OpenToonz's
    /// raster colorScale, `1 − (1 − c)(1 − tint)`: black ink takes the
    /// tint exactly, white stays white.
    #[inline]
    pub fn tinted(&self, cs: [f32; 3]) -> [f32; 3] { std::array::from_fn(|k| 1.0 - (1.0 - cs[k]) * (1.0 - self.tint[k])) }
}

/// `$e` on the node's struct, whatever its kind.
macro_rules! each {
    ($n:expr, $x:ident => $e:expr) => {
        match $n { Node::Layer($x) => $e, Node::Folder($x) => $e, Node::Filter($x) => $e, Node::Mask($x) => $e }
    };
}

impl Node {
    pub fn id(&self) -> u64 { each!(self, n => n.id) }
    pub(crate) fn set_id(&mut self, id: u64) { each!(self, n => n.id = id) }
    pub fn name(&self) -> &str { each!(self, n => &n.name) }
    pub fn name_mut(&mut self) -> &mut String { each!(self, n => &mut n.name) }
    pub fn visible(&self) -> bool { each!(self, n => n.visible) }
    pub fn visible_mut(&mut self) -> &mut bool { each!(self, n => &mut n.visible) }
    /// (A mask has no opacity: 1.)
    pub fn opacity(&self) -> f32 { match self { Node::Layer(l) => l.opacity, Node::Folder(f) => f.opacity, Node::Filter(f) => f.opacity, Node::Mask(_) => 1.0 } }
    pub fn set_opacity(&mut self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        match self { Node::Layer(l) => l.opacity = v, Node::Folder(f) => f.opacity = v, Node::Filter(f) => f.opacity = v, Node::Mask(_) => {} }
    }
    pub fn locked(&self) -> bool { each!(self, n => n.locked) }
    pub fn locked_mut(&mut self) -> &mut bool { each!(self, n => &mut n.locked) }
    pub fn props(&self) -> Props { each!(self, n => n.props) }
    pub fn props_mut(&mut self) -> &mut Props { each!(self, n => &mut n.props) }
    pub fn is_folder(&self) -> bool { matches!(self, Node::Folder(_)) }
    pub fn as_layer(&self) -> Option<&Layer> { match self { Node::Layer(l) => Some(l), _ => None } }
    pub fn as_folder(&self) -> Option<&Folder> { match self { Node::Folder(f) => Some(f), _ => None } }

    /// Short blend label for the UI ("Normal", "Pass Through"...).
    pub fn blend_name(&self) -> &'static str {
        match self {
            Node::Layer(l) => l.blend.name(),
            Node::Folder(f) => match f.blend { FolderBlend::PassThrough => "Pass Through", FolderBlend::Isolated(m) => m.name() },
            Node::Filter(f) => f.filter.kind.name(),
            Node::Mask(_) => "Mask",
        }
    }

    /// Ids of the paint layers in this node (itself, or all in a folder),
    /// bottom to top, appended to `out`.
    pub fn push_layer_ids(&self, out: &mut Vec<u64>) {
        match self {
            Node::Layer(l) => out.push(l.id),
            Node::Folder(f) => for c in &f.children { c.push_layer_ids(out) },
            Node::Filter(_) | Node::Mask(_) => {}
        }
    }

    /// The part of the canvas this node can change (every blend mode
    /// leaves the image below as it is where a layer is transparent).
    pub fn footprint(&self) -> Footprint {
        match self {
            Node::Layer(l) if l.surface.fill()[3] > 0 => Footprint::All,
            Node::Layer(l) => l.surface.visible_bounds().map_or(Footprint::Nothing, Footprint::Area),
            Node::Folder(f) => f.children.iter().fold(Footprint::Nothing, |acc, c| acc.union(c.footprint())),
            Node::Filter(_) => Footprint::All,
            Node::Mask(_) => Footprint::Nothing,
        }
    }

    /// Its paint layers and what is inside, depth first (folders before
    /// their children), bottom to top.
    pub fn for_each(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        if let Node::Folder(fo) = self { for c in &fo.children { c.for_each(f); } }
    }

    pub fn for_each_mut(&mut self, f: &mut impl FnMut(&mut Node)) {
        f(self);
        if let Node::Folder(fo) = self { for c in &mut fo.children { c.for_each_mut(f); } }
    }
}

impl HeapSize for Node {
    /// Pixels it holds (with what is inside a folder).
    fn heap_bytes(&self) -> usize {
        match self {
            Node::Layer(l) => l.surface.heap_bytes(),
            Node::Folder(f) => f.children.iter().map(HeapSize::heap_bytes).sum(),
            Node::Filter(_) => 0,
            Node::Mask(m) => m.mask.heap_bytes(),
        }
    }
}
