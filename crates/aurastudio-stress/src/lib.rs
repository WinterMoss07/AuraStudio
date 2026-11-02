//! Dev only: the fuzzers and the diagnostic scenarios. Scenarios are
//! deterministic (seeded, strokelog replays), each run twice so the report
//! can tell drift from noise; fuzzers check the documents after every
//! action. Crates add their scenarios and fuzzers here as they are rebuilt.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use aurastudio_diag::report::{Report, Rules};

pub mod scenarios;

/// One measured situation (idle, painting, layers…).
pub trait Scenario {
    fn name(&self) -> &'static str;
    /// Do the work once; marks go to the recorder. Findings that are the
    /// scenario's own (idle wakeups, preview targets) go to `report`.
    fn run(&mut self, report: &mut Report);
}

/// Run every scenario `runs` times (2 by default: drift), each from a
/// clean recording, and evaluate the rules.
pub fn run_all(scenarios: &mut [Box<dyn Scenario>], runs: u32, header: Vec<(String, String)>, rules: &Rules) -> Report {
    aurastudio_diag::install();
    let mut report = Report { header, ..Default::default() };
    for s in scenarios.iter_mut() {
        for _ in 0..runs.max(1) {
            aurastudio_diag::record::reset();
            s.run(&mut report);
            report.add_run(s.name(), aurastudio_diag::snapshot());
        }
    }
    report.check(rules);
    report
}

/// The machine and build, for the report header.
pub fn header() -> Vec<(String, String)> {
    vec![
        ("version".into(), env!("CARGO_PKG_VERSION").into()),
        ("os".into(), std::env::consts::OS.into()),
        ("arch".into(), std::env::consts::ARCH.into()),
        ("threads".into(), std::thread::available_parallelism().map_or(0, |n| n.get()).to_string()),
        ("profile".into(), if cfg!(debug_assertions) { "debug" } else { "optimized" }.into()),
    ]
}

static PANICKED: AtomicBool = AtomicBool::new(false);
static HOOK: Once = Once::new();

/// Note any panic on any thread (a worker thread's panic must fail the
/// fuzz, not vanish); the default message is still printed.
pub fn watch_panics() {
    HOOK.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICKED.store(true, Ordering::SeqCst);
            default_hook(info);
        }));
    });
}

/// Whether a panic happened since `watch_panics`.
pub fn panicked() -> bool { PANICKED.load(Ordering::SeqCst) }

/// A number from the environment (`FUZZ_SECS`, `FUZZ_SEED`…), or `default`.
pub fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
