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
    ("forge-library", &["core", "forge"]),
    ("selection", &["core", "raster"]),
    ("layers", &["core", "filters", "raster", "selection"]),
    ("document", &["core", "filters", "layers", "raster", "selection", "transform"]),
    ("file", &["core", "document", "filters", "layers", "raster"]),
    ("gpu", &["core", "layers"]),
    // The interface still reaches into layers, raster, selection, transform
    // and forge directly: to bring down, through `document`.
    ("ui", &["core", "document", "filters", "forge", "forge-library", "layers", "raster", "selection", "transform"]),
    // The application puts everything together.
    ("aurastudio", &["color", "core", "document", "file", "filters", "forge", "forge-library", "gpu", "input", "layers", "selection", "transform", "ui"]),
];

/// Crates that never see a window, the GPU or the interface.
const DATA: &[&str] = &["core", "raster", "selection", "transform", "forge", "forge-library", "layers", "document", "file", "filters", "color"];
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
