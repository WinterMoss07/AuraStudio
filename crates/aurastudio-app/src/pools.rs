//! The cores, shared out (one rayon setup): the global pool, every core,
//! for what the painter waits for (painting, the screen's merge, opening,
//! exporting), and a small pool for what can wait (stroke previews,
//! filter previews, saves, brush imports, recent-file thumbnails), its
//! threads below normal priority on Windows: work that can wait never
//! takes the cores from a stroke, and no thread is made for a job.

use std::sync::OnceLock;

/// Threads of the background pool: a quarter of the cores, at least one.
fn background_threads() -> usize { (std::thread::available_parallelism().map_or(2, |n| n.get()) / 4).max(1) }

#[cfg(windows)]
fn lower_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
    // SAFETY: the calling thread's pseudo handle, always valid.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

#[cfg(not(windows))]
fn lower_priority() {}

/// The pool for work that can wait (its parallel loops stay in it).
pub fn background() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(background_threads())
            .thread_name(|i| format!("background {i}"))
            .start_handler(|_| lower_priority())
            .build()
            .unwrap_or_else(|e| {
                log::warn!("background pool: {e}; one thread");
                rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("a one-thread pool")
            })
    })
}

/// Run `f` on the background pool.
pub fn spawn_background(f: impl FnOnce() + Send + 'static) { background().spawn(f); }

/// Run `f` on the global pool (work the painter waits for).
pub fn spawn_work(f: impl FnOnce() + Send + 'static) { rayon::spawn(f); }
