//! The scenarios. Only `idle` exists while the other crates are rebuilt;
//! each crate brings its own (painting, layers, view, history…).

use aurastudio_diag::report::Report;

use crate::Scenario;

/// Nothing to do for a while: what wakes up, what allocates. For now the
/// process alone (the app's event loop joins it with the app crate).
pub struct Idle { pub millis: u64 }

impl Scenario for Idle {
    fn name(&self) -> &'static str { "idle" }
    fn run(&mut self, _report: &mut Report) {
        // (No zone around the wait: a zone is work, and waiting is not.)
        std::thread::sleep(std::time::Duration::from_millis(self.millis));
    }
}

/// Every scenario, in report order.
pub fn all() -> Vec<Box<dyn Scenario>> {
    vec![Box::new(Idle { millis: crate::env_u64("IDLE_MS", 2000) })]
}
