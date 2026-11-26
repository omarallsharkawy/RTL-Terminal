#![forbid(unsafe_code)]

// Build the shared daemon source independently of the GUI library.
#[cfg(target_os = "linux")]
#[path = "../remote/daemon/mod.rs"]
pub mod daemon;
#[cfg(target_os = "linux")]
#[path = "../../crates/twitty-core/src/remote/wire.rs"]
mod wire;
#[cfg(target_os = "linux")]
use wire::{validate_name, ATTACH_MARKER, BOOTSTRAP_FAILED, SSH_DISCONNECTED};

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::process::exit(daemon::run(&args));
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("twitty-remote requires Linux");
    std::process::exit(1);
}
