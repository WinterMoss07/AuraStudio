//! `cargo test -p aurastudio-stress` runs a short fuzz of each kind;
//! `FUZZ_SEEDS=2000 FUZZ_OPS=400 cargo test --release -p aurastudio-stress
//! -- --ignored` a long one.

#[test]
fn raster_fuzz_short() {
    for seed in 1..=40 { aurastudio_stress::fuzz_raster::run(seed, 60); }
}

#[test]
#[ignore]
fn raster_fuzz_long() {
    let (seeds, ops) = (aurastudio_stress::env_u64("FUZZ_SEEDS", 500), aurastudio_stress::env_u64("FUZZ_OPS", 300) as u32);
    for seed in 1..=seeds { aurastudio_stress::fuzz_raster::run(seed, ops); }
}
