//! The crates are bricks: each one does one job and may depend only on
//! the crates listed here; the data crates know nothing about windows,
//! the GPU or the interface. A new dependency between crates is a
//! decision: change the table below on purpose, in the commit that adds
//! it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// crate → the crates of the workspace it may depend on (normal and build
/// dependencies; dev-dependencies are free).
const ALLOWED: &[(&str, &[&str])] = &[
    // Shared vocabulary and small pure helpers: depends on nothing at all.
    ("core", &[]),
    // Only the rules about the others.
    ("arch", &[]),
    // The only door for everything that points.
    ("input", &["core"]),
    // Pixels of one surface: tiles, forms, stroke writing, undo patches.
    ("raster", &["core"]),
    ("filters", &["core", "raster"]),
    ("transform", &["core"]),
    ("tools", &["core", "transform"]),
    ("selection", &["core", "raster", "filters"]),
    ("color", &["core"]),
    // What the document tree IS (a filter layer holds a `Filter`).
    ("layers", &["core", "raster", "filters"]),
    // The merged image of a rect, stroke and folder caches.
    ("compose", &["core", "raster", "layers", "filters"]),
    // What time DOES to the tree; the camera.
    ("animation", &["core", "layers"]),
    ("document", &["core", "raster", "layers", "compose", "animation", "selection", "filters", "transform", "color"]),
    // Reads and writes a neutral snapshot, not the document.
    ("file", &["core", "raster", "layers", "filters"]),
    ("gpu", &["core", "raster"]),
    ("forge", &["core"]),
    ("forge-library", &["core", "forge"]),
    ("ui", &["core", "document", "filters", "forge", "forge-library", "layers", "tools", "animation"]),
    // Diagnostic builds only (the app depends on it optionally).
    ("diag", &["core"]),
    // Fuzzers and diagnostic scenarios: dev only, nothing depends on it.
    ("stress", &["core", "diag", "input", "raster", "filters", "transform", "tools", "selection", "color", "layers", "compose", "animation", "document", "file", "gpu", "forge", "forge-library"]),
    // The application puts everything together.
    ("aurastudio", &["core", "diag", "input", "raster", "filters", "transform", "tools", "selection", "color", "layers", "compose", "animation", "document", "file", "gpu", "forge", "forge-library", "ui"]),
];

/// Crates that never see a window, the GPU or the interface.
const DATA: &[&str] = &["core", "input", "raster", "filters", "transform", "tools", "selection", "color", "layers", "compose", "animation", "document", "file", "forge", "forge-library", "diag"];
const FORBIDDEN_IN_DATA: &[&str] = &["wgpu", "egui", "egui-wgpu", "egui-winit", "egui_dock", "winit", "eframe"];

/// No file may hold more code than this outside its test modules. Big
/// files are split by job; this keeps them from coming back.
const MAX_LINES: usize = 1500;
/// Files allowed above the limit for now, with their size when listed:
/// the list can only shrink (each entry names the step that splits it).
const TOO_BIG: &[(&str, usize)] = &[];

/// Crates where `.unwrap()` / `.expect(` are not allowed outside tests
/// (a data crate returns an error or an `Option` instead).
const NO_UNWRAP: &[&str] = &["core", "raster", "filters", "transform", "tools", "selection", "color", "layers", "compose", "animation", "document", "file"];

fn short(name: &str) -> &str { name.strip_prefix("aurastudio-").unwrap_or(name) }

fn crates_dir() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf() }

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

struct Dep { name: String, optional: bool }

/// crate → its dependencies (without the dev-dependencies).
fn graph() -> BTreeMap<String, Vec<Dep>> {
    let m = metadata();
    let mut g = BTreeMap::new();
    for p in m["packages"].as_array().unwrap() {
        let name = short(p["name"].as_str().unwrap()).to_owned();
        let deps = p["dependencies"].as_array().unwrap().iter()
            .filter(|d| d["kind"].as_str() != Some("dev"))
            .map(|d| Dep { name: d["name"].as_str().unwrap().to_owned(), optional: d["optional"].as_bool() == Some(true) })
            .collect();
        g.insert(name, deps);
    }
    g
}

#[test]
fn every_crate_depends_only_on_what_the_table_allows() {
    let g = graph();
    let allowed: BTreeMap<&str, BTreeSet<&str>> = ALLOWED.iter().map(|(k, v)| (*k, v.iter().copied().collect())).collect();
    let mut errors = Vec::new();
    for (name, deps) in &g {
        let Some(ok) = allowed.get(name.as_str()) else {
            errors.push(format!("crate `{name}` is not in the table of tests/architecture.rs"));
            continue;
        };
        for d in deps.iter().filter(|d| d.name.starts_with("aurastudio-")) {
            let d = short(&d.name);
            if !ok.contains(d) { errors.push(format!("`{name}` must not depend on `{d}`")); }
        }
    }
    // (While `rework/foundations` rebuilds the crates one by one, crates of
    // the table may be missing from the workspace.)
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn core_has_no_dependency_at_all() {
    let g = graph();
    let deps: Vec<&str> = g["core"].iter().map(|d| d.name.as_str()).collect();
    assert!(deps.is_empty(), "`core` depends on {deps:?}");
}

#[test]
fn data_crates_know_nothing_about_windows_or_the_gpu() {
    let g = graph();
    let mut errors = Vec::new();
    for name in DATA {
        let Some(deps) = g.get(*name) else { continue };
        for d in deps {
            if FORBIDDEN_IN_DATA.contains(&d.name.as_str()) { errors.push(format!("`{name}` depends on `{}`", d.name)); }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// The GPU crate draws into any window: the window system and the
/// interface meet it in the application.
#[test]
fn the_gpu_crate_knows_no_window_system_or_interface() {
    let g = graph();
    let Some(deps) = g.get("gpu") else { return };
    let bad: Vec<&str> = deps.iter().map(|d| d.name.as_str()).filter(|n| ["egui", "egui-wgpu", "egui-winit", "egui_dock", "winit", "eframe"].contains(n)).collect();
    assert!(bad.is_empty(), "`gpu` depends on {bad:?}");
}

/// Diagnostics and stress tests never reach the release build: the app's
/// link to `diag` is optional, and nothing depends on `stress`.
#[test]
fn diagnostics_stay_out_of_the_release_build() {
    let g = graph();
    let mut errors = Vec::new();
    for (name, deps) in &g {
        for d in deps {
            if d.name == "aurastudio-stress" { errors.push(format!("`{name}` depends on `stress`")); }
            if d.name == "aurastudio-diag" && name == "aurastudio" && !d.optional { errors.push("the app must depend on `diag` optionally (feature `diag`)".into()); }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() { rust_files(&p, out); } else if p.extension().is_some_and(|x| x == "rs") { out.push(p); }
    }
}

/// The lines of a file before its first `#[cfg(test)]` module (tests sit
/// at the end of each file), without blank lines and comments.
fn code_lines(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("#[cfg(test)]") && lines.get(n + 1).is_some_and(|l| l.trim_start().starts_with("mod ")) { break; }
        if t.is_empty() || t.starts_with("//") { continue; }
        out.push((n + 1, *line));
    }
    out
}

/// Every crate's `src` files: (crate short name, path).
fn source_files() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(crates_dir()).unwrap().flatten() {
        let dir = e.path();
        let name = short(&dir.file_name().unwrap().to_string_lossy()).to_owned();
        let mut files = Vec::new();
        rust_files(&dir.join("src"), &mut files);
        // (A `tests.rs` is a test module of its own: `#[cfg(test)] mod tests;`.)
        out.extend(files.into_iter().filter(|f| f.file_name().is_none_or(|n| n != "tests.rs")).map(|f| (name.clone(), f)));
    }
    out
}

#[test]
fn no_file_is_too_big() {
    let mut errors = Vec::new();
    for (_, f) in source_files() {
        let text = std::fs::read_to_string(&f).unwrap();
        let n = code_lines(&text).len();
        let rel = f.strip_prefix(crates_dir()).unwrap().to_string_lossy().replace('\\', "/");
        match TOO_BIG.iter().find(|(p, _)| *p == rel) {
            Some((_, listed)) if n > *listed => errors.push(format!("{rel}: {n} lines, more than when it was listed ({listed})")),
            Some(_) => {}
            None if n > MAX_LINES => errors.push(format!("{rel}: {n} lines of code (limit {MAX_LINES}): split it by job")),
            None => {}
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

#[test]
fn data_crates_do_not_unwrap() {
    let mut errors = Vec::new();
    for (name, f) in source_files() {
        if !NO_UNWRAP.contains(&name.as_str()) { continue; }
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in code_lines(&text) {
            for t in [".unwrap()", ".expect("] {
                if line.contains(t) { errors.push(format!("{}:{n}: `{t}`", f.display())); }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// Outside `raster`, no code names a tile, its size or its layout: the
/// storage can change without them.
const TILE_NAMES: &[&str] = &["aurastudio_raster::tile", "raster::tile::", "TILE_SIZE", "TILE_BYTES"];

#[test]
fn only_raster_knows_tiles() {
    let mut errors = Vec::new();
    for (name, f) in source_files() {
        if name == "raster" || name == "arch" { continue; }
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in code_lines(&text) {
            for t in TILE_NAMES {
                if line.contains(t) { errors.push(format!("{}:{n}: `{t}`", f.display())); }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}

/// The interface changes the document only with `Edit`s: no mutable
/// access to the tree or its nodes.
#[test]
fn the_interface_changes_layers_only_with_edits() {
    let mut errors = Vec::new();
    for (name, f) in source_files() {
        if name != "ui" { continue; }
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in code_lines(&text) {
            for t in ["&mut LayerStack", "node_mut(", "name_mut()", "visible_mut()", "locked_mut()", "opacity_mut()", "props_mut()", "Editor::new("] {
                if line.contains(t) { errors.push(format!("{}:{n}: `{t}`", f.display())); }
            }
        }
    }
    assert!(errors.is_empty(), "\n{}", errors.join("\n"));
}
