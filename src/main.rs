// Release builds are GUI apps on Windows: no console window opens next to the terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use twitty::app::{App, AppEvent};
use twitty::control::cli::{parse_args, run_cli, CliAction};
use twitty::remote::{bootstrap, Target, BOOTSTRAP_FAILED, PANE_WORKER, SSH_DISCONNECTED};
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == PANE_WORKER) {
        std::process::exit(run_ssh_pane(&args[1..]));
    }
    let env_pane = std::env::var("TWITTY_PANE_ID").ok();
    let startup_ssh = match parse_args(&args, env_pane.as_deref()) {
        Ok(CliAction::RunGui) => None,
        Ok(CliAction::Ssh { destination, name })
            if std::env::var_os("TWITTY_SOCKET").is_none_or(|value| value.is_empty()) =>
        {
            Some(Target::for_tab(&destination, name.as_deref()).map_err(anyhow::Error::msg)?)
        }
        Ok(action) => {
            std::process::exit(run_cli(action));
        }
        Err(msg) => {
            eprintln!("twitty: {msg}");
            std::process::exit(1);
        }
    };

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
    if let Some(target) = startup_ssh {
        app = app.with_startup_ssh(target);
    }

    event_loop.run_app(&mut app)?;
    Ok(())
}

fn run_ssh_pane(args: &[String]) -> i32 {
    let result = (|| {
        let [destination, name, cols, rows] = args else {
            return Err(bootstrap::Error::new("invalid SSH pane arguments"));
        };
        let target = Target::new(destination, name).map_err(bootstrap::Error::new)?;
        let size = |arg: &str| {
            arg.parse::<u16>()
                .ok()
                .filter(|size| *size > 0)
                .ok_or_else(|| bootstrap::Error::new("invalid SSH pane size"))
        };
        bootstrap::run_in_pane(&target, size(cols)?, size(rows)?)
    })();
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("Twitty: {err}");
            if err.disconnected {
                SSH_DISCONNECTED as i32
            } else {
                BOOTSTRAP_FAILED as i32
            }
        }
    }
}
