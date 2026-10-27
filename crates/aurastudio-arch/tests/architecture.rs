//! The crates are bricks: each one may depend only on the crates listed
//! here, and the data crates know nothing about windows, the GPU or the
//! interface. A new dependency between crates is a decision: change the
//! table below on purpose, in the commit that adds it.

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

/// crate → the crates of the workspace it may depend on.
const ALLOWED: &[(&str, &[&str])] = &[
    ("core", &[]),
    ("color", &[]),
    ("filters", &[]),
    ("input", &[]),
    // Only the rules about the others.
    ("arch", &[]),
    ("raster", &["core"]),
    ("transform", &["core"]),
    ("forge", &["core"]),
    // The tools apart from the window (shapes, arc, transform box).
    ("tools", &["core", "transform"]),
    ("forge-library", &["core", "forge"]),
    ("selection", &["core", "raster"]),
    ("layers", &["core", "filters", "raster", "selection"]),
    ("animation", &["core", "layers"]),
    ("document", &["core", "filters", "layers", "raster", "selection", "transform"]),
    ("file", &["core", "document", "filters", "layers", "raster"]),
    ("gpu", &["core", "animation"]),
    // The interface reads the layers and changes them through `document`
    // (Edit); pixels, selection and transform types come through it too.
    ("ui", &["core", "document", "filters", "forge", "forge-library", "layers", "tools"]),
    // The application puts everything together.
    ("aurastudio", &["animation", "color", "core", "document", "file", "filters", "forge", "forge-library", "gpu", "input", "layers", "selection", "tools", "transform", "ui"]),
];

/// Crates that never see a window, the GPU or the interface.
const DATA: &[&str] = &["core", "raster", "selection", "transform", "forge", "forge-library", "layers", "animation", "document", "file", "filters", "color", "tools"];
const FORBIDDEN_IN_DATA: &[&str] = &["wgpu", "egui", "egui-wgpu", "egui_dock", "winit", "eframe"];

fn short(name: &str) -> &str { name.strip_prefix("aurastudio-").unwrap_or(name) }

fn metadata() -> serde_json::Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo metadata");
    assert!(out.status.success(), "cargo metadata failed");
    serde_json::from_slice(&out.stdout).expect("cargo metadata output")
}

/// (crate → workspace crates it depends on, crate → every dependency name),
/// without the dev-dependencies.
fn graph() -> (BTreeMap<String, BTreeSet<String>>, BTreeMap<String, BTreeSet<String>>) {
    let m = metadata();
    let (mut inner, mut all) = (BTreeMap::new(), BTreeMap::new());
    for p in m["packages"].as_array().unwrap() {
        let name = short(p["name"].as_str().unwrap()).to_owned();
        let (mut i, mut a) = (BTreeSet::new(), BTreeSet::new());
        for d in p["dependencies"].as_array().unwrap() {
            if d["kind"].as_str() == Some("dev") { continue; }
            let dn = d["name"].as_str().unwrap();
            a.insert(dn.to_owned());
            if dn.starts_with("aurastudio-") { i.insert(short(dn).to_owned()); }
        }
        inner.insert(name.clone(), i);
        all.insert(name, a);
    }
    (inner, all)
}

#[test]
fn every_crate_depends_only_on_what_the_table_allows() {
    let (inner, _) = graph();
    let allowed: BTreeMap<&str, BTreeSet<&str>> = ALLOWED.iter().map(|(k, v)| (*k, v.iter().copied().collect())).collect();
    let mut errors = Vec::new();
    for (name, deps) in &inner {
        let Some(ok) = allowed.get(name.as_str()) else {
            errors.push(format!("crate `{name}` is not in the table of tests/architecture.rs"));
            continue;
        };
        for d in deps {
            if !ok.contains(d.as_str()) { errors.push(format!("`{name}` must not depend on `{d}`")); }
        }
    }
    for name in allowed.keys() {
        if !inner.contains_key(*name) { errors.push(format!("`{name}` is in the table but not in the workspace")); }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn data_crates_know_nothing_about_windows_or_the_gpu() {
    let (_, all) = graph();
    let mut errors = Vec::new();
    for name in DATA {
        let Some(deps) = all.get(*name) else { continue };
        for f in FORBIDDEN_IN_DATA {
            if deps.contains(*f) { errors.push(format!("`{name}` depends on `{f}`")); }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// The GPU crate draws the canvas into any window: the window system and
/// the interface meet it in the application (window.rs).
#[test]
fn the_gpu_crate_knows_no_window_system_or_interface() {
    let (_, all) = graph();
    let deps = &all["gpu"];
    let bad: Vec<&str> = ["egui", "egui-wgpu", "egui-winit", "egui_dock", "winit", "eframe"].into_iter().filter(|f| deps.contains(*f)).collect();
    assert!(bad.is_empty(), "`gpu` depends on {bad:?}");
}

/// Crates that may handle single tiles (their bytes, their place in the
/// table). The others go through areas and rows (`read_rect`,
/// `write_rect`, `for_row`, `pixel`, `pack`...), so the way tiles are
/// stored can change without them.
const TILE_LEVEL: &[&str] = &["raster", "selection", "layers", "arch"];
const TILE_API: &[&str] = &["TILE_BYTES", "new_tile", ".tile(", ".tile_mut(", "set_tile(", "drop_if_fill(", ".cols()", "raster::Tile;", "raster::Tile,", " Tile}", "{Tile,"];

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() { rust_files(&p, out); } else if p.extension().is_some_and(|x| x == "rs") { out.push(p); }
    }
}

#[test]
fn only_the_raster_side_handles_single_tiles() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut errors = Vec::new();
    for e in std::fs::read_dir(crates).unwrap().flatten() {
        let dir = e.path();
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        if TILE_LEVEL.contains(&short(&name)) { continue; }
        let mut files = Vec::new();
        rust_files(&dir.join("src"), &mut files);
        rust_files(&dir.join("tests"), &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            for (n, line) in text.lines().enumerate() {
                if line.trim_start().starts_with("//") { continue; }
                for t in TILE_API {
                    if line.contains(t) { errors.push(format!("{}:{}: `{t}`", f.display(), n + 1)); }
                }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// The interface changes the layers only with `Edit`s (document/edit.rs):
/// no mutable access to the stack or its nodes.
#[test]
fn the_interface_changes_layers_only_with_edits() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("aurastudio-ui");
    let mut files = Vec::new();
    rust_files(&ui.join("src"), &mut files);
    let mut errors = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            // (Test modules build their stacks as they like.)
            if line.trim_start().starts_with("#[cfg(test)]") && lines.get(n + 1).is_some_and(|l| l.trim_start().starts_with("mod ")) { break; }
            if line.trim_start().starts_with("//") { continue; }
            for t in ["&mut LayerStack", "node_mut(", "name_mut()", "visible_mut()", "locked_mut()", "opacity_mut()", "props_mut()", "Editor::new("] {
                if line.contains(t) { errors.push(format!("{}:{}: `{t}`", f.display(), n + 1)); }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}
