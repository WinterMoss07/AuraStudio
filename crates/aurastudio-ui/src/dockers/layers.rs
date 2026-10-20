//! Layers docker (CSP / Krita): label, blend mode, opacity; one row of
//! actions; the tree (drag and drop, Ctrl / Shift selection, right-click
//! menu, chains across clipped rows). Its edits become undo steps.

use std::collections::{HashMap, HashSet};

use aurastudio_filters::{Filter, GROUPS};
use aurastudio_layers::{FolderBlend, LayerStack, Node, Place, Row, LABELS};
use egui::{pos2, vec2, Align, Color32, Frame, Layout, Rect, RichText, Sense, Stroke, Ui};

use crate::widgets::*;
use crate::{theme, Actions, BlendMode, Model, Notice};

const THUMB: f32 = 28.0;
const ROW_H: f32 = 34.0;
const INDENT: f32 = 14.0;
/// Clipped rows: the thumbnail moves right (the chain shows what to).
const CLIP_INDENT: f32 = 10.0;
/// The eye's center, from the row's left (the chains line up with it).
const EYE_X: f32 = 17.0;
/// CSP's draft blue.
const DRAFT_BLUE: Color32 = Color32::from_rgb(86, 156, 255);

#[derive(Default)]
pub struct LayersState {
    thumbs:   HashMap<u64, egui::TextureHandle>,
    stale:    HashSet<u64>,
    /// Layer being renamed: (id, text, focus not requested yet).
    renaming: Option<(u64, String, bool)>,
    /// Labels shown (bit i: label i); 0: everything.
    label_filter: u16,
    /// A layer command from a shortcut, done at the next frame.
    pending:  Option<LayerCmd>,
    /// A filter layer's dialog to open (the docker can't reach it).
    pub(crate) edit_filter: Option<(u64, bool)>,
    /// The layers picked with Ctrl / Shift, the active one with them
    /// (empty: only the active one).
    selected: Vec<u64>,
}

impl LayersState {
    pub fn invalidate(&mut self, id: u64) {
        self.stale.insert(id);
    }

    pub fn invalidate_all(&mut self) {
        self.thumbs.clear();
    }

    pub fn shortcut(&mut self, cmd: LayerCmd) { self.pending = Some(cmd); }

    /// What the layer actions apply to: the selected nodes (the active one
    /// among them), or the active one alone.
    pub fn selection(&self, layers: &LayerStack) -> Vec<u64> {
        let active = layers.active_id();
        let mut v: Vec<u64> = self.selected.iter().copied().filter(|id| layers.node(*id).is_some()).collect();
        if !v.contains(&active) { v.push(active); }
        v
    }

    /// Keep one small texture per paint layer, rebuilt when its pixels changed.
    pub fn refresh_thumbnails(&mut self, ctx: &egui::Context, layers: &LayerStack) {
        let ids = layers.layer_ids();
        let alive: HashSet<u64> = ids.iter().copied().collect();
        self.thumbs.retain(|id, _| alive.contains(id));

        for id in ids {
            let stale = self.stale.remove(&id);
            if self.thumbs.contains_key(&id) && !stale { continue; }
            let Some(Node::Layer(layer)) = layers.node(id) else { continue };

            let (w, h) = (layers.width as usize, layers.height as usize);
            let k = (64.0 / w.max(h) as f32).min(1.0);
            let tw = ((w as f32 * k).round() as usize).max(1);
            let th = ((h as f32 * k).round() as usize).max(1);
            let mut rgba = Vec::with_capacity(tw * th * 4);
            for y in 0..th {
                let sy = (y * h) / th;
                for x in 0..tw {
                    let sx = (x * w) / tw;
                    rgba.extend_from_slice(&layer.tiles.pixel(sx as u32, sy as u32));
                }
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([tw, th], &rgba);
            match self.thumbs.get_mut(&id) {
                Some(tex) => tex.set(image, egui::TextureOptions::LINEAR),
                None => {
                    let tex = ctx.load_texture(format!("layer-thumb-{id}"), image, egui::TextureOptions::LINEAR);
                    self.thumbs.insert(id, tex);
                }
            }
        }
    }
}

// --- Actions (shared with the Layer menu and the shortcuts) -------------------

pub fn new_layer(m: &mut Model, a: &mut Actions) {
    let n = m.layers.layer_count();
    m.layers.add_layer(&format!("Layer {n}"));
    a.recomposite = true;
    a.layer_edit = Some("New layer");
}

pub fn new_folder(m: &mut Model, a: &mut Actions) {
    m.layers.add_folder("Folder");
    a.recomposite = true;
    a.layer_edit = Some("New folder");
}

/// A new animation folder (Animation documents), the Cel template docker
/// shown.
pub fn new_animation_folder(m: &mut Model, a: &mut Actions) {
    if m.doc_kind != aurastudio_document::Kind::Animation { return; }
    let n = m.layers.rows().iter().filter(|r| r.animation).count() + 1;
    m.layers.add_animation_folder(&format!("Animation {n}"));
    a.recomposite = true;
    a.layer_edit = Some("New animation folder");
    a.show_docker = Some(super::Tab::CelTemplate);
}

/// The special folders (animation, camera, comics), each in its own kind
/// of document; grayed out elsewhere, saying why.
pub fn special_folder_entries(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    use aurastudio_document::Kind;
    let anim = m.doc_kind == Kind::Animation;
    let b = ui.add_enabled(anim, egui::Button::new("Animation folder")).on_disabled_hover_text("Animation documents only");
    if b.clicked() { new_animation_folder(m, a); ui.close_menu(); }
    ui.add_enabled(false, egui::Button::new("Camera folder"))
        .on_disabled_hover_text(if anim { "Coming with the camera" } else { "Animation documents only" });
    ui.add_enabled(false, egui::Button::new("Comics folder"))
        .on_disabled_hover_text(if m.doc_kind == Kind::Comics { "Coming with the comics tools" } else { "Comics documents only" });
}

/// A new filter layer above the active node, its dialog opened (Cancel
/// there removes it again).
pub fn new_filter_layer(st: &mut LayersState, kind: aurastudio_filters::Kind, m: &mut Model, a: &mut Actions) {
    let color = match kind { aurastudio_filters::Kind::BlackWhite => Filter::new(kind).color, _ => m.brush.color };
    let id = m.layers.add_filter_layer(Filter { color, ..Filter::new(kind) });
    st.selected.clear();
    st.edit_filter = Some((id, true));
    a.recomposite = true;
    a.layer_edit = Some("New filter layer");
}

/// Delete the selected nodes (those that can be: not locked, not the last
/// paint layer).
pub fn delete_layer(st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    let mut refused = None;
    for id in st.selection(m.layers) {
        // (Gone with a folder deleted before it.)
        if m.layers.node(id).is_none() { continue; }
        m.layers.set_active(id);
        match m.layers.removal_refused() {
            Some(why) => refused = Some(why),
            None => {
                m.layers.remove_active();
                a.recomposite = true;
                a.layer_edit = Some("Delete layer");
            }
        }
    }
    st.selected.clear();
    if let Some(why) = refused { notice.set(why); }
}

/// Duplicate the selected nodes, each above itself.
pub fn duplicate(st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    let mut copies = Vec::new();
    for id in st.selection(m.layers) {
        m.layers.set_active(id);
        if m.layers.active_id() == id { copies.push(m.layers.duplicate_active()); }
    }
    st.selected = if copies.len() > 1 { copies } else { Vec::new() };
    a.recomposite = true;
    a.layer_edit = Some("Duplicate");
}

/// Layer commands with a shortcut (and a menu entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerCmd {
    /// Put the selected nodes in a new folder (Ctrl+G).
    Group,
    /// Take a folder's content out (Ctrl+Shift+G).
    Ungroup,
    /// Clip to the node below, or not (Ctrl+Alt+G, as in Photoshop).
    Clip,
    Reference,
    Draft,
}

impl LayerCmd {
    pub fn name(self) -> &'static str {
        match self {
            LayerCmd::Group => "Group",
            LayerCmd::Ungroup => "Ungroup",
            LayerCmd::Clip => "Clip to layer below",
            LayerCmd::Reference => "Reference layer",
            LayerCmd::Draft => "Draft layer",
        }
    }
    /// Its name, and its shortcut in parentheses.
    pub fn tip(self) -> String {
        match self.shortcut() {
            "" => self.name().to_owned(),
            k => format!("{} ({k})", self.name()),
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            LayerCmd::Group => "Ctrl+G",
            LayerCmd::Ungroup => "Ctrl+Shift+G",
            LayerCmd::Clip => "Ctrl+Alt+G",
            LayerCmd::Reference | LayerCmd::Draft => "",
        }
    }
    /// The active node has it on (for a check mark).
    pub fn is_on(self, layers: &LayerStack) -> bool {
        let p = layers.active_node().props();
        match self {
            LayerCmd::Clip => p.clip,
            LayerCmd::Reference => p.reference,
            LayerCmd::Draft => p.draft,
            _ => false,
        }
    }
    pub fn enabled(self, layers: &LayerStack) -> bool {
        match self {
            LayerCmd::Ungroup => layers.active_node().is_folder(),
            _ => true,
        }
    }
}

/// The folder button's command: ungroup a folder selected alone, else
/// group what is selected.
fn group_or_ungroup(st: &LayersState, layers: &LayerStack) -> LayerCmd {
    let sel = st.selection(layers);
    if sel.len() == 1 && layers.active_node().is_folder() { LayerCmd::Ungroup } else { LayerCmd::Group }
}

pub fn apply(cmd: LayerCmd, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    let sel = st.selection(m.layers);
    let active = m.layers.active_id();
    match cmd {
        LayerCmd::Group => {
            if sel.len() > 1 { m.layers.group_nodes(&sel, "Folder"); } else { m.layers.group_active("Folder"); }
            st.selected.clear();
        }
        LayerCmd::Ungroup => {
            if !m.layers.ungroup_active() { return; }
            st.selected.clear();
        }
        // (The selected nodes follow the active one.)
        LayerCmd::Clip => {
            let on = !m.layers.active_node().props().clip;
            for id in &sel { if let Some(n) = m.layers.node_mut(*id) { n.props_mut().clip = on; } }
        }
        LayerCmd::Draft => {
            let on = !m.layers.active_node().props().draft;
            for id in &sel { if let Some(n) = m.layers.node_mut(*id) { n.props_mut().draft = on; } }
        }
        // One per folder: the active node only.
        LayerCmd::Reference => {
            let on = !m.layers.active_node().props().reference;
            m.layers.set_reference(active, on);
        }
    }
    a.layer_edit = Some(cmd.name());
    // (Reference and draft change no pixel, only what tools look at.)
    if matches!(cmd, LayerCmd::Reference | LayerCmd::Draft) { a.modified = true; } else { a.recomposite = true; }
}

/// Give the selected nodes color label `l`.
fn set_label(st: &LayersState, l: u8, m: &mut Model, a: &mut Actions) {
    for id in st.selection(m.layers) {
        if let Some(n) = m.layers.node_mut(id) { n.props_mut().label = l; }
    }
    a.modified = true;
    a.layer_edit = Some("Color label");
}

/// The Layer menu's entries for the marks and folders.
pub fn menu_entries(ui: &mut Ui, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    ui.menu_button("New filter layer", |ui| filter_layer_menu(ui, st, m, a));
    ui.menu_button("New special folder", |ui| special_folder_entries(ui, m, a));
    ui.separator();
    for cmd in [LayerCmd::Group, LayerCmd::Ungroup] {
        if ui.add_enabled(cmd.enabled(m.layers), egui::Button::new(cmd.name()).shortcut_text(cmd.shortcut())).clicked() {
            apply(cmd, st, m, a);
            ui.close_menu();
        }
    }
    ui.separator();
    for cmd in [LayerCmd::Clip, LayerCmd::Reference, LayerCmd::Draft] {
        let label = format!("{} {}", if cmd.is_on(m.layers) { "✔" } else { "   " }, cmd.name());
        if ui.add(egui::Button::new(label).shortcut_text(cmd.shortcut())).clicked() {
            apply(cmd, st, m, a);
            ui.close_menu();
        }
    }
    ui.menu_button("Color label", |ui| {
        if let Some(l) = label_list(ui, m.layers.active_node().props().label) {
            set_label(st, l, m, a);
            ui.close_menu();
        }
    });
}

/// The filters, by group, as new filter layers.
fn filter_layer_menu(ui: &mut Ui, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    for (k, (group, kinds)) in GROUPS.iter().enumerate() {
        if k > 0 { ui.separator(); }
        ui.label(RichText::new(*group).small().color(theme::TEXT_WEAK));
        for &kind in *kinds {
            if ui.button(kind.name()).clicked() {
                new_filter_layer(st, kind, m, a);
                ui.close_menu();
            }
        }
    }
}

/// The label colors as round swatches in a row (the current one ringed);
/// the one clicked, if any.
fn label_swatches(ui: &mut Ui, current: u8) -> Option<u8> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (i, (name, _)) in LABELS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            let resp = resp.on_hover_text(*name);
            swatch(ui.painter(), r.center(), i as u8, 7.0);
            if current as usize == i { ui.painter().circle_stroke(r.center(), 9.5, Stroke::new(1.5_f32, theme::TEXT)); }
            else if resp.hovered() { ui.painter().circle_stroke(r.center(), 9.5, Stroke::new(1.0_f32, theme::TEXT_WEAK)); }
            if resp.clicked() { picked = Some(i as u8); }
        }
    });
    picked
}

/// The label colors listed top to bottom, with their names (the docker's
/// label button, the Layer menu).
fn label_list(ui: &mut Ui, current: u8) -> Option<u8> {
    let mut picked = None;
    ui.set_min_width(120.0);
    for (i, (name, _)) in LABELS.iter().enumerate() {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(120.0), 22.0), Sense::click());
        if resp.hovered() || current as usize == i {
            ui.painter().rect_filled(r, 2.0, if current as usize == i { theme::ACCENT_DIM } else { theme::WIDGET_HOVER });
        }
        swatch(ui.painter(), pos2(r.left() + 13.0, r.center().y), i as u8, 6.0);
        ui.painter().text(pos2(r.left() + 26.0, r.center().y), egui::Align2::LEFT_CENTER, *name, egui::FontId::proportional(13.0), theme::TEXT);
        if resp.clicked() { picked = Some(i as u8); }
    }
    picked
}

/// Label `i`'s dot (an empty circle crossed out for "none").
fn swatch(p: &egui::Painter, c: egui::Pos2, i: u8, r: f32) {
    match label_color(i) {
        Some(col) => { p.circle_filled(c, r, col); }
        None => {
            p.circle_stroke(c, r, Stroke::new(1.2_f32, theme::TEXT_WEAK));
            p.line_segment([c + vec2(-r * 0.7, r * 0.7), c + vec2(r * 0.7, -r * 0.7)], Stroke::new(1.2_f32, theme::TEXT_WEAK));
        }
    }
}

fn label_color(label: u8) -> Option<Color32> {
    (label != 0).then(|| LABELS.get(label as usize)).flatten().map(|(_, c)| Color32::from_rgb(c[0], c[1], c[2]))
}

/// A layer row being dragged (egui drag and drop payload).
struct LayerDrag(u64);

/// Clipped layers' chain (sky blue).
const CHAIN: Color32 = Color32::from_rgb(110, 200, 255);

/// Two links of a chain, upright around `c`: half on the clipped row,
/// half on the one under it.
fn chain(p: &egui::Painter, c: egui::Pos2) {
    p.rect_filled(Rect::from_center_size(c, vec2(11.0, 21.0)), 2.5, Color32::from_black_alpha(150));
    let s = Stroke::new(1.7_f32, CHAIN);
    for dy in [-4.5, 4.5] {
        p.rect_stroke(Rect::from_center_size(c + vec2(0.0, dy), vec2(6.5, 10.5)), 2.0, s);
    }
}

/// New layer (its kinds under the arrow), new folder, duplicate.
fn make_buttons(ui: &mut Ui, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    if icon_button(ui, Icon::NewLayer, "New layer", true) { new_layer(m, a); }
    let arrow = egui::Button::image(egui::Image::new(Icon::Expanded.source()).fit_to_exact_size(vec2(12.0, 12.0)).tint(Color32::from_gray(0xc8)))
        .frame(false)
        .min_size(vec2(14.0, 24.0));
    egui::menu::menu_custom_button(ui, arrow, |ui| {
        ui.set_min_width(150.0);
        if ui.button("Paint layer").clicked() { new_layer(m, a); ui.close_menu(); }
        ui.menu_button("Filter layer", |ui| filter_layer_menu(ui, st, m, a));
    }).response.on_hover_text("Layer types");
    if icon_button(ui, Icon::NewFolder, "New folder", true) { new_folder(m, a); }
    let arrow = egui::Button::image(egui::Image::new(Icon::Expanded.source()).fit_to_exact_size(vec2(12.0, 12.0)).tint(Color32::from_gray(0xc8)))
        .frame(false)
        .min_size(vec2(14.0, 24.0));
    egui::menu::menu_custom_button(ui, arrow, |ui| {
        ui.set_min_width(150.0);
        special_folder_entries(ui, m, a);
    }).response.on_hover_text("Special folders");
    if icon_button(ui, Icon::Duplicate, "Duplicate", true) { duplicate(st, m, a); }
}

/// Clip, reference, draft, then group / ungroup.
fn mark_buttons(ui: &mut Ui, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    for (cmd, icon) in [(LayerCmd::Clip, Icon::Clip), (LayerCmd::Reference, Icon::Reference), (LayerCmd::Draft, Icon::Draft)] {
        if icon_toggle(ui, icon, &cmd.tip(), true, cmd.is_on(m.layers)).clicked() { apply(cmd, st, m, a); }
    }
    let g = group_or_ungroup(st, m.layers);
    let icon = if g == LayerCmd::Ungroup { Icon::Ungroup } else { Icon::Group };
    if icon_toggle(ui, icon, &g.tip(), true, false).clicked() { apply(g, st, m, a); }
}

fn delete_button(ui: &mut Ui, st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    let deletable = st.selected.len() > 1 || m.layers.removal_refused().is_none();
    if icon_button(ui, Icon::Delete, "Delete", deletable) { delete_layer(st, notice, m, a); }
}

/// Show only the layers of some color labels.
fn label_filter_button(ui: &mut Ui, st: &mut LayersState, m: &mut Model) {
    let fr = icon_toggle(ui, Icon::FilterBy, "Filter by color", true, st.label_filter != 0);
    let fid = egui::Id::new("layer_filter_popup");
    egui::popup::popup_below_widget(ui, fid, &fr, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
        ui.set_min_width(LABELS.len() as f32 * 25.0 + 8.0);
        label_filter_swatches(ui, &mut st.label_filter, m.layers);
        if ui.add_enabled(st.label_filter != 0, egui::Button::new("Show all")).clicked() { st.label_filter = 0; }
    });
    if fr.clicked() { ui.memory_mut(|mem| mem.toggle_popup(fid)); }
}

// --- Docker ---------------------------------------------------------------

pub fn show(ui: &mut Ui, st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    if let Some(cmd) = st.pending.take() { apply(cmd, st, m, a); }
    // (Deleted meanwhile: an undo, a filter layer's Cancel.)
    st.selected.retain(|id| m.layers.node(*id).is_some());
    if st.selected.len() < 2 { st.selected.clear(); }

    // The active node's color label, blend mode and opacity; a filter
    // layer's filter instead of a blend mode.
    let full = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let left = ui.cursor().left();
        let label = m.layers.active_node().props().label;
        let lr = icon_toggle(ui, Icon::Label, "Color label", true, false);
        if let Some(c) = label_color(label) {
            ui.painter().circle_filled(lr.rect.right_bottom() + vec2(-6.0, -6.0), 3.5, c);
        }
        let lid = egui::Id::new("layer_label_popup");
        egui::popup::popup_below_widget(ui, lid, &lr, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            if let Some(l) = label_list(ui, label) { set_label(st, l, m, a); }
        });
        if lr.clicked() { ui.memory_mut(|mem| mem.toggle_popup(lid)); }
        if let Node::Filter(f) = m.layers.active_node() {
            let id = f.id;
            let b = egui::Button::new(format!("{}…", f.filter.kind.name())).min_size(vec2(96.0, 24.0));
            if ui.add(b).clicked() { st.edit_filter = Some((id, false)); }
        } else {
            node_blend_combo(ui, m, a);
        }
        // The rest of the row (available_width overshoots in a row).
        let w = (full - (ui.cursor().left() - left)).max(40.0);
        let node = m.layers.active_node_mut();
        let pct = (node.opacity() * 100.0).round();
        if let Some(v) = crate::widgets::fill_slider(ui, "layer_opacity", crate::widgets::percent("Opacity"), pct, w)
        {
            *node.opacity_mut() = v / 100.0;
            a.recomposite = true;
            a.layer_edit = Some("Opacity");
        }
    });

    // The actions in one row (CSP): new layer (its other kinds under the
    // arrow), new folder, duplicate; the marks, then group; label filter
    // and delete on the right. Too narrow for one row: two, making and
    // deleting above, the marks below (nothing overlaps).
    const ONE_ROW: f32 = 3.0 * 22.0 + 2.0 * 14.0 + 6.0 + 4.0 * 22.0 + 12.0 + 2.0 * 22.0;
    if ui.available_width() >= ONE_ROW {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            make_buttons(ui, st, m, a);
            ui.add_space(6.0);
            mark_buttons(ui, st, m, a);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                delete_button(ui, st, notice, m, a);
                label_filter_button(ui, st, m);
            });
        });
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            make_buttons(ui, st, m, a);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| delete_button(ui, st, notice, m, a));
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            mark_buttons(ui, st, m, a);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| label_filter_button(ui, st, m));
        });
    }
    if st.label_filter != 0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Showing").small().color(theme::TEXT_WEAK));
            for (i, _) in LABELS.iter().enumerate().filter(|(i, _)| st.label_filter >> i & 1 == 1) {
                let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                swatch(ui.painter(), r.center(), i as u8, 5.0);
            }
            if icon_button(ui, Icon::Close, "Show all", true) { st.label_filter = 0; }
        });
    }

    // A layer being renamed may have been deleted meanwhile.
    if st.renaming.as_ref().is_some_and(|(id, _, _)| m.layers.node(*id).is_none()) {
        st.renaming = None;
    }

    let list_h = ui.available_height().max(ROW_H);
    let mut event = None;
    let mut dropped = None;
    let mut menu_cmd = None;
    let mut shown_ids = Vec::new();
    Frame::none().fill(theme::DESK).rounding(3.0).show(ui, |ui| {
        ui.set_min_height(list_h);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let active = m.layers.active_id();
            let mut rects: HashMap<u64, Rect> = HashMap::new();
            let mut chains = Vec::new();
            for row in m.layers.rows() {
                // (The active layer always shows: a new layer has no label
                // yet.)
                if st.label_filter != 0 && row.id != active && !m.layers.node(row.id).is_some_and(|n| shown(n, st.label_filter)) { continue; }
                let (e, resp) = layer_row(ui, st, m, &row);
                if let Some(e) = e { event = Some((row.id, e)); }
                if let Some(d) = drop_target(ui, &resp, &row, &st.selected) { dropped = Some(d); }
                resp.context_menu(|ui| {
                    if let Some(c) = row_menu(ui, m, row.id) { menu_cmd = Some((row.id, c)); }
                });
                rects.insert(row.id, resp.rect);
                if let (true, Some(b)) = (row.clipped, row.below) { chains.push((row.id, b)); }
                shown_ids.push(row.id);
            }
            // The chains, over the rows' edges.
            for (c, b) in chains {
                let (Some(rc), Some(rb)) = (rects.get(&c), rects.get(&b)) else { continue };
                let y = if rb.top() - rc.bottom() < 4.0 { (rc.bottom() + rb.top()) / 2.0 } else { rb.top() };
                chain(ui.painter(), pos2(rc.left() + EYE_X, y));
            }
            drag_autoscroll(ui);
        });
    });
    let multi = st.selected.len() > 1;
    if let Some(d) = egui::DragAndDrop::payload::<LayerDrag>(ui.ctx()) {
        let n = st.selection(m.layers).len();
        match m.layers.node(d.0) {
            Some(_) if multi && st.selected.contains(&d.0) => drag_ghost(ui.ctx(), &format!("{n} layers")),
            Some(node) => drag_ghost(ui.ctx(), node.name()),
            None => {}
        }
    }
    if let Some((id, target, place)) = dropped {
        // The selected layers go together when one of them is dragged.
        let ids = if multi && st.selected.contains(&id) { st.selection(m.layers) } else { vec![id] };
        if m.layers.move_nodes(&ids, id, target, place) {
            a.recomposite = true;
            a.layer_edit = Some("Move layer");
        }
        if ids.len() < 2 { st.selected.clear(); }
    }

    if let Some((id, c)) = menu_cmd {
        // A row of the selection: for all of it; another: for it alone.
        if !st.selection(m.layers).contains(&id) { st.selected.clear(); }
        m.layers.set_active(id);
        match c {
            RowMenu::Cmd(cmd) => apply(cmd, st, m, a),
            RowMenu::Label(l) => set_label(st, l, m, a),
            RowMenu::Rename => st.renaming = m.layers.node(id).filter(|n| n.frame().is_none()).map(|n| (id, n.name().to_owned(), true)),
            RowMenu::Duplicate => duplicate(st, m, a),
            RowMenu::Delete => delete_layer(st, notice, m, a),
            RowMenu::EditFilter => st.edit_filter = Some((id, false)),
        }
    }

    // Applied after the loop: the rows borrow the stack while drawing.
    if let Some((id, e)) = event {
        match e {
            RowEvent::Select(mods) => {
                select(st, m, id, mods, &shown_ids);
                // A frame chosen: go to it.
                if let Some(n) = m.layers.node(id).and_then(Node::frame) {
                    if m.layers.time() != n { m.layers.set_time(n); a.recomposite = true; }
                }
            }
            RowEvent::ToggleVisible => {
                if let Some(node) = m.layers.node_mut(id) { *node.visible_mut() ^= true; }
                a.recomposite = true;
            }
            RowEvent::ToggleLock => {
                if let Some(node) = m.layers.node_mut(id) { *node.locked_mut() ^= true; }
                a.modified = true;
                a.layer_edit = Some("Lock");
            }
            RowEvent::Renamed => {
                a.modified = true;
                a.layer_edit = Some("Rename");
            }
            RowEvent::ToggleCollapse => {
                if let Some(Node::Folder(f)) = m.layers.node_mut(id) { f.collapsed ^= true; }
            }
            RowEvent::EditFilter => {
                st.selected.clear();
                m.layers.set_active(id);
                st.edit_filter = Some((id, false));
            }
            RowEvent::Mark(cmd) => {
                if !st.selection(m.layers).contains(&id) { st.selected.clear(); }
                m.layers.set_active(id);
                apply(cmd, st, m, a);
            }
        }
    }
}

/// A click on row `id`: select it alone; with Ctrl, add or remove it;
/// with Shift, everything from the active row to it (`shown`: the rows
/// top to bottom).
fn select(st: &mut LayersState, m: &mut Model, id: u64, mods: egui::Modifiers, shown: &[u64]) {
    let active = m.layers.active_id();
    if mods.command {
        let mut sel = st.selection(m.layers);
        match sel.iter().position(|x| *x == id) {
            Some(i) if sel.len() > 1 => {
                sel.remove(i);
                if id == active { m.layers.set_active(*sel.last().unwrap()); }
            }
            Some(_) => {}
            None => {
                sel.push(id);
                m.layers.set_active(id);
            }
        }
        st.selected = if sel.len() > 1 { sel } else { Vec::new() };
    } else if mods.shift {
        let (Some(i), Some(j)) = (shown.iter().position(|x| *x == active), shown.iter().position(|x| *x == id)) else { return };
        let (i, j) = (i.min(j), i.max(j));
        st.selected = if i < j { shown[i..=j].to_vec() } else { Vec::new() };
        m.layers.set_active(id);
    } else {
        st.selected.clear();
        m.layers.set_active(id);
    }
}

/// The node shows with the label filter `mask`: its label is one shown,
/// or a folder holds one.
fn shown(n: &Node, mask: u16) -> bool {
    mask >> n.props().label & 1 == 1 || matches!(n, Node::Folder(f) if f.children.iter().any(|c| shown(c, mask)))
}

/// Toggle swatches for the label filter: the labels in use (and "none").
fn label_filter_swatches(ui: &mut Ui, mask: &mut u16, layers: &LayerStack) {
    fn used(nodes: &[Node], out: &mut u16) {
        for n in nodes {
            *out |= 1 << n.props().label;
            if let Node::Folder(f) = n { used(&f.children, out); }
        }
    }
    let mut in_use = 0u16;
    used(&layers.root, &mut in_use);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (i, (name, _)) in LABELS.iter().enumerate() {
            let on = *mask >> i & 1 == 1;
            let usable = in_use >> i & 1 == 1;
            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), if usable { Sense::click() } else { Sense::hover() });
            let resp = resp.on_hover_text(*name);
            let p = ui.painter();
            let fade = if usable { 1.0 } else { 0.25 };
            match label_color(i as u8) {
                Some(c) => { p.circle_filled(r.center(), 7.0, c.gamma_multiply(fade)); }
                None => { p.circle_stroke(r.center(), 7.0, Stroke::new(1.2_f32, theme::TEXT_WEAK.gamma_multiply(fade))); }
            }
            if on { p.circle_stroke(r.center(), 10.0, Stroke::new(1.8_f32, theme::TEXT)); }
            if resp.clicked() { *mask ^= 1 << i; }
        }
    });
}

/// Blend combo for the active node; folders also offer Pass Through.
fn node_blend_combo(ui: &mut Ui, m: &mut Model, a: &mut Actions) {
    let node = m.layers.active_node_mut();
    let label = node.blend_name();
    egui::ComboBox::from_id_salt("node_blend")
        .selected_text(label)
        .width(104.0)
        .show_ui(ui, |ui| match node {
            Node::Layer(l) => {
                for mode in BlendMode::ALL {
                    if ui.selectable_value(&mut l.blend, mode, mode.name()).changed() {
                        a.recomposite = true;
                    }
                }
            }
            Node::Folder(f) => {
                if ui.selectable_value(&mut f.blend, FolderBlend::PassThrough, "Pass Through").changed() {
                    a.recomposite = true;
                }
                for mode in BlendMode::ALL {
                    if ui.selectable_value(&mut f.blend, FolderBlend::Isolated(mode), mode.name()).changed() {
                        a.recomposite = true;
                    }
                }
            }
            Node::Filter(_) => {}
        });
}

enum RowEvent { Select(egui::Modifiers), ToggleVisible, ToggleLock, ToggleCollapse, Renamed, EditFilter, Mark(LayerCmd) }

enum RowMenu { Cmd(LayerCmd), Label(u8), Rename, Duplicate, Delete, EditFilter }

/// A row's right-click menu.
fn row_menu(ui: &mut Ui, m: &Model, id: u64) -> Option<RowMenu> {
    let node = m.layers.node(id)?;
    let p = node.props();
    let mut out = None;
    if let Node::Filter(f) = node {
        if ui.button(format!("Edit {}…", f.filter.kind.name())).clicked() { out = Some(RowMenu::EditFilter); }
        ui.separator();
    }
    if ui.button("Rename").clicked() { out = Some(RowMenu::Rename); }
    if ui.button("Duplicate").clicked() { out = Some(RowMenu::Duplicate); }
    if ui.button("Delete").clicked() { out = Some(RowMenu::Delete); }
    ui.separator();
    if let Some(l) = label_swatches(ui, p.label) { out = Some(RowMenu::Label(l)); }
    ui.separator();
    for (cmd, on) in [(LayerCmd::Clip, p.clip), (LayerCmd::Reference, p.reference), (LayerCmd::Draft, p.draft)] {
        let label = format!("{} {}", if on { "✔" } else { "   " }, cmd.name());
        if ui.add(egui::Button::new(label).shortcut_text(cmd.shortcut())).clicked() { out = Some(RowMenu::Cmd(cmd)); }
    }
    ui.separator();
    if ui.add(egui::Button::new("Group").shortcut_text("Ctrl+G")).clicked() { out = Some(RowMenu::Cmd(LayerCmd::Group)); }
    if node.is_folder() && ui.add(egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G")).clicked() { out = Some(RowMenu::Cmd(LayerCmd::Ungroup)); }
    if out.is_some() { ui.close_menu(); }
    out
}

/// Drag and drop on a row: where the dragged layer would land (a line, or
/// the folder outlined), and the move once released there.
fn drop_target(ui: &Ui, resp: &egui::Response, row: &Row, selected: &[u64]) -> Option<(u64, u64, Place)> {
    let dragged = resp.dnd_hover_payload::<LayerDrag>()?;
    if dragged.0 == row.id { return None; }
    // (Several dragged together: not onto one of them.)
    if selected.contains(&dragged.0) && selected.contains(&row.id) { return None; }
    let pointer = ui.ctx().pointer_interact_pos()?;
    let r = resp.rect;
    let t = (pointer.y - r.top()) / r.height();
    let place = match (row.is_folder, t) {
        (true, t) if (0.25..0.75).contains(&t) => Place::Inside,
        (_, t) if t < 0.5 => Place::Above,
        _ => Place::Below,
    };
    let p = ui.painter();
    let stroke = Stroke::new(2.0_f32, theme::DROP);
    match place {
        Place::Inside => { p.rect_stroke(r.shrink(1.0), 2.5, stroke); }
        Place::Above => { p.hline(r.x_range(), r.top(), stroke); }
        Place::Below => { p.hline(r.x_range(), r.bottom(), stroke); }
    }
    let released = resp.dnd_release_payload::<LayerDrag>()?;
    Some((released.0, row.id, place))
}

fn layer_row(ui: &mut Ui, st: &mut LayersState, m: &mut Model, row: &Row) -> (Option<RowEvent>, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
    if resp.drag_started() { resp.dnd_set_drag_payload(LayerDrag(row.id)); }
    let event = row_content(ui, st, m, row, rect, &resp);
    (event, resp)
}

/// Layers in a folder (for its info line).
fn count(nodes: &[Node]) -> usize {
    nodes.iter().map(|n| match n { Node::Folder(f) => count(&f.children), _ => 1 }).sum()
}

fn row_content(ui: &mut Ui, st: &mut LayersState, m: &mut Model, row: &Row, rect: Rect, resp: &egui::Response) -> Option<RowEvent> {
    let node = m.layers.node(row.id)?;
    let (name, visible, opacity, locked) = (node.name().to_owned(), node.visible(), node.opacity(), node.locked());
    let props = node.props();
    let blend = node.blend_name();
    let active = m.layers.active_id() == row.id;
    let picked = !active && st.selected.contains(&row.id);
    let is_filter = matches!(node, Node::Filter(_));
    let folder_size = match node { Node::Folder(f) if row.frame.is_none() && !row.animation => Some(count(&f.children)), _ => None };
    let sub_cels = match node { Node::Folder(f) => f.children.len(), _ => 0 };
    let label = label_color(props.label);

    let painter = ui.painter_at(rect);
    let body = rect.shrink2(vec2(2.0, 0.0));
    // Folders stand out (Layer Kit); a color label tints the row lightly.
    let bg = if active {
        theme::ACCENT
    } else if picked {
        theme::ACCENT.gamma_multiply(if resp.hovered() { 0.6 } else { 0.5 })
    } else if let Some(c) = label {
        c.gamma_multiply(if resp.hovered() { 0.32 } else { 0.22 })
    } else if row.is_folder {
        if resp.hovered() { theme::WIDGET_HOVER } else { theme::SURFACE }
    } else if resp.hovered() {
        theme::WIDGET
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(body, 2.5, bg);
    if row.is_folder && !active {
        painter.rect_stroke(body, 2.5, Stroke::new(1.0_f32, theme::BORDER));
    }
    // The label: a strip on the left edge.
    if let Some(c) = label {
        let strip = Rect::from_min_max(pos2(body.left() + 1.0, body.top() + 5.0), pos2(body.left() + 4.0, body.bottom() - 5.0));
        painter.rect_filled(strip, 1.5, c);
    }

    // Eye, at the far left whatever the depth (CSP / Krita). A frame has
    // none: the current frame decides which one shows.
    let eye = if row.frame.is_some() { Rect::NOTHING } else { Rect::from_center_size(pos2(rect.left() + EYE_X, rect.center().y), vec2(22.0, 22.0)) };
    let (eye_icon, eye_col) = if visible { (Icon::Eye, theme::TEXT) } else { (Icon::EyeClosed, if active { Color32::from_white_alpha(170) } else { theme::TEXT_WEAK }) };
    if row.frame.is_none() { crate::icons::paint(ui, eye_icon, eye, 16.0, eye_col); }

    // Lock, at the far right (Krita).
    let lock = Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 22.0));
    let lock_col = if locked {
        theme::TEXT
    } else if row.locked_by_parent {
        theme::TEXT_WEAK
    } else if resp.hovered() {
        theme::TEXT_WEAK.gamma_multiply(0.7)
    } else {
        if active { Color32::from_white_alpha(110) } else { theme::TEXT_WEAK.gamma_multiply(0.35) }
    };
    let lock_icon = if locked || row.locked_by_parent { Icon::Lock } else { Icon::LockOpen };
    crate::icons::paint(ui, lock_icon, lock, 15.0, lock_col);

    // Marks before the lock: reference (lighthouse), draft (pencil); a
    // parent folder's shown fainter.
    let mut marks_left = lock.left();
    let mut mark_rects: Vec<(Rect, LayerCmd)> = Vec::new();
    for (on, by_parent, icon, cmd) in [
        (props.reference, row.reference_by_parent, Icon::Reference, LayerCmd::Reference),
        (props.draft, row.draft_by_parent, Icon::Draft, LayerCmd::Draft),
    ] {
        if !on && !by_parent { continue; }
        let r = Rect::from_center_size(pos2(marks_left - 10.0, rect.center().y), vec2(20.0, 22.0));
        let col = match (on, cmd) {
            (true, LayerCmd::Draft) => DRAFT_BLUE,
            (true, _) => if active { theme::TEXT } else { Color32::from_rgb(240, 200, 90) },
            (false, _) => theme::TEXT_WEAK.gamma_multiply(0.6),
        };
        crate::icons::paint(ui, icon, r, 14.0, col);
        if on { mark_rects.push((r, cmd)); }
        marks_left = r.left();
    }

    // Content, indented by folder depth (and clipping).
    let mut x = rect.left() + EYE_X + 11.0 + 4.0 + row.depth as f32 * INDENT;
    if row.clipped { x += CLIP_INDENT; }
    let mut toggle_rect = Rect::NOTHING;
    let icon_box = if row.is_folder {
        // Disclosure triangle + folder icon.
        toggle_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 20.0));
        let chevron = if row.collapsed { Icon::Collapsed } else { Icon::Expanded };
        crate::icons::paint(ui, chevron, toggle_rect, 13.0, theme::TEXT_WEAK);
        let b = Rect::from_center_size(pos2(toggle_rect.right() + 4.0 + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let folder = if row.animation { Icon::Film } else if row.collapsed { Icon::Folder } else { Icon::FolderOpen };
        crate::icons::paint(ui, folder, b, 20.0, if active { theme::TEXT } else { label.unwrap_or(theme::TEXT_WEAK) });
        b
    } else if let Some(n) = row.frame {
        // A frame: its number on a card (shown now: brighter).
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let card = Rect::from_center_size(b.center(), vec2(THUMB - 4.0, THUMB - 10.0));
        painter.rect_filled(card, 2.0, if active { Color32::from_black_alpha(70) } else if visible { theme::WIDGET_HOVER } else { theme::WIDGET });
        painter.rect_stroke(card, 2.0, Stroke::new(1.0_f32, if visible || active { theme::TEXT } else { theme::BORDER }));
        painter.text(card.center(), egui::Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(if n < 1000 { 13.0 } else { 10.0 }), theme::TEXT);
        b
    } else if is_filter {
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let tile = Rect::from_center_size(b.center(), vec2(THUMB - 6.0, THUMB - 6.0));
        painter.rect_filled(tile, 2.0, if active { Color32::from_black_alpha(70) } else { theme::WIDGET });
        painter.rect_stroke(tile, 2.0, Stroke::new(1.0_f32, if active { theme::TEXT } else { theme::BORDER }));
        crate::icons::paint(ui, Icon::FilterLayer, tile, 18.0, theme::TEXT);
        b
    } else {
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        if let Some(tex) = st.thumbs.get(&row.id) {
            let s = tex.size_vec2();
            let k = (THUMB / s.x).min(THUMB / s.y);
            let r = Rect::from_center_size(b.center(), s * k);
            checker(&painter, r, 4.0);
            painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, if active { theme::TEXT } else { theme::BORDER }));
        }
        b
    };
    // A draft: a blue line left of the thumbnail (CSP).
    if props.draft || row.draft_by_parent {
        let lx = icon_box.left() - 3.0;
        painter.line_segment([pos2(lx, icon_box.top() + 3.0), pos2(lx, icon_box.bottom() - 3.0)], Stroke::new(2.0_f32, DRAFT_BLUE));
    }

    // "100 % Normal" above the name, like CSP.
    let text_x = icon_box.right() + 8.0;
    let name_rect = Rect::from_min_max(pos2(text_x, rect.center().y - 4.0), pos2(marks_left - 4.0, rect.center().y + 14.0));
    let frames = if row.animation { m.layers.frames(row.id).len() } else { 0 };
    let info = match folder_size {
        _ if row.animation => format!("Animation  ·  {frames} frame{}", if frames == 1 { "" } else { "s" }),
        _ if row.frame.is_some() => format!("{sub_cels} sub-cels{}", if visible { "  ·  shown" } else { "" }),
        Some(n) => format!("{:.0} %  {}  ·  {n} layer{}", opacity * 100.0, blend, if n == 1 { "" } else { "s" }),
        None if is_filter => format!("{:.0} %  Filter: {}", opacity * 100.0, blend),
        None => format!("{:.0} %  {}", opacity * 100.0, blend),
    };
    crate::widgets::elided_text(
        &painter,
        pos2(text_x, rect.center().y - 7.0),
        &info,
        egui::FontId::proportional(10.0),
        if active { Color32::from_white_alpha(200) } else { theme::TEXT_WEAK },
        name_rect.width(),
    );
    let renaming = st.renaming.as_ref().is_some_and(|(rid, _, _)| *rid == row.id);
    if renaming {
        let (_, buf, first) = st.renaming.as_mut().unwrap();
        let edit = ui.put(name_rect, egui::TextEdit::singleline(buf));
        // Focus once: requesting it every frame kept the field open after
        // Enter (focus came straight back).
        if *first {
            edit.request_focus();
            *first = false;
        }
        if edit.lost_focus() {
            let new_name = buf.trim().to_owned();
            st.renaming = None;
            if !new_name.is_empty() && new_name != name {
                if let Some(n) = m.layers.node_mut(row.id) { *n.name_mut() = new_name; }
                return Some(RowEvent::Renamed);
            }
        }
    } else {
        // Long names (an imported image's file name) end with "…" before
        // the marks instead of running under them.
        let font = if row.is_folder { egui::FontId::proportional(12.5) } else { egui::FontId::proportional(12.0) };
        crate::widgets::elided_text(
            &painter,
            pos2(text_x, rect.center().y + 6.0),
            &name,
            font,
            if active { Color32::WHITE } else { theme::TEXT },
            name_rect.width(),
        );
    }

    let pt = resp.interact_pointer_pos();
    if resp.double_clicked() && !renaming {
        // (A frame's name is its place in time.)
        if row.frame.is_none() && pt.is_some_and(|pt| pt.x >= text_x && pt.x < marks_left) {
            st.renaming = Some((row.id, name, true));
            return None;
        }
        if is_filter && pt.is_some_and(|pt| icon_box.contains(pt)) { return Some(RowEvent::EditFilter); }
    }
    if resp.clicked() {
        let pt = pt?;
        if eye.contains(pt) { return Some(RowEvent::ToggleVisible); }
        if lock.contains(pt) { return Some(RowEvent::ToggleLock); }
        if toggle_rect.contains(pt) { return Some(RowEvent::ToggleCollapse); }
        if let Some((_, cmd)) = mark_rects.iter().find(|(r, _)| r.contains(pt)) { return Some(RowEvent::Mark(*cmd)); }
        return Some(RowEvent::Select(ui.input(|i| i.modifiers)));
    }
    None
}
