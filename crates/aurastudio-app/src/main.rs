mod app;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    // Info level by default so the startup log (pen backend, GPU) shows in
    // the console without setting RUST_LOG.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = app::App::default();
    event_loop.run_app(&mut app).unwrap();
}
