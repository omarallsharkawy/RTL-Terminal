#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    std::process::exit(twitty_remote::daemon::run(&args));
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("twitty-remote requires Linux");
    std::process::exit(1);
}
