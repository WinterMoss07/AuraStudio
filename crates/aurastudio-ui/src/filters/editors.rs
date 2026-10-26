//! Each filter's controls and pictures of what it does, made by the
//! filter itself (Levels and Hue / Saturation after PhotoCraft, MIT,
//! see NOTICE).

use std::hash::{Hash, Hasher};

use aurastudio_filters::tone::{self, LevelsChannel};
use aurastudio_filters::{idx, Filter, HueRange, Kind, BW_DEFAULTS, RANGE_NAMES};
use egui::{pos2, vec2, Align2, Color32, Rect, RichText, Sense, Stroke, TextureHandle, Ui};

use super::draw::{self, Spec, Track, WIDTH};
use super::source::Source;
use crate::theme;

/// What an editor remembers while its dialog is open.
#[derive(Default)]
pub struct State {
    /// Levels' channel (0 RGB, 1 R, 2 G, 3 B).
    channel: usize,
    /// Hue / Saturation's range (0 master, 1..6 reds … magentas).
    range:   usize,
    /// The handle being dragged, and where it was grabbed (its x minus
    /// the pointer's: it doesn't jump under the pointer).
    drag:    Option<usize>,
    grab:    f32,
    /// The painting's main colors.
    palette: Option<Vec<[u8; 3]>>,
    /// A sample picture (emboss, shadow) and what it shows.
    sample:  Option<(u64, TextureHandle)>,
}

/// The controls of `f`'s filter. Returns lines of information.
pub fn show(ui: &mut Ui, f: &mut Filter, src: &Source, st: &mut State) -> Vec<String> {
    let mut info = Vec::new();
    match f.kind {
        Kind::BrightnessContrast => brightness_contrast(ui, f, src, &mut info),
        Kind::Levels => levels(ui, f, src, st, &mut info),
        Kind::HueSaturation => hue_saturation(ui, f, src, st, &mut info),
        Kind::BlackWhite => black_white(ui, f, src, st),
        Kind::Invert => {
            amount(ui, f, "Amount", Track::Colors(&[draw::gray(128), Color32::WHITE]));
            palette(ui, f, src, st);
        }
        Kind::Posterize => posterize(ui, f, src, st, &mut info),
        Kind::Threshold => threshold(ui, f, src, &mut info),
        Kind::Sepia => {
            amount(ui, f, "Amount", Track::Colors(&[draw::gray(150), Color32::from_rgb(196, 160, 110)]));
            palette(ui, f, src, st);
        }
        Kind::Opacity => {
            amount(ui, f, "Opacity", Track::Alpha(Color32::WHITE));
            let a = f.v(0) / 100.0;
            info.push(format!("Fully opaque pixels become {:.0} / 255 (alpha multiplied by {:.0} %)", (255.0 * a).round(), a * 100.0));
        }
        Kind::GaussianBlur => {
            let mut r = f.v(0);
            if draw::slider(ui, "radius", Spec::new("Radius", 0.0, 250.0, 4.0, "px").step(0.1).pow(2.5), &mut r) { f.values[0] = r; }
            info.push(format!("σ {r:.1} px: the blur reaches {} px around (three box blurs, W3C feGaussianBlur)", f.reach()));
        }
        Kind::UnsharpMask => unsharp(ui, f, &mut info),
        Kind::Thicken | Kind::Thin => lines(ui, f, &mut info),
        Kind::FindEdges => {
            amount(ui, f, "Strength", Track::Fill(0.0));
            info.push("Sobel gradient of each color: dark where the image changes, white elsewhere".into());
        }
        Kind::Emboss => emboss(ui, f, st, &mut info),
        Kind::DropShadow => {
            drop_shadow(ui, f, st, &mut info);
            if !src.transparent { info.push("This layer is opaque (paper): its shadow stays hidden under it. Use it on a layer with transparent parts".into()); }
        }
    }
    info
}

/// The value at 0 as a 0..100 % slider.
fn amount(ui: &mut Ui, f: &mut Filter, label: &str, track: Track) {
    let mut v = f.v(0);
    let d = f.kind.defaults()[0];
    if draw::slider(ui, "amount", Spec::new(label, 0.0, 100.0, d, "%").track(track), &mut v) { f.values[0] = v; }
}

// --- Tones ----------------------------------------------------------------

/// Counts of the layer's channel values after the filter's tables.
fn mapped(h: &[u32; 256], t: &[u8; 256]) -> [u32; 256] {
    let mut o = [0u32; 256];
    for i in 0..256 { o[t[i] as usize] += h[i]; }
    o
}

/// Histograms of R, G and B together, before and after.
fn rgb_after(src: &Source, t: &[[u8; 256]; 3]) -> [u32; 256] {
    let mut o = [0u32; 256];
    for c in 0..3 { for (i, v) in mapped(&src.hist.chan[c], &t[c]).iter().enumerate() { o[i] += v; } }
    o
}

/// Share of the channel values the tables push to black / white that
/// weren't (clipping).
fn clipped(hists: &[&[u32; 256]], tables: &[&[u8; 256]]) -> (f64, f64) {
    let (mut lo, mut hi, mut n) = (0u64, 0u64, 0u64);
    for (h, t) in hists.iter().zip(tables) {
        for i in 0..256 {
            n += h[i] as u64;
            if t[i] == 0 && i > 0 { lo += h[i] as u64; }
            if t[i] == 255 && i < 255 { hi += h[i] as u64; }
        }
    }
    let n = n.max(1) as f64;
    (lo as f64 * 100.0 / n, hi as f64 * 100.0 / n)
}

/// `clipped` over R, G and B with tables `t`.
fn rgb_clipping(src: &Source, t: &[[u8; 256]; 3]) -> (f64, f64) {
    clipped(&[&src.hist.chan[0], &src.hist.chan[1], &src.hist.chan[2]], &[&t[0], &t[1], &t[2]])
}

fn clip_line(info: &mut Vec<String>, (lo, hi): (f64, f64)) {
    if lo >= 0.05 || hi >= 0.05 {
        info.push(format!("Clipped: {lo:.1} % to black, {hi:.1} % to white (detail lost there)"));
    } else {
        info.push("No clipping: every tone keeps its detail".into());
    }
}

/// The histogram before (gray) and after (outline), the curve over them.
fn tone_graph(ui: &mut Ui, before: &[u32; 256], after: &[u32; 256], curves: &[(&[u8; 256], Color32)], h: f32) -> (Rect, egui::Response, egui::Painter) {
    let (r, resp, p) = draw::graph(ui, h, Sense::click_and_drag());
    draw::grid(&p, r, !curves.is_empty());
    let top = draw::hist_top(before).max(draw::hist_top(after));
    draw::histogram(&p, r, before, top, draw::BEFORE);
    draw::histogram_line(&p, r, after, top, draw::AFTER);
    for (t, c) in curves { draw::curve(&p, r, t, *c, 1.6); }
    draw::frame(&p, r);
    (r, resp, p)
}

/// The layer's histogram before and after a filter that is a tone table
/// on each channel (a viewer of the painting, no curve), and the
/// clipping.
fn tone_view(ui: &mut Ui, f: &Filter, src: &Source, info: &mut Vec<String>) {
    let Some(t) = f.tone_tables() else { return };
    let after = rgb_after(src, &t);
    let (r, _, p) = tone_graph(ui, &src.hist.rgb, &after, &[], 120.0);
    draw::corner(&p, r, Align2::LEFT_TOP, "Histogram: before (gray), after (line)");
    gray_bar(ui);
    clip_line(info, rgb_clipping(src, &t));
}

/// A black to white bar under a tone graph (the input levels).
fn gray_bar(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(WIDTH, 7.0), Sense::hover());
    draw::gradient(ui.painter(), r, &[Color32::BLACK, Color32::WHITE]);
}

fn brightness_contrast(ui: &mut Ui, f: &mut Filter, src: &Source, info: &mut Vec<String>) {
    tone_view(ui, f, src, info);
    ui.add_space(4.0);
    let legacy = f.v(idx::LEGACY) >= 0.5;
    let (mut b, mut c) = (f.v(0), f.v(1));
    if draw::slider(ui, "brightness", Spec::new("Brightness", -150.0, 150.0, 0.0, "").track(Track::Colors(&[Color32::BLACK, draw::gray(128), Color32::WHITE])), &mut b) { f.values[0] = b; }
    let lo = if legacy { -100.0 } else { -50.0 };
    if draw::slider(ui, "contrast", Spec::new("Contrast", lo, 100.0, 0.0, "").track(Track::Fill(0.0)), &mut c) { f.values[1] = c; }
    let mut l = legacy;
    if ui.checkbox(&mut l, "Use legacy").changed() {
        f.values[idx::LEGACY] = if l { 1.0 } else { 0.0 };
        if !l { f.values[1] = f.values[1].max(-50.0); }
    }
}

/// Levels' channels.
const CHANNELS: [&str; 4] = ["RGB", "Red", "Green", "Blue"];

fn levels(ui: &mut Ui, f: &mut Filter, src: &Source, st: &mut State, info: &mut Vec<String>) {
    let colors = [Color32::WHITE, draw::RED, draw::GREEN, draw::BLUE];
    let hist_colors = [draw::BEFORE, draw::RED.gamma_multiply(0.55), draw::GREEN.gamma_multiply(0.55), draw::BLUE.gamma_multiply(0.55)];
    let mut auto = false;
    ui.horizontal(|ui| {
        ui.set_width(WIDTH);
        ui.label(RichText::new("Channel").color(theme::TEXT_WEAK));
        draw::choice(ui, &mut st.channel, &CHANNELS);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            auto = ui.button("Auto").clicked();
        });
    });
    let ch = st.channel.min(3);
    let base = idx::levels(ch);
    let hist = *src.hist.channel(ch);
    if auto {
        let (lo, hi) = tone::auto_levels(&hist);
        f.values[base] = lo;
        f.values[base + 1] = 1.0;
        f.values[base + 2] = hi;
    }
    let tables = f.tone_tables().unwrap();
    let l = f.levels(ch);
    let own: [u8; 256] = std::array::from_fn(|i| (l.eval(i as f32 / 255.0) * 255.0).round() as u8);
    let after = if ch == 0 { rgb_after(src, &tables) } else { mapped(&src.hist.chan[ch - 1], &tables[ch - 1]) };

    // Histogram, with this channel's curve.
    let (r, _, p) = draw::graph(ui, 118.0, Sense::hover());
    draw::grid(&p, r, true);
    let top = draw::hist_top(&hist).max(draw::hist_top(&after));
    draw::histogram(&p, r, &hist, top, hist_colors[ch]);
    draw::histogram_line(&p, r, &after, top, draw::AFTER);
    draw::curve(&p, r, &own, colors[ch], 1.4);
    // Shade the levels cut off by the input points.
    let xof = |v: f32| r.left() + v / 255.0 * r.width();
    let shade = Color32::from_black_alpha(110);
    p.rect_filled(Rect::from_min_max(r.min, pos2(xof(l.in_black), r.bottom())), 0.0, shade);
    p.rect_filled(Rect::from_min_max(pos2(xof(l.in_white), r.top()), r.max), 0.0, shade);
    draw::frame(&p, r);
    draw::corner(&p, r, Align2::LEFT_TOP, "Input levels: before (filled), after (line)");

    // Input: black, gamma, white.
    let mut lv = [l.in_black, l.gamma, l.in_white, l.out_black, l.out_white];
    let g = tone::gamma_pos(lv[0], lv[2], lv[1]);
    if let Some(k) = handles(ui, st, 0, &[lv[0], g, lv[2]], &[Color32::BLACK, draw::gray(128), Color32::WHITE], &[Color32::BLACK, colors[ch]]) {
        let (i, x) = k;
        match i {
            0 => lv[0] = x.min(lv[2] - 2.0).round(),
            2 => lv[2] = x.max(lv[0] + 2.0).round(),
            _ => lv[1] = (tone::gamma_from_pos(lv[0], lv[2], x) * 100.0).round() / 100.0,
        }
    }
    ui.horizontal(|ui| {
        ui.set_width(WIDTH);
        ui.add(egui::DragValue::new(&mut lv[0]).range(0.0..=253.0).speed(1.0)).on_hover_text("Input black");
        ui.add_space(78.0);
        ui.add(egui::DragValue::new(&mut lv[1]).range(0.01..=9.99).speed(0.01).fixed_decimals(2)).on_hover_text("Midtones");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::DragValue::new(&mut lv[2]).range(2.0..=255.0).speed(1.0)).on_hover_text("Input white");
        });
    });
    ui.add_space(4.0);
    draw::caption(ui, "Output levels");
    if let Some((i, x)) = handles(ui, st, 10, &[lv[3], lv[4]], &[Color32::BLACK, Color32::WHITE], &[Color32::BLACK, colors[ch]]) {
        lv[3 + i] = x.round();
    }
    ui.horizontal(|ui| {
        ui.set_width(WIDTH);
        ui.add(egui::DragValue::new(&mut lv[3]).range(0.0..=255.0).speed(1.0)).on_hover_text("Output black");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::DragValue::new(&mut lv[4]).range(0.0..=255.0).speed(1.0)).on_hover_text("Output white");
        });
    });
    lv[0] = lv[0].min(253.0);
    lv[2] = lv[2].max(lv[0] + 2.0);
    f.values[base..base + 5].copy_from_slice(&lv);
    ui.horizontal(|ui| {
        if ui.small_button("Reset channel").clicked() { f.values[base..base + 5].copy_from_slice(&[0.0, 1.0, 255.0, 0.0, 255.0]); }
        let changed: Vec<&str> = (0..4).filter(|&c| c != ch && f.levels(c) != LevelsChannel::IDENTITY).map(|c| CHANNELS[c]).collect();
        if !changed.is_empty() { ui.label(RichText::new(format!("Also changed: {}", changed.join(", "))).small().color(theme::TEXT_WEAK)); }
    });
    let t = f.tone_tables().unwrap();
    info.push(format!("{}: input {:.0}–{:.0}, midtones {:.2}, output {:.0}–{:.0}", CHANNELS[ch], lv[0], lv[2], lv[1], lv[3], lv[4]));
    clip_line(info, rgb_clipping(src, &t));
}

/// A gradient bar (`bar` colors) and triangle handles at the levels
/// `xs` (0..255) under it; returns (handle, level) while one is dragged.
/// `id` tells the rows apart.
fn handles(ui: &mut Ui, st: &mut State, id: usize, xs: &[f32], fills: &[Color32], bar: &[Color32]) -> Option<(usize, f32)> {
    let (r, resp) = ui.allocate_exact_size(vec2(WIDTH, 22.0), Sense::click_and_drag());
    let b = Rect::from_min_max(pos2(r.left() + 1.0, r.top()), pos2(r.right() - 1.0, r.top() + 8.0));
    let xof = |v: f32| b.left() + v / 255.0 * b.width();
    let vof = |x: f32| ((x - b.left()) / b.width() * 255.0).clamp(0.0, 255.0);
    if resp.drag_started() || (resp.is_pointer_button_down_on() && st.drag.is_none()) {
        if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
            let hx: Vec<f32> = xs.iter().map(|v| xof(*v)).collect();
            st.drag = draw::nearest(&hx, pos.x).map(|k| id + k);
            // Grabbed on the handle: it follows from there; beside it: it
            // comes to the pointer.
            st.grab = st.drag.map_or(0.0, |k| { let d = hx[k - id] - pos.x; if d.abs() <= 7.0 { d } else { 0.0 } });
        }
    }
    let mut out = None;
    if let (Some(k), true) = (st.drag, resp.is_pointer_button_down_on() || resp.dragged()) {
        if (id..id + xs.len()).contains(&k) {
            if let Some(pos) = resp.interact_pointer_pos() {
                let v = vof(pos.x + st.grab);
                // (A press that hasn't moved changes nothing.)
                if (v - xs[k - id]).abs() >= 0.5 || resp.dragged() { out = Some((k - id, v)); }
            }
        }
    }
    if !resp.is_pointer_button_down_on() && !resp.dragged() && st.drag.is_some_and(|k| (id..id + xs.len()).contains(&k)) { st.drag = None; }
    let p = ui.painter_at(r.expand(6.0));
    draw::gradient(&p, b, bar);
    p.rect_stroke(b, 0.0, Stroke::new(1.0_f32, theme::BORDER), egui::StrokeKind::Middle);
    for (k, v) in xs.iter().enumerate() {
        draw::handle(&p, xof(*v), b.bottom() + 1.0, fills[k], st.drag == Some(id + k));
    }
    out
}

fn posterize(ui: &mut Ui, f: &mut Filter, src: &Source, st: &mut State, info: &mut Vec<String>) {
    let mut n = f.v(0);
    if draw::slider(ui, "levels", Spec::new("Levels", 2.0, 64.0, 4.0, "per channel").track(Track::Fill(2.0)).pow(1.6), &mut n) { f.values[0] = n; }
    let k = n.round() as u64;
    info.push(format!("{k} tones per channel: at most {} colors", k * k * k));
    palette(ui, f, src, st);
}

fn threshold(ui: &mut Ui, f: &mut Filter, src: &Source, info: &mut Vec<String>) {
    let lvl = f.v(0);
    let (r, resp, p) = draw::graph(ui, 110.0, Sense::click_and_drag());
    let top = draw::hist_top(&src.hist.lum);
    let x = r.left() + lvl / 255.0 * r.width();
    p.rect_filled(Rect::from_min_max(r.min, pos2(x, r.bottom())), 0.0, Color32::from_black_alpha(90));
    p.rect_filled(Rect::from_min_max(pos2(x, r.top()), r.max), 0.0, Color32::from_white_alpha(18));
    draw::histogram(&p, r, &src.hist.lum, top, draw::gray(150));
    p.line_segment([pos2(x, r.top()), pos2(x, r.bottom())], Stroke::new(1.5_f32, theme::ACCENT));
    draw::frame(&p, r);
    draw::corner(&p, r, Align2::LEFT_TOP, "Luminosity · drag to split");
    if let (true, Some(pos)) = (resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
        f.values[0] = ((pos.x - r.left()) / r.width() * 255.0).round().clamp(1.0, 255.0);
    }
    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal); }
    gray_bar(ui);
    let mut v = f.v(0);
    if draw::slider(ui, "level", Spec::new("Threshold level", 1.0, 255.0, 128.0, "").track(Track::Colors(&[Color32::BLACK, Color32::WHITE])), &mut v) { f.values[0] = v; }
    let v = f.v(0);
    let black: u64 = (0..256).filter(|&i| !tone::threshold_white([i as u8; 3], v)).map(|i| src.hist.lum[i] as u64).sum();
    let pb = black as f64 * 100.0 / src.hist.count.max(1) as f64;
    info.push(format!("{pb:.0} % of the pixels turn black, {:.0} % white", 100.0 - pb));
}

// --- Colors ---------------------------------------------------------------

/// Up to 14 of the painting's main colors, dark to light (once per
/// dialog).
fn main_colors(src: &Source) -> Vec<[u8; 3]> {
    let l = src.level_under(1024);
    let mut buckets = vec![(0u64, [0u64; 3]); 4096];
    for (i, p) in l.px.chunks_exact(4).enumerate() {
        if p[3] < 128 || l.mask.as_ref().is_some_and(|m| !m[i]) { continue; }
        let k = (p[0] as usize >> 4) << 8 | (p[1] as usize >> 4) << 4 | p[2] as usize >> 4;
        let b = &mut buckets[k];
        b.0 += 1;
        for c in 0..3 { b.1[c] += p[c] as u64; }
    }
    let mut all: Vec<(u64, [u8; 3])> = buckets.iter().filter(|b| b.0 > 0).map(|b| (b.0, b.1.map(|s| (s / b.0) as u8))).collect();
    all.sort_by_key(|b| std::cmp::Reverse(b.0));
    let mut out: Vec<[u8; 3]> = Vec::new();
    // Distinct colors: skip one too close to one already taken.
    for (_, c) in all {
        if out.len() >= 14 { break; }
        if out.iter().all(|o| (0..3).map(|k| (o[k] as i32 - c[k] as i32).abs()).sum::<i32>() > 40) { out.push(c); }
    }
    out.sort_by_key(|c| tone::luminosity(*c));
    out
}

/// The painting's colors before and after.
fn palette(ui: &mut Ui, f: &Filter, src: &Source, st: &mut State) {
    let colors = st.palette.get_or_insert_with(|| main_colors(src));
    if colors.is_empty() { return; }
    draw::caption(ui, "Your colors, before / after");
    let px: Vec<u8> = colors.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect();
    let out = f.apply(&px, colors.len(), 1);
    let n = colors.len();
    let (r, _) = ui.allocate_exact_size(vec2(WIDTH, 30.0), Sense::hover());
    let cw = r.width() / n as f32;
    let p = ui.painter();
    for i in 0..n {
        let x0 = r.left() + i as f32 * cw;
        let a = Rect::from_min_max(pos2(x0 + 1.0, r.top()), pos2(x0 + cw - 1.0, r.center().y - 1.0));
        let b = Rect::from_min_max(pos2(x0 + 1.0, r.center().y + 1.0), pos2(x0 + cw - 1.0, r.bottom()));
        p.rect_filled(a, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, Color32::from_rgb(px[i * 4], px[i * 4 + 1], px[i * 4 + 2]));
        p.rect_filled(b, egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, Color32::from_rgb(out[i * 4], out[i * 4 + 1], out[i * 4 + 2]));
    }
}

/// Hues around the circle as the filter turns them.
fn spectrum_after(f: &Filter, n: usize) -> Vec<Color32> {
    let px: Vec<u8> = (0..n).flat_map(|i| { let c = draw::hue(i as f32 * 360.0 / n as f32); [c.r(), c.g(), c.b(), 255] }).collect();
    let out = f.apply(&px, n, 1);
    out.chunks_exact(4).map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect()
}

fn hue_saturation(ui: &mut Ui, f: &mut Filter, src: &Source, st: &mut State, info: &mut Vec<String>) {
    let colorize = f.v(idx::COLORIZE) >= 0.5;
    if colorize { st.range = 0; }
    ui.add_enabled_ui(!colorize, |ui| {
        let names: Vec<&str> = std::iter::once("Master").chain(RANGE_NAMES.iter().copied()).collect();
        draw::choice(ui, &mut st.range, &names);
    });
    let range = st.range.min(6);
    let base = if range == 0 { 0 } else { idx::range(range - 1) };
    let center = if range == 0 { 0.0 } else { 60.0 * (range - 1) as f32 };
    let (hlo, hhi, slo) = if colorize { (0.0, 360.0, 0.0) } else { (-180.0, 180.0, -100.0) };
    let mut h = f.values[base].clamp(hlo, hhi);
    let shifted: Vec<Color32> = (0..=12).map(|i| draw::hue(center + if colorize { 30.0 * i as f32 } else { -180.0 + 30.0 * i as f32 })).collect();
    if draw::slider(ui, "hue", Spec::new("Hue", hlo, hhi, 0.0, "°").track(Track::Colors(&shifted)), &mut h) { f.values[base] = h; }
    let now = draw::hue(center + h);
    let mut s = f.values[base + 1].clamp(slo, 100.0);
    let sd = if colorize { 25.0 } else { 0.0 };
    if draw::slider(ui, "sat", Spec::new("Saturation", slo, 100.0, sd, "%").track(Track::Colors(&[draw::gray(128), now])), &mut s) { f.values[base + 1] = s; }
    let mut l = f.values[base + 2].clamp(-100.0, 100.0);
    if draw::slider(ui, "light", Spec::new("Lightness", -100.0, 100.0, 0.0, "%").track(Track::Colors(&[Color32::BLACK, now, Color32::WHITE])), &mut l) { f.values[base + 2] = l; }
    let mut c = colorize;
    if ui.checkbox(&mut c, "Colorize").changed() {
        f.values[idx::COLORIZE] = if c { 1.0 } else { 0.0 };
        let h = f.values[0];
        f.values[0] = if c { h.rem_euclid(360.0) } else if h > 180.0 { h - 360.0 } else { h };
        if c { f.values[1] = f.values[1].max(25.0); } else { f.values[1] = f.values[1].min(100.0); }
    }

    // Spectrum: every hue before (top) and after (bottom); the range in
    // between, its handles dragged.
    ui.add_space(4.0);
    let (bars, resp) = ui.allocate_exact_size(vec2(WIDTH, 42.0), Sense::click_and_drag());
    let top = Rect::from_min_size(bars.min, vec2(WIDTH, 11.0));
    let bottom = Rect::from_min_size(pos2(bars.left(), bars.bottom() - 11.0), vec2(WIDTH, 11.0));
    let p = ui.painter_at(bars.expand(4.0));
    draw::cells(&p, top, 120, |i| draw::hue(i as f32 * 3.0));
    let after = spectrum_after(f, 120);
    draw::cells(&p, bottom, 120, |i| after[i]);
    if range > 0 && !colorize {
        let b0 = f.range(range - 1).bounds;
        let mut b = HueRange::canonical_bounds(b0);
        let xof = |deg: f32| bars.left() + deg.rem_euclid(360.0) / 360.0 * WIDTH;
        let mid = bars.center().y;
        let band = |a: f32, z: f32, color: Color32| {
            let (xa, xz) = (xof(a), xof(z));
            let row = |x0: f32, x1: f32| { p.rect_filled(Rect::from_min_max(pos2(x0, mid - 3.0), pos2(x1, mid + 3.0)), 0.0, color); };
            if xa <= xz { row(xa, xz); } else { row(xa, bars.right()); row(bars.left(), xz); }
        };
        band(b[0], b[3], draw::gray(90));
        band(b[1], b[2], draw::gray(200));
        if resp.drag_started() || (resp.is_pointer_button_down_on() && st.drag.is_none()) {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
                st.drag = draw::nearest(&b.map(xof), pos.x).filter(|k| (xof(b[*k]) - pos.x).abs() < 12.0);
            }
        }
        if let (Some(k), true, Some(pos)) = (st.drag, resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
            if k < 4 {
                let deg = ((pos.x - bars.left()) / WIDTH * 360.0).clamp(0.0, 360.0);
                b = HueRange::move_handle(b, k, deg);
                let r0 = idx::range(range - 1) + 3;
                f.values[r0..r0 + 4].copy_from_slice(&b.map(f32::round));
            }
        }
        if !resp.is_pointer_button_down_on() && !resp.dragged() { st.drag = None; }
        for (k, d) in b.iter().enumerate() {
            let x = xof(*d);
            let fill = if k == 1 || k == 2 { theme::TEXT } else { draw::gray(70) };
            let hr = Rect::from_center_size(pos2(x, mid), vec2(6.0, 14.0));
            p.rect_filled(hr, 1.0, fill);
            p.rect_stroke(hr, 1.0, Stroke::new(1.0_f32, if st.drag == Some(k) { theme::ACCENT } else { draw::gray(160) }), egui::StrokeKind::Middle);
        }
        let n = |v: f32| v.rem_euclid(360.0).round();
        info.push(format!("{}: fully from {:.0}° to {:.0}°, fading out to {:.0}° and {:.0}° (drag the handles)", RANGE_NAMES[range - 1], n(b[1]), n(b[2]), n(b[0]), n(b[3])));
        if ui.small_button("Reset range").clicked() {
            let r0 = idx::range(range - 1);
            let nr = HueRange::neutral(range - 1);
            f.values[r0..r0 + 7].copy_from_slice(&[0.0, 0.0, 0.0, nr.bounds[0], nr.bounds[1], nr.bounds[2], nr.bounds[3]]);
        }
    }
    info.push("Spectrum: hues before (top), after (bottom)".into());
    palette(ui, f, src, st);
}

fn black_white(ui: &mut Ui, f: &mut Filter, src: &Source, st: &mut State) {
    let colors = [draw::hue(0.0), draw::hue(60.0), draw::hue(120.0), draw::hue(180.0), draw::hue(240.0), draw::hue(300.0)];
    for i in 0..6 {
        let mut v = f.v(i).clamp(-200.0, 300.0);
        if draw::slider(ui, RANGE_NAMES[i], Spec::new(RANGE_NAMES[i], -200.0, 300.0, BW_DEFAULTS[i], "%").track(Track::Colors(&[Color32::BLACK, colors[i]])), &mut v) { f.values[i] = v; }
    }
    ui.horizontal(|ui| {
        let mut on = f.v(idx::TINT) >= 0.5;
        if ui.checkbox(&mut on, "Tint").changed() { f.values[idx::TINT] = if on { 1.0 } else { 0.0 }; }
        ui.add_enabled_ui(on, |ui| { egui::color_picker::color_edit_button_srgb(ui, &mut f.color); });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Default").clicked() { f.values[..6].copy_from_slice(&BW_DEFAULTS); }
        });
    });
    palette(ui, f, src, st);
}

// --- Blur, sharpen, lines -------------------------------------------------

#[derive(Clone, Copy)]
enum Sample {
    /// Dark on the left, light on the right.
    Edge,
    /// A dark line this many px wide on light.
    Line(usize),
}

/// A hard edge or a line across, before and after the filter (the
/// luminosity of each pixel): for the numbers under the controls.
fn profile(f: &Filter, sample: Sample, span: usize) -> (Vec<u8>, Vec<u8>) {
    let span = span.clamp(6, 400);
    let pad = f.reach() as usize + 2;
    let n = 2 * (span + pad);
    let c = n / 2;
    let (dark, light) = (40u8, 225u8);
    let before: Vec<u8> = (0..n).map(|x| match sample {
        Sample::Edge => if x < c { dark } else { light },
        Sample::Line(w) => if x >= c - w / 2 && x < c - w / 2 + w { dark } else { light },
    }).collect();
    let px: Vec<u8> = before.iter().flat_map(|&v| [v, v, v, 255]).collect();
    let out = f.apply(&px, n, 1);
    let after: Vec<u8> = out.chunks_exact(4).map(|p| tone::luminosity([p[0], p[1], p[2]])).collect();
    (before, after)
}

fn unsharp(ui: &mut Ui, f: &mut Filter, info: &mut Vec<String>) {
    let (mut a, mut r, mut t) = (f.v(0), f.v(1), f.v(2));
    if draw::slider(ui, "amount", Spec::new("Amount", 1.0, 500.0, 80.0, "%").pow(1.5), &mut a) { f.values[0] = a; }
    if draw::slider(ui, "radius", Spec::new("Radius", 0.1, 100.0, 2.0, "px").step(0.1).pow(2.5), &mut r) { f.values[1] = r; }
    if draw::slider(ui, "threshold", Spec::new("Threshold", 0.0, 255.0, 0.0, "levels").pow(1.5), &mut t) { f.values[2] = t; }
    let (before, after) = profile(f, Sample::Edge, (3.0 * r + 6.0) as usize);
    let over = after.iter().zip(&before).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
    info.push(format!("Halos up to {over} levels, about {:.0} px wide on each side of an edge", (2.0 * r).max(1.0)));
    if t > 0.0 { info.push(format!("Differences under {t:.0} levels are left alone (skin, paper grain)")); }
}

fn lines(ui: &mut Ui, f: &mut Filter, info: &mut Vec<String>) {
    let mut r = f.v(0);
    if draw::slider(ui, "radius", Spec::new("Radius", 0.0, 50.0, 1.0, "px").step(0.5).pow(1.8), &mut r) { f.values[0] = r; }
    let mut round = f.v(idx::ROUND) >= 0.5;
    if ui.checkbox(&mut round, "Round").changed() {
        f.values[idx::ROUND] = if round { 1.0 } else { 0.0 };
    }
    let w = 6;
    let (before, after) = profile(f, Sample::Line(w), (w + 2 * r.ceil() as usize + 6).max(10));
    let width = |v: &[u8]| v.iter().filter(|&&x| x < 128).count();
    info.push(format!("A {} px line becomes {} px", width(&before), width(&after)));
    info.push("Works on line art on a transparent layer as on paper".into());
}

// --- Relief and shadow: a sample ------------------------------------------

/// Coverage of a shape at (x, y) from its signed distance (inside < 0).
fn cover(d: f32) -> f32 { (0.5 - d).clamp(0.0, 1.0) }

/// The sample: a disc, a rounded square, a ring and a stroke, `w` × `h`.
fn shapes(w: usize, h: usize, x: f32, y: f32) -> f32 {
    let (fw, fh) = (w as f32, h as f32);
    let disc = ((x - fw * 0.18).hypot(y - fh * 0.5)) - fh * 0.3;
    let (bx, by, bs) = ((x - fw * 0.47).abs(), (y - fh * 0.5).abs(), fh * 0.26);
    let rounded = (bx - bs + 6.0).max(0.0).hypot((by - bs + 6.0).max(0.0)) + (bx - bs + 6.0).max(by - bs + 6.0).min(0.0) - 6.0;
    let ring = (((x - fw * 0.78).hypot(y - fh * 0.5)) - fh * 0.26).abs() - 3.5;
    cover(disc).max(cover(rounded)).max(cover(ring))
}

/// The sample shapes as RGBA, `color` giving each pixel from its
/// coverage.
fn sample_pixels(w: usize, h: usize, color: impl Fn(f32) -> [u8; 4]) -> Vec<u8> {
    let mut px = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w { px.extend_from_slice(&color(shapes(w, h, x as f32 + 0.5, y as f32 + 0.5))); }
    }
    px
}

fn sample_texture(ui: &mut Ui, st: &mut State, key: u64, w: usize, h: usize, make: impl FnOnce() -> Vec<u8>) -> TextureHandle {
    if st.sample.as_ref().map(|s| s.0) != Some(key) {
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], &make());
        match &mut st.sample {
            Some((k, t)) => { t.set(img, egui::TextureOptions::LINEAR); *k = key; }
            None => st.sample = Some((key, ui.ctx().load_texture("filter-sample", img, egui::TextureOptions::LINEAR))),
        }
    }
    st.sample.as_ref().unwrap().1.clone()
}

/// Feed what makes `f`'s result (kind, color, values) to `h`.
pub(super) fn hash_filter(f: &Filter, h: &mut impl Hasher) {
    (f.kind, f.color).hash(h);
    for v in &f.values { v.to_bits().hash(h); }
}

fn key_of(f: &Filter) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hash_filter(f, &mut h);
    h.finish()
}

fn emboss(ui: &mut Ui, f: &mut Filter, st: &mut State, info: &mut Vec<String>) {
    let mut a = f.v(0);
    if draw::angle(ui, "Light from", &mut a, 135.0) { f.values[0] = a; }
    let (mut h, mut m) = (f.v(1), f.v(2));
    if draw::slider(ui, "height", Spec::new("Height", 1.0, 20.0, 3.0, "px"), &mut h) { f.values[1] = h; }
    if draw::slider(ui, "amount", Spec::new("Amount", 1.0, 500.0, 100.0, "%").pow(1.5), &mut m) { f.values[2] = m; }
    let (w, hh) = ((WIDTH * 1.5) as usize, 120usize);
    let f: &Filter = f;
    let tex = sample_texture(ui, st, key_of(f), w, hh, || {
        let px = sample_pixels(w, hh, |c| { let v = (235.0 - c * 170.0) as u8; [v, v, v, 255] });
        f.apply(&px, w, hh)
    });
    let (r, _, p) = draw::graph(ui, 80.0, Sense::hover());
    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    draw::frame(&p, r);
    draw::corner(&p, r, Align2::LEFT_TOP, "Sample");
    info.push(format!("Lit from {a:.0}°: edges facing the light turn light, the others dark"));
}

fn drop_shadow(ui: &mut Ui, f: &mut Filter, st: &mut State, info: &mut Vec<String>) {
    let mut a = f.v(0);
    if draw::angle(ui, "Light from", &mut a, 120.0) { f.values[0] = a; }
    let (mut d, mut sp, mut sz, mut o) = (f.v(1), f.v(2), f.v(3), f.v(4));
    if draw::slider(ui, "distance", Spec::new("Distance", 0.0, 300.0, 12.0, "px").pow(1.8), &mut d) { f.values[1] = d; }
    if draw::slider(ui, "spread", Spec::new("Spread", 0.0, 100.0, 0.0, "%"), &mut sp) { f.values[2] = sp; }
    if draw::slider(ui, "size", Spec::new("Size", 0.0, 250.0, 16.0, "px").pow(1.8), &mut sz) { f.values[3] = sz; }
    let col = Color32::from_rgb(f.color[0], f.color[1], f.color[2]);
    if draw::slider(ui, "opacity", Spec::new("Opacity", 0.0, 100.0, 60.0, "%").track(Track::Alpha(col)), &mut o) { f.values[4] = o; }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Color").color(theme::TEXT));
        egui::color_picker::color_edit_button_srgb(ui, &mut f.color);
    });
    // The sample is small: a long or wide shadow is shown reduced.
    let (w, h) = ((WIDTH * 1.5) as usize, 120usize);
    let reach = f.reach() as f32;
    let scale = (36.0 / reach.max(1.0)).min(1.0);
    let f: &Filter = f;
    let tex = sample_texture(ui, st, key_of(f), w, h, || {
        let px = sample_pixels(w, h, |c| [245, 240, 230, (c * 255.0) as u8]);
        f.scaled(scale).apply(&px, w, h)
    });
    let (r, _, p) = draw::graph(ui, 80.0, Sense::hover());
    p.rect_filled(r, 0.0, Color32::from_rgb(150, 156, 166));
    p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    draw::frame(&p, r);
    draw::corner(&p, r, Align2::LEFT_TOP, &if scale < 0.999 { format!("Sample, shown at 1:{:.0}", 1.0 / scale) } else { "Sample".into() });
    let (dx, dy) = aurastudio_filters::spatial::shadow_offset(a, d);
    info.push(format!("Shadow moved {dx:+.0}, {dy:+.0} px; soft over {:.0} px", sz * (1.0 - sp / 100.0)));
}
