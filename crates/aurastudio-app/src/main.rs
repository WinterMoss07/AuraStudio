mod app;
mod arc_tool;
mod debounce;
mod cursors;
#[cfg(test)]
mod fuzz;
#[cfg(test)]
mod painter;
#[cfg(test)]
mod pure;
mod paint_thread;
mod projection;
mod shapes;

use winit::event_loop::{ControlFlow, DeviceEvents, EventLoop};

fn main() {
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

    let mut app = app::App::new(event_loop.create_proxy());
    event_loop.run_app(&mut app).unwrap();
}
