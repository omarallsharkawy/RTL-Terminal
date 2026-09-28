#![allow(clippy::collapsible_match)]
#![allow(clippy::manual_range_contains)]
#![allow(clippy::single_match)]

mod app;
mod color;
mod config;
mod input;
mod pty;
mod quad;
mod renderer;
mod terminal;

use app::App;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    let event_loop = EventLoop::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);

    event_loop.run_app(&mut app)?;
    Ok(())
}
