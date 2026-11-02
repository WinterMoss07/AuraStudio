//! The strokelog: every stroke recorded (pure Rust, no Windows code), so
//! strokes can be replayed (benchmarks, pixel-exact tests, the
//! diagnostic), attached to bug reports and, later, painted again.
//!
//! A session file: header (magic, version, the app's version, the global
//! pressure curve), then strokes, each LZ4 on its own: the app's opaque
//! context blob (view, brush, tool, layer: `input` never reads it) and the
//! events. Pressure is kept before the global curve. Written at stroke
//! end on a thread of its own, never on the pen's path. Oldest sessions
//! are removed past a size budget.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

use aurastudio_core::{ByteReader, ByteWriter, Micros, TransferCurve};

use crate::event::{Device, Event, Kind, Source, Target};

const MAGIC: [u8; 4] = *b"AURL";
const VERSION: u16 = 1;
/// Sessions kept, in total bytes.
pub const BUDGET: u64 = 50 << 20;

/// One stroke as recorded: the app's context and the events from down to
/// up (with the hovers and buttons between).
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub context: Vec<u8>,
    pub events:  Vec<Event>,
}

/// A session read back.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub app_version: String,
    pub curve:       TransferCurve,
    pub strokes:     Vec<Stroke>,
}

fn device_code(d: Device) -> (u8, u32) {
    match d { Device::Pen => (0, 0), Device::Eraser => (1, 0), Device::Mouse => (2, 0), Device::Touch(id) => (3, id) }
}

fn device_of(c: u8, id: u32) -> Option<Device> {
    Some(match c { 0 => Device::Pen, 1 => Device::Eraser, 2 => Device::Mouse, 3 => Device::Touch(id), _ => return None })
}

fn kind_code(k: Kind) -> (u8, u8) {
    match k {
        Kind::Down => (0, 0), Kind::Move => (1, 0), Kind::Up { cancelled } => (2, cancelled as u8), Kind::Hover => (3, 0),
        Kind::ButtonDown(b) => (4, b), Kind::ButtonUp(b) => (5, b), Kind::ProximityIn => (6, 0), Kind::ProximityOut => (7, 0),
        Kind::Tap(n) => (8, n),
        // (Wheel and gestures are not part of a stroke.)
        Kind::Wheel { .. } | Kind::Gesture(_) => (255, 0),
    }
}

fn kind_of(c: u8, a: u8) -> Option<Kind> {
    Some(match c {
        0 => Kind::Down, 1 => Kind::Move, 2 => Kind::Up { cancelled: a != 0 }, 3 => Kind::Hover,
        4 => Kind::ButtonDown(a), 5 => Kind::ButtonUp(a), 6 => Kind::ProximityIn, 7 => Kind::ProximityOut, 8 => Kind::Tap(a),
        _ => return None,
    })
}

/// A stroke's bytes (before LZ4): context, count, then per event its kind,
/// device, target, position (f32), raw pressure (u16), tilt (0.01°),
/// rotation (0.01°) and time (delta, µs).
fn encode_stroke(s: &Stroke) -> Vec<u8> {
    let mut w = ByteWriter::new();
    w.blob(&s.context);
    let events: Vec<&Event> = s.events.iter().filter(|e| kind_code(e.kind).0 != 255).collect();
    w.varint(events.len() as u64);
    let mut t = 0u64;
    for e in events {
        let (k, a) = kind_code(e.kind);
        let (d, id) = device_code(e.device);
        w.u8(k).u8(a).u8(d).varint(id as u64).u8((e.target == Target::Canvas) as u8);
        w.f32(e.x as f32).f32(e.y as f32).u16((e.pressure_raw.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16);
        w.svarint((e.tilt.0 * 100.0) as i64).svarint((e.tilt.1 * 100.0) as i64).varint((e.rotation * 100.0) as u64);
        w.varint(e.time.0.saturating_sub(t));
        t = e.time.0;
    }
    w.finish()
}

fn decode_stroke(b: &[u8], curve: &TransferCurve) -> Option<Stroke> {
    let mut r = ByteReader::new(b);
    let context = r.blob(1 << 20)?.to_vec();
    let n = r.varint()? as usize;
    if n > r.remaining() { return None; }
    let mut events = Vec::with_capacity(n);
    let mut t = 0u64;
    for _ in 0..n {
        let (k, a, d) = (r.u8()?, r.u8()?, r.u8()?);
        let id = u32::try_from(r.varint()?).ok()?;
        let target = if r.u8()? != 0 { Target::Canvas } else { Target::Ui };
        let (x, y) = (r.f32()? as f64, r.f32()? as f64);
        let pressure_raw = r.u16()? as f32 / 65535.0;
        let tilt = (r.svarint()? as f32 / 100.0, r.svarint()? as f32 / 100.0);
        let rotation = r.varint()? as f32 / 100.0;
        t += r.varint()?;
        let device = device_of(d, id)?;
        let pressure = if device == Device::Mouse { 1.0 } else { curve.eval(pressure_raw) };
        events.push(Event { kind: kind_of(k, a)?, device, source: Source::Strokelog, target, x, y, pressure, pressure_raw, tilt, rotation, time: Micros(t) });
    }
    r.at_end().then_some(Stroke { context, events })
}

/// A session's header.
fn header(app_version: &str, curve: &TransferCurve) -> Vec<u8> {
    let mut w = ByteWriter::with_header(&MAGIC, VERSION);
    w.str(app_version);
    w.varint(curve.points().len() as u64);
    for (x, y) in curve.points() { w.f32(*x).f32(*y); }
    w.finish()
}

/// One stroke's record in a session file.
fn record(s: &Stroke) -> Vec<u8> {
    let mut w = ByteWriter::new();
    w.blob(&lz4_flex::compress_prepend_size(&encode_stroke(s)));
    w.finish()
}

/// Read a session file (or an exported stroke). A stroke cut by a crash at
/// the end of the file is left out; damage before it fails the whole.
pub fn read(b: &[u8]) -> Option<Session> {
    let mut r = ByteReader::new(b);
    if r.header(&MAGIC)? > VERSION { return None; }
    let app_version = r.str(256)?.to_owned();
    let n = r.varint()? as usize;
    if n > 1024 { return None; }
    let mut pts = Vec::with_capacity(n);
    for _ in 0..n { pts.push((r.f32()?, r.f32()?)); }
    let curve = TransferCurve::new(&pts);
    let mut strokes = Vec::new();
    while !r.at_end() {
        let Some(blob) = r.blob(64 << 20) else { break };
        let raw = lz4_flex::decompress_size_prepended(blob).ok()?;
        strokes.push(decode_stroke(&raw, &curve)?);
    }
    Some(Session { app_version, curve, strokes })
}

/// One stroke as a file of its own ("send this stroke").
pub fn export(app_version: &str, curve: &TransferCurve, stroke: &Stroke) -> Vec<u8> {
    let mut b = header(app_version, curve);
    b.extend_from_slice(&record(stroke));
    b
}

/// The events of a stroke, `speed` times as fast as they were drawn (0:
/// at once), from `start`: what a replay feeds to the paint thread.
pub fn replay(stroke: &Stroke, start: Micros, speed: f64) -> Vec<Event> {
    let t0 = stroke.events.first().map_or(0, |e| e.time.0);
    stroke.events.iter().map(|e| {
        let dt = e.time.0 - t0;
        let t = if speed <= 0.0 { 0 } else { (dt as f64 / speed) as u64 };
        Event { time: Micros(start.0 + t), source: Source::Strokelog, ..*e }
    }).collect()
}

/// The writer: strokes go to its thread, written at stroke end.
pub struct Writer {
    tx: Option<Sender<Stroke>>,
    pub path: PathBuf,
}

impl Writer {
    /// A new session file in `dir` (made if needed); older sessions are
    /// removed past `budget` bytes. `None` if the folder cannot be used.
    pub fn open(dir: &Path, app_version: &str, curve: &TransferCurve, budget: u64) -> Option<Writer> {
        std::fs::create_dir_all(dir).ok()?;
        prune(dir, budget);
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let path = dir.join(format!("session-{stamp}.aurl"));
        std::fs::write(&path, header(app_version, curve)).ok()?;
        let (tx, rx) = channel::<Stroke>();
        let p = path.clone();
        std::thread::Builder::new().name("strokelog".into()).spawn(move || {
            use std::io::Write;
            let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&p) else { return };
            for s in rx { let _ = f.write_all(&record(&s)); }
        }).ok()?;
        Some(Writer { tx: Some(tx), path })
    }

    /// Record a finished stroke (never blocks).
    pub fn push(&self, s: Stroke) {
        if let Some(tx) = &self.tx { let _ = tx.send(s); }
    }
}

/// Remove the oldest `.aurl` files until the folder holds at most `budget`.
fn prune(dir: &Path, budget: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "aurl"))
        .filter_map(|e| { let m = e.metadata().ok()?; Some((m.modified().ok()?, m.len(), e.path())) })
        .collect();
    files.sort();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    for (_, len, p) in files {
        if total <= budget { break; }
        if std::fs::remove_file(&p).is_ok() { total -= len; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: Kind, x: f64, t: u64) -> Event {
        Event { kind, device: Device::Pen, source: Source::Live, target: Target::Canvas, x, y: 2.5, pressure: 0.25, pressure_raw: 0.5, tilt: (12.5, -40.0), rotation: 90.0, time: Micros(t) }
    }

    fn stroke() -> Stroke {
        Stroke { context: b"brush=round".to_vec(), events: vec![ev(Kind::Down, 1.0, 1000), ev(Kind::Move, 1.5, 6000), ev(Kind::ButtonDown(0), 1.5, 6100), ev(Kind::Up { cancelled: false }, 2.0, 11000)] }
    }

    #[test]
    fn a_stroke_round_trips() {
        let curve = TransferCurve::new(&[(0.0, 0.0), (0.5, 0.3), (1.0, 1.0)]);
        let b = export("0.20.0", &curve, &stroke());
        let s = read(&b).expect("session");
        assert_eq!(s.app_version, "0.20.0");
        let got = &s.strokes[0];
        assert_eq!(got.context, b"brush=round");
        assert_eq!(got.events.len(), 4);
        let e = got.events[1];
        assert_eq!((e.kind, e.time, e.x, e.tilt, e.rotation, e.source), (Kind::Move, Micros(6000), 1.5, (12.5, -40.0), 90.0, Source::Strokelog));
        assert!((e.pressure_raw - 0.5).abs() < 1e-4 && (e.pressure - 0.3).abs() < 0.01, "the curve of the session applies");
    }

    #[test]
    fn a_cut_file_keeps_its_whole_strokes() {
        let curve = TransferCurve::linear();
        let mut b = header("x", &curve);
        b.extend_from_slice(&record(&stroke()));
        let one = b.len();
        b.extend_from_slice(&record(&stroke()));
        assert_eq!(read(&b).map(|s| s.strokes.len()), Some(2));
        assert_eq!(read(&b[..one + 5]).map(|s| s.strokes.len()), Some(1), "the stroke cut by a crash is left out");
        assert!(read(b"nope").is_none());
    }

    #[test]
    fn replay_keeps_or_squeezes_the_timing() {
        let s = stroke();
        let at_speed = replay(&s, Micros(500), 1.0);
        assert_eq!(at_speed.iter().map(|e| e.time.0).collect::<Vec<_>>(), vec![500, 5500, 5600, 10500]);
        assert!(replay(&s, Micros(0), 0.0).iter().all(|e| e.time.0 == 0));
    }

    #[test]
    fn the_writer_appends_and_old_sessions_go() {
        let dir = std::env::temp_dir().join(format!("aurl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let curve = TransferCurve::linear();
        let w = Writer::open(&dir, "t", &curve, BUDGET).expect("writer");
        w.push(stroke());
        w.push(stroke());
        let path = w.path.clone();
        drop(w);
        // (The writer thread ends when its channel closes.)
        let mut n = 0;
        for _ in 0..200 {
            n = std::fs::read(&path).ok().and_then(|b| read(&b)).map_or(0, |s| s.strokes.len());
            if n == 2 { break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(n, 2);
        prune(&dir, 0);
        assert_eq!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(9), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
