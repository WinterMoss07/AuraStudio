//! The diagnostic facade: zones, counters and samples any crate can mark
//! without depending on anything. `aurastudio-diag` (diagnostic builds
//! only) installs the hooks that record them (and forwards zones to Tracy
//! when the profiler is on).
//!
//! Without the `diag` feature every macro compiles to nothing. With it and
//! no hooks installed, a mark costs one relaxed load and a skipped call.
//! This slot is the one piece of state in `core`.

#[cfg(feature = "diag")]
use std::sync::atomic::{AtomicPtr, Ordering};

/// What the recorder provides. Names are `'static` (literals at the call
/// site), so recording one is a pointer copy.
pub struct Hooks {
    /// A zone starts on this thread; returns a token handed back at its
    /// end (a timestamp, a Tracy span id…).
    pub begin:  fn(&'static str) -> u64,
    pub end:    fn(&'static str, u64),
    /// Add `n` to a counter (bytes uploaded, tiles merged…).
    pub count:  fn(&'static str, i64),
    /// A value at this moment (queue depth, MB…).
    pub sample: fn(&'static str, f64),
}

#[cfg(feature = "diag")]
static HOOKS: AtomicPtr<Hooks> = AtomicPtr::new(std::ptr::null_mut());

/// Install the recorder (once, at start; later calls replace it).
pub fn set_hooks(h: &'static Hooks) {
    #[cfg(feature = "diag")]
    HOOKS.store(h as *const Hooks as *mut Hooks, Ordering::Release);
    #[cfg(not(feature = "diag"))]
    let _ = h;
}

#[cfg(feature = "diag")]
#[inline(always)]
fn hooks() -> Option<&'static Hooks> {
    // SAFETY: the pointer is null or comes from a `&'static Hooks`.
    unsafe { HOOKS.load(Ordering::Acquire).as_ref() }
}

/// Whether marks are compiled in (the `diag` feature).
pub const ENABLED: bool = cfg!(feature = "diag");

/// A zone, ended when dropped (`zone!`).
#[must_use]
pub struct Zone {
    #[cfg(feature = "diag")]
    name:  &'static str,
    #[cfg(feature = "diag")]
    token: Option<u64>,
}

impl Zone {
    #[inline(always)]
    pub fn begin(name: &'static str) -> Zone {
        #[cfg(feature = "diag")]
        { Zone { name, token: hooks().map(|h| (h.begin)(name)) } }
        #[cfg(not(feature = "diag"))]
        { let _ = name; Zone {} }
    }
}

impl Drop for Zone {
    #[inline(always)]
    fn drop(&mut self) {
        #[cfg(feature = "diag")]
        if let (Some(t), Some(h)) = (self.token, hooks()) { (h.end)(self.name, t); }
    }
}

#[inline(always)]
pub fn count(name: &'static str, n: i64) {
    #[cfg(feature = "diag")]
    if let Some(h) = hooks() { (h.count)(name, n); }
    #[cfg(not(feature = "diag"))]
    let _ = (name, n);
}

#[inline(always)]
pub fn sample(name: &'static str, v: f64) {
    #[cfg(feature = "diag")]
    if let Some(h) = hooks() { (h.sample)(name, v); }
    #[cfg(not(feature = "diag"))]
    let _ = (name, v);
}

/// Time the rest of the enclosing block: `zone!("merge");`.
#[macro_export]
macro_rules! zone {
    ($name:expr) => {
        let _zone = $crate::diag::Zone::begin($name);
    };
}

/// Add to a counter: `count!("tiles uploaded", n);`.
#[macro_export]
macro_rules! count {
    ($name:expr, $n:expr) => {
        if $crate::diag::ENABLED { $crate::diag::count($name, ($n) as i64) }
    };
}

/// Record a value: `sample!("paint queue", q.len());`.
#[macro_export]
macro_rules! sample {
    ($name:expr, $v:expr) => {
        if $crate::diag::ENABLED { $crate::diag::sample($name, ($v) as f64) }
    };
}

#[cfg(all(test, feature = "diag"))]
mod tests {
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    static BEGUN: AtomicU64 = AtomicU64::new(0);
    static ENDED: AtomicU64 = AtomicU64::new(0);
    static COUNTED: AtomicI64 = AtomicI64::new(0);
    static HOOKS: super::Hooks = super::Hooks {
        begin:  |_| { BEGUN.fetch_add(1, Ordering::Relaxed); 7 },
        end:    |_, t| { assert_eq!(t, 7); ENDED.fetch_add(1, Ordering::Relaxed); },
        count:  |_, n| { COUNTED.fetch_add(n, Ordering::Relaxed); },
        sample: |_, _| {},
    };

    #[test]
    fn marks_reach_the_hooks() {
        super::set_hooks(&HOOKS);
        {
            crate::zone!("test");
            crate::count!("things", 3);
        }
        assert_eq!(BEGUN.load(Ordering::Relaxed), 1);
        assert_eq!(ENDED.load(Ordering::Relaxed), 1);
        assert_eq!(COUNTED.load(Ordering::Relaxed), 3);
    }
}
