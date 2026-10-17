mod app;

use winit::event_loop::{ControlFlow, EventLoop};

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

    let mut app = app::App::default();
    app.proxy = Some(event_loop.create_proxy());
    event_loop.run_app(&mut app).unwrap();
}
