mod app;
mod memory;
mod merge_thread;
mod cursors;
mod latency;
mod paint_thread;
mod preview;
mod projection;
mod rescue;
mod size_drag;
mod system;
mod window;

/// A lock taken even if a thread panicked while holding it (the data is
/// still used: a crash report or a save beats a second panic).
pub(crate) trait Locked<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> Locked<T> for std::sync::Mutex<T> {
    fn locked(&self) -> std::sync::MutexGuard<'_, T> { self.lock().unwrap_or_else(|e| e.into_inner()) }
}

#[global_allocator]
static ALLOC: memory::Counting = memory::Counting;

use winit::event_loop::{ControlFlow, DeviceEvents, EventLoop};

fn main() {
    // Tracy profiler build (--features profile): the client runs from here
    // on, the Tracy window connects to it (README, Profiling).
    #[cfg(feature = "profile")]
    let _tracy = tracy_client::Client::start();
    profiling::register_thread!("main");
    // Diagnostic build: zone timers on (Help > Diagnostic writes the report).
    #[cfg(feature = "diag")]
    aurastudio_diag::install();
    memory::mark("program start");
    // Info level by default so the startup log (pen backend, GPU) shows in
    // the console without setting RUST_LOG.
    // wgpu_hal warnings are per-frame driver chatter (e.g. "Suboptimal
    // present", which we handle by reconfiguring the surface): keep errors.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_hal=error"),
    ).init();

    let event_loop = EventLoop::<app::AppEvent>::with_user_event().build().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);
    // No raw input (unused): on Windows it posts a message per mouse report
    // (1000 per second for many mice), the queue never empties and the
    // frames waiting for it lag (sliders froze, then jumped).
    event_loop.listen_device_events(DeviceEvents::Never);

    // A crash keeps the work and writes a report (rescue.rs).
    rescue::install(app::recovery_path(), dirs::data_dir().map(|d| d.join("AuraStudio").join("crashes")));
    memory::mark("event loop");
    let mut app = app::App::new(event_loop.create_proxy());
    memory::mark("brushes, document, paint thread");
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| event_loop.run_app(&mut app)));
    match run {
        Ok(r) => { if let Err(e) = r { log::error!("event loop: {e}"); } }
        Err(_) => rescue::after_unwind(),
    }
}
