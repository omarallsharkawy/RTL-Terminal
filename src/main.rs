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
mod shaping;
mod terminal;

use app::App;
use winit::event_loop::EventLoop;

fn parse_args() -> Option<(String, Vec<String>)> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return None;
    }

    if args[0] == "-e" {
        if args.len() > 1 {
            let cmd = args[1].clone();
            let cmd_args = args[2..].to_vec();
            return Some((cmd, cmd_args));
        } else {
            return None;
        }
    }

    if args[0] == "--version" || args[0] == "-v" {
        println!("Twitty v{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args[0] == "--help" || args[0] == "-h" {
        println!(
            "Twitty v{} - Native Bidirectional RTL Terminal",
            env!("CARGO_PKG_VERSION")
        );
        println!("Usage: twitty [OPTIONS] [-e COMMAND [ARGS...]]");
        println!();
        println!("Options:");
        println!("  -e <CMD> [ARGS...]  Execute command in terminal");
        println!("  -v, --version       Print version");
        println!("  -h, --help          Print help");
        std::process::exit(0);
    }

    let cmd = args[0].clone();
    let cmd_args = args[1..].to_vec();
    Some((cmd, cmd_args))
}

fn main() -> anyhow::Result<()> {
    let custom_cmd = parse_args();
    let event_loop = EventLoop::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, custom_cmd);

    event_loop.run_app(&mut app)?;
    Ok(())
}
