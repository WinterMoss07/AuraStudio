//! Reference renders: every catalog brush paints the same strokes (slow
//! S with a pressure ramp, a fast line, tilted strokes, a stroke over
//! paint), and each result's hash is compared with reference.txt. Any
//! Forge change that alters a single pixel shows here, by brush.
//!
//! The reference records how Forge paints today (not how they should:
//! fidelity to MyPaint / Krita is checked against their own renders).
//! After an intended change: UPDATE_REFERENCE=1 cargo test -p
//! aurastudio-forge-library --test reference, and say why in the commit.
//! Linux only: other platforms' math libraries may round differently.

use aurastudio_forge::{Brush, Forge, PenSample};
use aurastudio_forge_library::Library;
use aurastudio_core::FlatSurface;

const W: u32 = 260;
const H: u32 = 140;

fn strokes() -> Vec<Vec<PenSample>> {
    use std::f32::consts::{PI, TAU};
    vec![
        // Slow S, pressure up then down.
        (0..=120).map(|i| {
            let t = i as f32 / 120.0;
            PenSample::new(20.0 + t * 220.0, 40.0 - 25.0 * (t * TAU).sin(), (t * PI).sin().max(0.05), i as f64 * 0.008)
        }).collect(),
        // Fast line, full pressure.
        (0..=12).map(|i| PenSample::new(20.0 + i as f32 * 18.0, 80.0, 1.0, i as f64 * 0.004)).collect(),
        // Tilted, the tilt turning.
        (0..=80).map(|i| {
            let t = i as f32 / 80.0;
            let a = t * TAU;
            PenSample { tilt: (0.8 * a.cos(), 0.8 * a.sin()), ..PenSample::new(30.0 + t * 200.0, 115.0, 0.7, i as f64 * 0.008) }
        }).collect(),
    ]
}

fn render(p: &aurastudio_forge_library::Preset) -> u64 {
    Forge::reset_random();
    let mut s = FlatSurface::new(W, H, [255, 255, 255, 0]);
    // A band of paint, for smudge and eraser brushes.
    for y in 95..135 {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            s.pixels[i..i + 4].copy_from_slice(&[(x % 200) as u8, 120, 200 - (x % 200) as u8, 255]);
        }
    }
    s.next_stroke();
    let settings = Brush { color: [30, 60, 140], radius: p.settings.radius.min(12.0), ..p.settings.clone() };
    for stroke in strokes() {
        let mut e = p.start();
        e.sync_from(&settings);
        e.begin_stroke();
        for sample in stroke { e.stroke_to(&mut s, sample); }
        e.end_stroke(&mut s);
        s.next_stroke();
    }
    // FNV-1a.
    s.pixels.iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[test]
#[cfg_attr(not(target_os = "linux"), ignore)]
fn every_brush_paints_as_recorded() {
    let lib = Library::load_bundled();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reference.txt");
    let now: Vec<(String, u64)> = lib.presets.iter().map(|p| (format!("{} / {}", p.medium, p.name), render(p))).collect();
    if std::env::var("UPDATE_REFERENCE").is_ok() || !path.exists() {
        let text: String = now.iter().map(|(n, h)| format!("{n}\t{h:016x}\n")).collect();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let text = std::fs::read_to_string(&path).unwrap();
    let recorded: std::collections::HashMap<&str, &str> = text.lines().filter_map(|l| l.split_once('\t')).collect();
    let changed: Vec<&str> = now.iter()
        .filter(|(n, h)| recorded.get(n.as_str()) != Some(&format!("{h:016x}").as_str()))
        .map(|(n, _)| n.as_str()).collect();
    assert!(changed.is_empty(), "{} brush(es) paint differently: {changed:?}", changed.len());
}
