//! Stress test: a painter at work, not random clicks. Weighted profiles
//! (sketch, painting, inking, comics flatting), realistic pen paths at
//! 200 Hz with pressure ramps, big MyPaint brushes, smudge / blend, many
//! layers, lasso + copy / paste, fill, geometry tools, undo bursts, saves.
//! Runs on the real paint thread and documents; the "GPU" side takes the
//! merged pieces like the app does.
//!
//! For each kind of work it reports how long the paint thread took per
//! stroke compared with how long the pen took to draw it (above 1x, the
//! painting would trail the pen), worst cases, and memory.
//!
//!     PAINT_SECS=1200 cargo test --release -p aurastudio painter_session -- --ignored --nocapture

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aurastudio_brush::{Brush, PenSample};
use aurastudio_brushes::Library;
use aurastudio_document::{DocId, Document, SharedDoc};
use aurastudio_layers::{FillParams, Node, Selection};

use crate::fuzz::{merge_all_in_bands, rss_mb, Rng};
use crate::paint_thread::{Cmd, PaintThread};
use crate::projection::Projection;

static PANICKED: AtomicBool = AtomicBool::new(false);

type P = (f32, f32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Work { Sketch, Paint, Blend, Ink, Erase, Shape, Fill, Lasso, Paste, Layers, Undo, Save }

#[derive(Default)]
struct Stat { n: u64, samples: u64, busy: Duration, pen: Duration, worst: Duration, worst_ratio: f64, lagging: u64 }

impl Stat {
    fn add(&mut self, busy: Duration, pen: Duration, samples: usize) {
        self.n += 1;
        self.samples += samples as u64;
        self.busy += busy;
        self.pen += pen;
        self.worst = self.worst.max(busy);
        if pen > Duration::ZERO {
            let ratio = busy.as_secs_f64() / pen.as_secs_f64();
            self.worst_ratio = self.worst_ratio.max(ratio);
            // Painting slower than the pen: the stroke would trail it.
            if ratio > 1.0 { self.lagging += 1; }
        }
    }
}

/// Brushes of a kind, by name (the bundled MyPaint sets and ours).
fn pick(lib: &Library, words: &[&str]) -> Vec<usize> {
    lib.presets.iter().enumerate()
        .filter(|(_, p)| { let n = p.name.to_lowercase(); words.iter().any(|w| n.contains(w)) })
        .map(|(i, _)| i).collect()
}

/// Pen pressure along a stroke (t in 0..1): pressed in, held with a
/// little tremor, lifted out.
fn pressure(r: &mut Rng, t: f32, base: f32) -> f32 {
    let s = |e0: f32, e1: f32, x: f32| { let k = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0); k * k * (3.0 - 2.0 * k) };
    (base * s(0.0, 0.15, t) * s(1.0, 0.8, t) + r.range(-0.03, 0.03)).clamp(0.0, 1.0)
}

/// A hand-drawn path: a cubic curve with a little wobble, sampled like a
/// tablet at 200 Hz for a pen moving at `speed` px/s.
fn pen_path(r: &mut Rng, start: P, len: f32, speed: f32) -> Vec<P> {
    let a = r.range(0.0, std::f32::consts::TAU);
    let b = a + r.range(-1.2, 1.2);
    let p1 = (start.0 + len * 0.35 * a.cos(), start.1 + len * 0.35 * a.sin());
    let p3 = (start.0 + len * b.cos(), start.1 + len * b.sin());
    let p2 = (p3.0 - len * 0.3 * (b + 0.6).cos(), p3.1 - len * 0.3 * (b + 0.6).sin());
    let n = ((len / (speed / 200.0)).ceil() as usize).clamp(2, 4000);
    let wob = r.range(0.0, 1.5);
    (0..=n).map(|i| {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let x = u * u * u * start.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
        let y = u * u * u * start.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
        (x + wob * (t * 40.0).sin(), y + wob * (t * 33.0).cos())
    }).collect()
}

struct Session {
    r:      Rng,
    lib:    Library,
    doc:    SharedDoc,
    pt:     PaintThread,
    size:   (u32, u32),
    t:      f64,
    stats:  BTreeMap<Work, Stat>,
    /// Slowest strokes compared with the pen: (ratio, ms, brush, radius).
    slowest: Vec<(f64, f64, String, f32)>,
    colors: Vec<[u8; 3]>,
}

impl Session {
    /// Wait until the paint thread has done everything sent so far.
    fn sync(&self) -> Duration {
        let (tx, rx) = channel();
        let t0 = Instant::now();
        self.pt.send(Cmd::Ping(tx));
        rx.recv_timeout(Duration::from_secs(120)).expect("paint thread stalled for 2 minutes");
        t0.elapsed()
    }

    fn focus(&mut self) -> P {
        // Painters work an area for a while: mostly near the middle.
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        (w * (0.5 + self.r.range(-0.45, 0.45) * self.r.f()), h * (0.5 + self.r.range(-0.45, 0.45) * self.r.f()))
    }

    fn stroke(&mut self, work: Work, preset: usize, radius: f32, path: &[P], base: f32, erase: bool) {
        let mut brush: Brush = self.lib.presets[preset].settings.clone();
        brush.radius = radius;
        brush.erase = erase;
        brush.color = self.colors[self.r.below(self.colors.len() as u64) as usize];
        brush.opacity = if self.r.chance(0.3) { self.r.range(0.3, 1.0) } else { 1.0 };
        let engine = self.lib.presets[preset].engine.clone();
        let t0 = Instant::now();
        self.pt.send(Cmd::Begin { doc: Arc::clone(&self.doc), brush, engine });
        let n = path.len();
        for (i, &(x, y)) in path.iter().enumerate() {
            self.t += 0.005;
            let p = pressure(&mut self.r, i as f32 / (n - 1).max(1) as f32, base);
            self.pt.send(Cmd::Sample(PenSample::new(x, y, p, self.t)));
        }
        self.pt.send(Cmd::End);
        self.sync();
        let pen = Duration::from_secs_f64(n as f64 / 200.0);
        let busy = t0.elapsed();
        self.stats.entry(work).or_default().add(busy, pen, n);
        let ratio = busy.as_secs_f64() / pen.as_secs_f64();
        self.slowest.push((ratio, busy.as_secs_f64() * 1000.0, self.lib.presets[preset].name.clone(), radius));
        self.slowest.sort_by(|a, b| b.0.total_cmp(&a.0));
        self.slowest.truncate(12);
        self.t += 0.3;
    }

    fn timed(&mut self, work: Work, f: impl FnOnce(&mut Self)) {
        let t0 = Instant::now();
        f(self);
        self.sync();
        self.stats.entry(work).or_default().add(t0.elapsed(), Duration::ZERO, 0);
    }

    fn layer_count(&self) -> usize { self.doc.lock().unwrap().layers.layer_count() }
}

#[test]
#[ignore]
fn painter_session() {
    let secs: u64 = std::env::var("PAINT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let seed: u64 = std::env::var("PAINT_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| { PANICKED.store(true, Ordering::SeqCst); default_hook(info); }));

    let lib = Library::load();
    let pencils = pick(&lib, &["pencil", "charcoal", "2b", "4h", "8b"]);
    let paints = pick(&lib, &["acrylic", "oil", "knife", "bristle", "paint", "watercolor", "brush", "Soft round"]);
    let blends = pick(&lib, &["smudg", "blend", "water"]);
    let inks = pick(&lib, &["ink", "pen"]);
    let erasers = pick(&lib, &["eraser"]);
    let round = pick(&lib, &["round pen"]);
    assert!(!pencils.is_empty() && !paints.is_empty() && !blends.is_empty() && !inks.is_empty() && !round.is_empty());
    println!("brushes: {} pencils, {} paint, {} smudge/blend, {} ink, {} erasers",
        pencils.len(), paints.len(), blends.len(), inks.len(), erasers.len());

    // An A4 page at 300 dpi, like a real illustration.
    let size = (2480u32, 3508u32);
    let doc: SharedDoc = Arc::new(Mutex::new(Document::new(DocId(1), "session", size.0, size.1)));
    let projection = Arc::new(Mutex::new(Projection::new(size.0, size.1)));
    let pt = PaintThread::spawn(Arc::clone(&projection), |_| {});

    // The "GPU": takes the merged pieces as they come, ~120 times a second.
    let stop = Arc::new(AtomicBool::new(false));
    let gpu = {
        let (projection, stop) = (Arc::clone(&projection), Arc::clone(&stop));
        std::thread::spawn(move || {
            let mut bytes = 0u64;
            while !stop.load(Ordering::Relaxed) {
                for (r, rgba) in projection.lock().unwrap().take_patches() {
                    assert_eq!(rgba.len(), (r.2 - r.0) as usize * (r.3 - r.1) as usize * 4);
                    bytes += rgba.len() as u64;
                }
                std::thread::sleep(Duration::from_millis(8));
            }
            bytes
        })
    };

    let mut s = Session {
        r: Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
        lib, doc, pt, size, t: 0.0, stats: BTreeMap::new(), slowest: Vec::new(),
        colors: vec![[20, 20, 24], [180, 60, 50], [230, 200, 160], [60, 90, 150], [240, 240, 235], [90, 140, 80]],
    };
    let (t0, mut next_report, mut next_save) = (Instant::now(), 60u64, 90u64);
    let mut peak_mb = 0u64;
    let mut ops = 0u64;

    // A few layers to start with: sketch, ink, flats, paint.
    for name in ["Sketch", "Ink", "Flats", "Paint"] {
        s.doc.lock().unwrap().layers.add_layer(name);
    }

    while t0.elapsed().as_secs() < secs && !PANICKED.load(Ordering::SeqCst) {
        ops += 1;
        // Work on a random layer, as a painter switches between them.
        if s.r.chance(0.08) {
            let mut d = s.doc.lock().unwrap();
            let ids = d.layers.layer_ids();
            let id = ids[s.r.below(ids.len() as u64) as usize];
            d.layers.set_active(id);
        }
        let start = s.focus();
        match s.r.below(100) {
            // Sketching: small pencils, many quick strokes, light pressure.
            0..=24 => {
                let preset = pencils[s.r.below(pencils.len() as u64) as usize];
                for _ in 0..s.r.below(6) + 1 {
                    let len = s.r.range(40.0, 500.0);
                    let speed = s.r.range(800.0, 3000.0);
                    let at = (start.0 + s.r.range(-60.0, 60.0), start.1 + s.r.range(-60.0, 60.0));
                    let path = pen_path(&mut s.r, at, len, speed);
                    let radius = s.r.range(1.0, 6.0);
                    let base = s.r.range(0.25, 0.7);
                    s.stroke(Work::Sketch, preset, radius, &path, base, false);
                }
            }
            // Painting: big brushes, long strokes.
            25..=44 => {
                let preset = paints[s.r.below(paints.len() as u64) as usize];
                let radius = if s.r.chance(0.2) { s.r.range(250.0, 500.0) } else { s.r.range(30.0, 250.0) };
                let (len, speed) = (s.r.range(200.0, 2500.0), s.r.range(500.0, 2000.0));
                let path = pen_path(&mut s.r, start, len, speed);
                let base = s.r.range(0.5, 1.0);
                s.stroke(Work::Paint, preset, radius, &path, base, false);
            }
            // Smudge / blend over what is there.
            45..=56 => {
                let preset = blends[s.r.below(blends.len() as u64) as usize];
                let radius = s.r.range(20.0, 300.0);
                let (len, speed) = (s.r.range(100.0, 1200.0), s.r.range(300.0, 1200.0));
                let path = pen_path(&mut s.r, start, len, speed);
                let base = s.r.range(0.4, 1.0);
                s.stroke(Work::Blend, preset, radius, &path, base, false);
            }
            // Inking: thin lines, careful (slow), tapered.
            57..=68 => {
                let preset = if s.r.chance(0.5) { inks[s.r.below(inks.len() as u64) as usize] } else { round[0] };
                let (len, speed) = (s.r.range(100.0, 1500.0), s.r.range(300.0, 1200.0));
                let path = pen_path(&mut s.r, start, len, speed);
                let radius = s.r.range(0.5, 4.0);
                let base = s.r.range(0.6, 1.0);
                s.stroke(Work::Ink, preset, radius, &path, base, false);
            }
            // Erasing (the eraser tool: the brush, erasing).
            69..=72 => {
                let preset = if !erasers.is_empty() && s.r.chance(0.5) { erasers[s.r.below(erasers.len() as u64) as usize] } else { round[0] };
                let (len, speed) = (s.r.range(50.0, 800.0), s.r.range(500.0, 2000.0));
                let path = pen_path(&mut s.r, start, len, speed);
                let radius = s.r.range(3.0, 80.0);
                s.stroke(Work::Erase, preset, radius, &path, 1.0, true);
            }
            // Geometry: closed arc outlines and ellipses, as the app sends them.
            73..=76 => {
                let preset = round[0];
                let mut path = Vec::new();
                let first = start;
                let mut a = first;
                let n = s.r.below(5) + 2;
                for i in 0..n {
                    let b = if i + 1 == n { first } else { (a.0 + s.r.range(-300.0, 300.0), a.1 + s.r.range(-300.0, 300.0)) };
                    let c = (a.0 + s.r.range(-250.0, 250.0), a.1 + s.r.range(-250.0, 250.0));
                    let arc = crate::shapes::arc(a, b, c);
                    path.extend(arc.into_iter().skip(usize::from(i > 0)));
                    a = b;
                }
                if s.r.chance(0.4) { path = crate::shapes::ellipse(start, (start.0 + s.r.range(20.0, 900.0), start.1 + s.r.range(20.0, 900.0))); }
                let radius = s.r.range(0.5, 3.0);
                s.stroke(Work::Shape, preset, radius, &path, 1.0, false);
            }
            // Flatting: fill closed areas on the flats layer.
            77..=81 => {
                let color = s.colors[s.r.below(s.colors.len() as u64) as usize];
                let params = FillParams { tolerance: s.r.below(60) as u8, merged: s.r.chance(0.8), expand: s.r.below(3) as u32 };
                let (x, y) = (start.0.clamp(0.0, size.0 as f32 - 1.0) as u32, start.1.clamp(0.0, size.1 as f32 - 1.0) as u32);
                s.timed(Work::Fill, |s| s.pt.send(Cmd::Fill { doc: Arc::clone(&s.doc), x, y, color: [color[0], color[1], color[2], 255], params }));
            }
            // Lasso a part, paint inside it (clipped), sometimes keep it.
            82..=85 => {
                let pts: Vec<P> = (0..s.r.below(80) + 8).map(|i| {
                    let a = i as f32 * 0.4;
                    let rr = s.r.range(150.0, 600.0);
                    (start.0 + rr * a.cos(), start.1 + rr * a.sin())
                }).collect();
                let sel = Selection::polygon(size.0, size.1, &pts);
                s.timed(Work::Lasso, |s| s.doc.lock().unwrap().selection = sel);
                let preset = paints[s.r.below(paints.len() as u64) as usize];
                let path = pen_path(&mut s.r, start, 900.0, 1500.0);
                let radius = s.r.range(40.0, 200.0);
                s.stroke(Work::Paint, preset, radius, &path, 0.9, false);
                if s.r.chance(0.6) { s.doc.lock().unwrap().selection = None; }
            }
            // Copy the lasso (or a layer) and paste it a bit further.
            86..=88 => {
                let pts: Vec<P> = (0..24).map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 24.0;
                    (start.0 + 300.0 * a.cos(), start.1 + 220.0 * a.sin())
                }).collect();
                let (dx, dy) = (s.r.range(-400.0, 400.0) as i64, s.r.range(-400.0, 400.0) as i64);
                s.timed(Work::Paste, |s| {
                    let clip = {
                        let mut d = s.doc.lock().unwrap();
                        d.selection = Selection::polygon(size.0, size.1, &pts);
                        let c = d.copy();
                        d.selection = None;
                        c
                    };
                    if let Some(mut clip) = clip {
                        clip.x += dx;
                        clip.y += dy;
                        s.pt.send(Cmd::Paste { doc: Arc::clone(&s.doc), clip });
                    }
                });
            }
            // Layers: new, duplicate, move, hide, opacity, delete.
            89..=93 => {
                let k = s.r.below(6);
                let many = s.layer_count() > 30;
                let pick_opacity = s.r.range(0.2, 1.0);
                let up = s.r.chance(0.5);
                s.timed(Work::Layers, |s| {
                    let mut d = s.doc.lock().unwrap();
                    match k {
                        0 if !many => { let id = d.layers.add_layer("Layer"); d.layers.set_active(id); }
                        1 if !many => { d.layers.duplicate_active(); }
                        2 => d.layers.move_active(up),
                        3 => { let n = d.layers.active_node_mut(); *n.visible_mut() ^= true; }
                        4 => *d.layers.active_node_mut().opacity_mut() = pick_opacity,
                        _ => if d.layers.layer_count() > 6 { d.layers.remove_active(); },
                    }
                    // Changes like these merge the whole picture again.
                    merge_all_in_bands(&d.layers);
                });
            }
            // Undo a few steps, redo some of them.
            94..=97 => {
                let (u, rdo) = (s.r.below(10) + 1, s.r.below(5));
                s.timed(Work::Undo, |s| {
                    for _ in 0..u { s.pt.send(Cmd::Undo(Arc::clone(&s.doc))); }
                    for _ in 0..rdo { s.pt.send(Cmd::Redo(Arc::clone(&s.doc))); }
                });
            }
            // A huge soft brush (1000 px) sweeping a background.
            _ => {
                let preset = pick(&s.lib, &["soft round", "airbrush"]).first().copied().unwrap_or(round[0]);
                let (len, speed) = (s.r.range(500.0, 3000.0), s.r.range(800.0, 2500.0));
                let path = pen_path(&mut s.r, start, len, speed);
                s.stroke(Work::Paint, preset, 500.0, &path, 1.0, false);
            }
        }

        // Save (to memory) and open it again now and then: identical.
        if t0.elapsed().as_secs() >= next_save {
            next_save += 90;
            s.timed(Work::Save, |s| {
                let d = s.doc.lock().unwrap();
                let mut buf = std::io::Cursor::new(Vec::new());
                aurastudio_file::write(&aurastudio_file::snapshot(&d, None), &mut buf).unwrap();
                buf.set_position(0);
                let back = aurastudio_file::read(buf, DocId(2), "back", 1 << 15).expect("saved file must open").doc;
                for id in d.layers.layer_ids() {
                    let (Some(Node::Layer(a)), Some(Node::Layer(b))) = (d.layers.node(id), back.layers.node(id)) else { panic!("layer {id} lost") };
                    assert!(a.tiles == b.tiles, "layer {id} differs after save / open");
                }
            });
        }
        peak_mb = peak_mb.max(rss_mb());
        if t0.elapsed().as_secs() >= next_report {
            next_report += 60;
            let d = s.doc.lock().unwrap();
            // Pixels the layers hold (painted tiles only): what the RAM
            // should mostly be.
            let tiles: usize = d.layers.layer_ids().iter().filter_map(|&id| match d.layers.node(id) {
                Some(Node::Layer(l)) => Some(l.tiles.bytes()),
                _ => None,
            }).sum();
            println!("{:>5} s: {ops} actions, {} layers ({} MB of tiles), {} undo steps ({} MB), RAM {} MB (peak {peak_mb})",
                t0.elapsed().as_secs(), d.layers.layer_count(), tiles >> 20, d.history().undo_len(), d.history().bytes() >> 20, rss_mb());
        }
    }

    s.sync();
    stop.store(true, Ordering::Relaxed);
    let uploaded = gpu.join().expect("gpu thread");
    println!("\n{} s, {ops} actions, {} MB merged and uploaded, peak RAM {peak_mb} MB", t0.elapsed().as_secs(), uploaded >> 20);
    println!("{:<8} {:>6} {:>9} {:>11} {:>11} {:>10} {:>9}", "work", "count", "samples", "avg ms", "worst ms", "worst x", "lagging");
    for (w, st) in &s.stats {
        let avg = st.busy.as_secs_f64() * 1000.0 / st.n.max(1) as f64;
        println!("{:<8} {:>6} {:>9} {:>11.1} {:>11.1} {:>10.2} {:>9}", format!("{w:?}"), st.n, st.samples, avg,
            st.worst.as_secs_f64() * 1000.0, st.worst_ratio, st.lagging);
    }
    println!("\nslowest strokes (paint time / pen time):");
    for (ratio, ms, name, radius) in &s.slowest {
        println!("  {ratio:>6.2}x  {ms:>8.0} ms  {name} ({:.0} px)", radius * 2.0);
    }
    assert!(!PANICKED.load(Ordering::SeqCst), "a panic happened (see above)");
}
