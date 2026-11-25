// Release builds are GUI apps on Windows: no console window opens next to the terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use twitty::app::{App, AppEvent};
use twitty::control::cli::{parse_args, run_cli, CliAction};
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env_pane = std::env::var("TWITTY_PANE_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("CMUX_SURFACE_ID")
                .ok()
                .filter(|s| !s.trim().is_empty())
        });
    match parse_args(&args, env_pane.as_deref()) {
        Ok(CliAction::RunGui) => {}
        Ok(action) => {
            std::process::exit(run_cli(action));
        }
        Err(msg) => {
            eprintln!("twitty: {msg}");
            std::process::exit(1);
        }
    }

    // Logs go to stderr; TWITTY_LOG (or RUST_LOG) picks the level, e.g. TWITTY_LOG=debug.
    let level = std::env::var("TWITTY_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|v| v.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::WARN);
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();

    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy);

    event_loop.run_app(&mut app)?;
    Ok(())
}
