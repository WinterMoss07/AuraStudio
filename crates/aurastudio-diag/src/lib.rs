//! The diagnostic recorder (diagnostic builds only, never shipped): it
//! receives the marks every crate makes through `core::diag` (zones,
//! counters, samples), counts allocations per zone, reads the process
//! memory, and turns a run into one report (JSON for Claude, HTML for a
//! person) with the most expensive behaviors first and the suspects found.

pub mod alloc;
pub mod os;
pub mod record;
pub mod report;

pub use record::{install, snapshot, Snapshot};
pub use report::{Report, Suspect};
