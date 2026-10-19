//! Layers docker (CSP / Krita): blend mode + opacity of the active node,
//! action icons, the active node's marks, then the layer tree: eye, color
//! label, thumbnail / folder / filter icon, "100 % Normal" over the name,
//! marks, lock. Rows drag and drop above, below, or into a folder; a right
//! click opens their menu.
//!
//! Marks (`aurastudio_layers::Props`): a color label (Krita's colors, the
//! list can be filtered by them, as Krita and the Label Box plugin do),
//! clipping to the node below (CSP), reference and draft (CSP). Folders
//! stand out from layers (Layer Kit darkens them). Filter layers (Krita)
//! open their filter's dialog on a double-click.

use std::collections::{HashMap, HashSet};

use aurastudio_filters::{Filter, GROUPS};
use aurastudio_layers::{FolderBlend, LayerStack, Node, Place, Row, LABELS};
use egui::{pos2, vec2, Align, Color32, Frame, Layout, Rect, RichText, Sense, Stroke, Ui};

use crate::widgets::*;
use crate::{theme, Actions, BlendMode, Model, Notice};

const THUMB: f32 = 36.0;
const ROW_H: f32 = 44.0;
const INDENT: f32 = 14.0;
/// Clipped rows: the thumbnail moves right, a hooked arrow before it.
const CLIP_INDENT: f32 = 14.0;
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
}

impl LayersState {
    pub fn invalidate(&mut self, id: u64) {
        self.stale.insert(id);
    }

    pub fn invalidate_all(&mut self) {
        self.thumbs.clear();
    }

    pub fn shortcut(&mut self, cmd: LayerCmd) { self.pending = Some(cmd); }

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
}

pub fn new_folder(m: &mut Model, a: &mut Actions) {
    m.layers.add_folder("Folder");
    a.recomposite = true;
}

/// A new filter layer above the active node, its dialog opened (Cancel
/// there removes it again).
pub fn new_filter_layer(st: &mut LayersState, kind: aurastudio_filters::Kind, m: &mut Model, a: &mut Actions) {
    let color = match kind { aurastudio_filters::Kind::BlackWhite => Filter::new(kind).color, _ => m.brush.color };
    let id = m.layers.add_filter_layer(Filter { color, ..Filter::new(kind) });
    st.edit_filter = Some((id, true));
    a.recomposite = true;
}

pub fn delete_layer(notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    match m.layers.removal_refused() {
        Some(why) => notice.set(why),
        None => {
            m.layers.remove_active();
            a.recomposite = true;
        }
    }
}

/// Layer commands with a shortcut (and a menu entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerCmd {
    /// Put the active node in a new folder (Ctrl+G).
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
    pub fn shortcut(self) -> &'static str {
        match self {
            LayerCmd::Group => "Ctrl+G",
            LayerCmd::Ungroup => "Ctrl+Shift+G",
            LayerCmd::Clip => "Ctrl+Alt+G",
            LayerCmd::Reference | LayerCmd::Draft => "",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            LayerCmd::Group => "Put the layer in a new folder",
            LayerCmd::Ungroup => "Take the folder's layers out and remove the folder",
            LayerCmd::Clip => "Show only where the layer below has pixels (several layers can clip to the same one)",
            LayerCmd::Reference => "What the fill tool and \"Stay inside the lines\" look at (CSP's reference layer): mark the line art",
            LayerCmd::Draft => "A sketch: shown, but left out of fills, references and exports (CSP's draft layer)",
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

pub fn apply(cmd: LayerCmd, m: &mut Model, a: &mut Actions) {
    match cmd {
        LayerCmd::Group => { m.layers.group_active("Folder"); }
        LayerCmd::Ungroup => { if !m.layers.ungroup_active() { return; } }
        LayerCmd::Clip => { let p = m.layers.active_node_mut().props_mut(); p.clip = !p.clip; }
        LayerCmd::Reference => { let p = m.layers.active_node_mut().props_mut(); p.reference = !p.reference; }
        LayerCmd::Draft => { let p = m.layers.active_node_mut().props_mut(); p.draft = !p.draft; }
    }
    // (Reference and draft change no pixel, only what tools look at.)
    if matches!(cmd, LayerCmd::Reference | LayerCmd::Draft) { a.modified = true; } else { a.recomposite = true; }
}

/// The Layer menu's entries for the marks and folders.
pub fn menu_entries(ui: &mut Ui, st: &mut LayersState, m: &mut Model, a: &mut Actions) {
    ui.menu_button("New filter layer", |ui| filter_layer_menu(ui, st, m, a));
    ui.separator();
    for cmd in [LayerCmd::Group, LayerCmd::Ungroup] {
        if ui.add_enabled(cmd.enabled(m.layers), egui::Button::new(cmd.name()).shortcut_text(cmd.shortcut())).on_hover_text(cmd.hint()).clicked() {
            apply(cmd, m, a);
            ui.close_menu();
        }
    }
    ui.separator();
    for cmd in [LayerCmd::Clip, LayerCmd::Reference, LayerCmd::Draft] {
        let label = format!("{} {}", if cmd.is_on(m.layers) { "✔" } else { "   " }, cmd.name());
        if ui.add(egui::Button::new(label).shortcut_text(cmd.shortcut())).on_hover_text(cmd.hint()).clicked() {
            apply(cmd, m, a);
            ui.close_menu();
        }
    }
    ui.menu_button("Color label", |ui| {
        if label_swatches(ui, m.layers.active_node().props().label) .map(|l| { m.layers.active_node_mut().props_mut().label = l; a.modified = true; }).is_some() {
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
            if ui.button(kind.name()).on_hover_text(kind.hint()).clicked() {
                new_filter_layer(st, kind, m, a);
                // (In the Layer menu or the docker's popup.)
                ui.close_menu();
                ui.memory_mut(|mem| mem.close_popup());
            }
        }
    }
}

/// The label colors as round swatches (the current one ringed); the one
/// clicked, if any.
fn label_swatches(ui: &mut Ui, current: u8) -> Option<u8> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for (i, (name, rgb)) in LABELS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            let resp = resp.on_hover_text(*name);
            let p = ui.painter();
            if i == 0 {
                p.circle_stroke(r.center(), 7.0, Stroke::new(1.2_f32, theme::TEXT_WEAK));
                p.line_segment([r.center() + vec2(-5.0, 5.0), r.center() + vec2(5.0, -5.0)], Stroke::new(1.2_f32, theme::TEXT_WEAK));
            } else {
                p.circle_filled(r.center(), 7.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
            }
            if current as usize == i { p.circle_stroke(r.center(), 9.5, Stroke::new(1.5_f32, theme::TEXT)); }
            else if resp.hovered() { p.circle_stroke(r.center(), 9.5, Stroke::new(1.0_f32, theme::TEXT_WEAK)); }
            if resp.clicked() { picked = Some(i as u8); }
        }
    });
    picked
}

fn label_color(label: u8) -> Option<Color32> {
    (label != 0).then(|| LABELS.get(label as usize)).flatten().map(|(_, c)| Color32::from_rgb(c[0], c[1], c[2]))
}

/// A layer row being dragged (egui drag and drop payload).
struct LayerDrag(u64);

// --- Docker ---------------------------------------------------------------

pub fn show(ui: &mut Ui, st: &mut LayersState, notice: &mut Notice, m: &mut Model, a: &mut Actions) {
    if let Some(cmd) = st.pending.take() { apply(cmd, m, a); }

    // Active node settings (CSP / Krita): blend mode + opacity; a filter
    // layer's filter instead of a blend mode.
    let full = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let left = ui.cursor().left();
        if let Node::Filter(f) = m.layers.active_node() {
            let id = f.id;
            let b = egui::Button::new(format!("{}…", f.filter.kind.name())).min_size(vec2(104.0, 24.0));
            if ui.add(b).on_hover_text("Edit the filter (or double-click the layer)").clicked() { st.edit_filter = Some((id, false)); }
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
        }
    });

    // Actions at the top, like CSP.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        if icon_button(ui, Icon::NewLayer, "New layer", true) { new_layer(m, a); }
        if icon_button(ui, Icon::NewFolder, "New folder", true) { new_folder(m, a); }
        let fl = icon_toggle(ui, Icon::FilterLayer, "New filter layer: an adjustment you can change later (Krita)", true, false);
        egui::popup::popup_below_widget(ui, egui::Id::new("new_filter_layer"), &fl, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(170.0);
            // Flat entries, as in a menu.
            ui.style_mut().visuals.button_frame = false;
            filter_layer_menu(ui, st, m, a);
        });
        if fl.clicked() { ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("new_filter_layer"))); }
        if icon_button(ui, Icon::Duplicate, "Duplicate", true) {
            m.layers.duplicate_active();
            a.recomposite = true;
        }
        if icon_button(ui, Icon::Up, "Move up", true) {
            m.layers.move_active(true);
            a.recomposite = true;
        }
        if icon_button(ui, Icon::Down, "Move down", true) {
            m.layers.move_active(false);
            a.recomposite = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Delete, "Delete", m.layers.removal_refused().is_none()) {
                delete_layer(notice, m, a);
            }
        });
    });

    // The active node's marks, and the folder commands (CSP's palette
    // icons); the color filter on the right.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (cmd, icon) in [(LayerCmd::Clip, Icon::Clip), (LayerCmd::Reference, Icon::Reference), (LayerCmd::Draft, Icon::Draft)] {
            let tip = format!("{}{}\n{}", cmd.name(), if cmd.shortcut().is_empty() { String::new() } else { format!(" ({})", cmd.shortcut()) }, cmd.hint());
            if icon_toggle(ui, icon, &tip, true, cmd.is_on(m.layers)).clicked() { apply(cmd, m, a); }
        }
        let label = m.layers.active_node().props().label;
        let lr = icon_toggle(ui, Icon::Label, "Color label", true, false);
        if let Some(c) = label_color(label) {
            ui.painter().circle_filled(lr.rect.right_bottom() + vec2(-6.0, -6.0), 3.5, c);
        }
        let lid = egui::Id::new("layer_label_popup");
        egui::popup::popup_below_widget(ui, lid, &lr, egui::PopupCloseBehavior::CloseOnClick, |ui| {
            if let Some(l) = label_swatches(ui, label) {
                m.layers.active_node_mut().props_mut().label = l;
                a.modified = true;
            }
        });
        if lr.clicked() { ui.memory_mut(|mem| mem.toggle_popup(lid)); }
        ui.add_space(6.0);
        for (cmd, icon) in [(LayerCmd::Group, Icon::Group), (LayerCmd::Ungroup, Icon::Ungroup)] {
            let tip = format!("{} ({})\n{}", cmd.name(), cmd.shortcut(), cmd.hint());
            if icon_toggle(ui, icon, &tip, cmd.enabled(m.layers), false).clicked() { apply(cmd, m, a); }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let fr = icon_toggle(ui, Icon::FilterBy, "Show only the layers with some color labels", true, st.label_filter != 0);
            let fid = egui::Id::new("layer_filter_popup");
            egui::popup::popup_below_widget(ui, fid, &fr, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(LABELS.len() as f32 * 25.0 + 8.0);
                ui.label(RichText::new("Show the labels").small().color(theme::TEXT_WEAK));
                label_filter_swatches(ui, &mut st.label_filter, m.layers);
                if ui.add_enabled(st.label_filter != 0, egui::Button::new("Show all")).clicked() { st.label_filter = 0; }
            });
            if fr.clicked() { ui.memory_mut(|mem| mem.toggle_popup(fid)); }
        });
    });
    if st.label_filter != 0 {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Showing").small().color(theme::TEXT_WEAK));
            for (i, _) in LABELS.iter().enumerate().filter(|(i, _)| st.label_filter >> i & 1 == 1) {
                let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                match label_color(i as u8) {
                    Some(c) => { ui.painter().circle_filled(r.center(), 5.0, c); }
                    None => { ui.painter().circle_stroke(r.center(), 5.0, Stroke::new(1.0_f32, theme::TEXT_WEAK)); }
                }
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
    Frame::none().fill(theme::DESK).rounding(6.0).show(ui, |ui| {
        ui.set_min_height(list_h);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let active = m.layers.active_id();
            for row in m.layers.rows() {
                // (The active layer always shows: a new layer has no label
                // yet.)
                if st.label_filter != 0 && row.id != active && !m.layers.node(row.id).is_some_and(|n| shown(n, st.label_filter)) { continue; }
                let (e, resp) = layer_row(ui, st, m, &row);
                if let Some(e) = e { event = Some((row.id, e)); }
                if let Some(d) = drop_target(ui, &resp, &row) { dropped = Some(d); }
                resp.context_menu(|ui| {
                    if let Some(c) = row_menu(ui, m, row.id) { menu_cmd = Some((row.id, c)); }
                });
            }
        });
    });
    if let Some(d) = egui::DragAndDrop::payload::<LayerDrag>(ui.ctx()) {
        if let Some(n) = m.layers.node(d.0) { drag_ghost(ui.ctx(), n.name()); }
    }
    if let Some((id, target, place)) = dropped {
        if m.layers.move_node(id, target, place) { a.recomposite = true; }
    }

    if let Some((id, c)) = menu_cmd {
        m.layers.set_active(id);
        match c {
            RowMenu::Cmd(cmd) => apply(cmd, m, a),
            RowMenu::Label(l) => { m.layers.active_node_mut().props_mut().label = l; a.modified = true; }
            RowMenu::Rename => st.renaming = m.layers.node(id).map(|n| (id, n.name().to_owned(), true)),
            RowMenu::Duplicate => { m.layers.duplicate_active(); a.recomposite = true; }
            RowMenu::Delete => delete_layer(notice, m, a),
            RowMenu::EditFilter => st.edit_filter = Some((id, false)),
        }
    }

    // Applied after the loop: the rows borrow the stack while drawing.
    if let Some((id, e)) = event {
        let node = m.layers.node_mut(id).expect("row node exists");
        match e {
            RowEvent::Select => m.layers.set_active(id),
            RowEvent::ToggleVisible => {
                *node.visible_mut() ^= true;
                a.recomposite = true;
            }
            RowEvent::ToggleLock => {
                *node.locked_mut() ^= true;
                a.modified = true;
            }
            RowEvent::Renamed => a.modified = true,
            RowEvent::ToggleCollapse => {
                if let Node::Folder(f) = node { f.collapsed ^= true; }
            }
            RowEvent::EditFilter => {
                m.layers.set_active(id);
                st.edit_filter = Some((id, false));
            }
            RowEvent::Mark(cmd) => {
                m.layers.set_active(id);
                apply(cmd, m, a);
            }
        }
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
            let resp = resp.on_hover_text(if usable { name.to_string() } else { format!("{name} (no layer has it)") });
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

enum RowEvent { Select, ToggleVisible, ToggleLock, ToggleCollapse, Renamed, EditFilter, Mark(LayerCmd) }

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
        if ui.add(egui::Button::new(label).shortcut_text(cmd.shortcut())).on_hover_text(cmd.hint()).clicked() { out = Some(RowMenu::Cmd(cmd)); }
    }
    ui.separator();
    if ui.add(egui::Button::new("Group").shortcut_text("Ctrl+G")).clicked() { out = Some(RowMenu::Cmd(LayerCmd::Group)); }
    if node.is_folder() && ui.add(egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G")).clicked() { out = Some(RowMenu::Cmd(LayerCmd::Ungroup)); }
    if out.is_some() { ui.close_menu(); }
    out
}

/// Drag and drop on a row: where the dragged layer would land (a line, or
/// the folder outlined), and the move once released there.
fn drop_target(ui: &Ui, resp: &egui::Response, row: &Row) -> Option<(u64, u64, Place)> {
    let dragged = resp.dnd_hover_payload::<LayerDrag>()?;
    if dragged.0 == row.id { return None; }
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
        Place::Inside => { p.rect_stroke(r.shrink(1.0), 5.0, stroke); }
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
    let is_filter = matches!(node, Node::Filter(_));
    let folder_size = match node { Node::Folder(f) => Some(count(&f.children)), _ => None };
    let label = label_color(props.label);

    let painter = ui.painter_at(rect);
    let body = rect.shrink2(vec2(2.0, 0.0));
    // Folders stand out (Layer Kit); a color label tints the row lightly.
    let bg = if active {
        theme::ACCENT
    } else if let Some(c) = label {
        c.gamma_multiply(if resp.hovered() { 0.32 } else { 0.22 })
    } else if row.is_folder {
        if resp.hovered() { theme::WIDGET_HOVER } else { theme::SURFACE }
    } else if resp.hovered() {
        theme::WIDGET
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(body, 5.0, bg);
    if row.is_folder && !active {
        painter.rect_stroke(body, 5.0, Stroke::new(1.0_f32, theme::BORDER));
    }
    // The label: a strip on the left edge.
    if let Some(c) = label {
        let strip = Rect::from_min_max(pos2(body.left() + 1.0, body.top() + 5.0), pos2(body.left() + 4.0, body.bottom() - 5.0));
        painter.rect_filled(strip, 1.5, c);
    }

    // Eye, at the far left whatever the depth (CSP / Krita).
    let eye = Rect::from_center_size(pos2(rect.left() + 17.0, rect.center().y), vec2(22.0, 22.0));
    let (eye_icon, eye_col) = if visible { (Icon::Eye, theme::TEXT) } else { (Icon::EyeClosed, if active { Color32::from_white_alpha(170) } else { theme::TEXT_WEAK }) };
    crate::icons::paint(ui, eye_icon, eye, 16.0, eye_col);

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
    let mut x = eye.right() + 4.0 + row.depth as f32 * INDENT;
    if row.clipped {
        // A hooked arrow pointing down at the base (CSP / Photoshop).
        let arrow = Rect::from_min_size(pos2(x - 2.0, rect.center().y - 9.0), vec2(16.0, 16.0));
        crate::icons::paint(ui, Icon::Clip, arrow, 13.0, if active { theme::TEXT } else { Color32::from_rgb(240, 120, 120) });
        x += CLIP_INDENT;
    }
    let mut toggle_rect = Rect::NOTHING;
    let icon_box = if row.is_folder {
        // Disclosure triangle + folder icon.
        toggle_rect = Rect::from_center_size(pos2(x + 7.0, rect.center().y), vec2(14.0, 20.0));
        let chevron = if row.collapsed { Icon::Collapsed } else { Icon::Expanded };
        crate::icons::paint(ui, chevron, toggle_rect, 13.0, theme::TEXT_WEAK);
        let b = Rect::from_center_size(pos2(toggle_rect.right() + 4.0 + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let folder = if row.collapsed { Icon::Folder } else { Icon::FolderOpen };
        crate::icons::paint(ui, folder, b, 20.0, if active { theme::TEXT } else { label.unwrap_or(theme::TEXT_WEAK) });
        b
    } else if is_filter {
        let b = Rect::from_center_size(pos2(x + THUMB / 2.0, rect.center().y), vec2(THUMB, THUMB));
        let tile = Rect::from_center_size(b.center(), vec2(THUMB - 6.0, THUMB - 6.0));
        painter.rect_filled(tile, 4.0, if active { Color32::from_black_alpha(70) } else { theme::WIDGET });
        painter.rect_stroke(tile, 4.0, Stroke::new(1.0_f32, if active { theme::TEXT } else { theme::BORDER }));
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
    let info = match folder_size {
        Some(n) => format!("{:.0} %  {}  ·  {n} layer{}", opacity * 100.0, blend, if n == 1 { "" } else { "s" }),
        None if is_filter => format!("{:.0} %  Filter: {}", opacity * 100.0, blend),
        None => format!("{:.0} %  {}", opacity * 100.0, blend),
    };
    crate::widgets::elided_text(
        &painter,
        pos2(text_x, rect.center().y - 9.0),
        &info,
        egui::FontId::proportional(10.5),
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
        let font = if row.is_folder { egui::FontId::proportional(13.5) } else { egui::FontId::proportional(13.0) };
        crate::widgets::elided_text(
            &painter,
            pos2(text_x, rect.center().y + 7.0),
            &name,
            font,
            if active { Color32::WHITE } else { theme::TEXT },
            name_rect.width(),
        );
    }

    let pt = resp.interact_pointer_pos();
    if resp.double_clicked() && !renaming {
        if pt.is_some_and(|pt| pt.x >= text_x && pt.x < marks_left) {
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
        return Some(RowEvent::Select);
    }
    None
}
