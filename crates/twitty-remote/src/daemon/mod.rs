//! Persistent PTY sessions for the Linux remote helper.

mod client;
mod protocol;
mod runtime;
mod server;

use std::io::{self, Write};

use anyhow::{anyhow, bail, Result};

use self::protocol::{byte, Size, KILL, STATUS};
use self::runtime::Runtime;
use super::{validate_name, BOOTSTRAP_FAILED, SSH_DISCONNECTED};

pub use server::{ReplayBuffer, REPLAY_LIMIT};

/// Exercise the Unix-socket wire protocol decoders on arbitrary untrusted bytes (used by fuzzing).
#[doc(hidden)]
pub fn fuzz_protocol_stream(bytes: &[u8]) {
    let mut cursor = io::Cursor::new(bytes);
    let mut buf = [0u8; protocol::CHUNK];
    while let Ok(tag) = protocol::byte(&mut cursor) {
        match tag {
            protocol::ATTACH | protocol::RESIZE => {
                if Size::read(&mut cursor).is_err() {
                    break;
                }
            }
            protocol::INPUT | protocol::OUTPUT => {
                if protocol::read_data(&mut cursor, &mut buf).is_err() {
                    break;
                }
            }
            protocol::EXIT => {
                if protocol::byte(&mut cursor).is_err() {
                    break;
                }
            }
            protocol::STATUS | protocol::KILL => {}
            _ => break,
        }
    }
}

const USAGE: &str =
    "usage: twitty-remote attach <name> [--cols N --rows N] | list | kill <name> | version";

pub fn run(args: &[String]) -> i32 {
    let pane = args.first().is_some_and(|cmd| cmd == "attach")
        && std::env::var_os("TWITTY_REMOTE_MARKER").is_some_and(|value| value == "1");
    match dispatch(args) {
        Ok(code) if pane && matches!(u32::from(code), BOOTSTRAP_FAILED | SSH_DISCONNECTED) => 1,
        Ok(code) => i32::from(code),
        Err(err) => {
            eprintln!("twitty-remote: {err:#}");
            if pane {
                BOOTSTRAP_FAILED as i32
            } else {
                1
            }
        }
    }
}

fn dispatch(args: &[String]) -> Result<u8> {
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["version"] => println!("twitty-remote {}", env!("CARGO_PKG_VERSION")),
        ["list"] => {
            let runtime = Runtime::open()?;
            for name in runtime.names()? {
                if request(&runtime, &name, STATUS).unwrap_or(false) {
                    println!("{name}");
                }
            }
        }
        ["kill", name] => {
            validate_name(name).map_err(|err| anyhow!(err))?;
            if !request(&Runtime::open()?, name, KILL)? {
                bail!("session {name} does not exist");
            }
        }
        ["attach", name, options @ ..] => {
            validate_name(name).map_err(|err| anyhow!(err))?;
            let size = parse_size(options)?;
            return client::attach(&Runtime::open()?, name, size);
        }
        ["__serve", name, cols, rows] => {
            validate_name(name).map_err(|err| anyhow!(err))?;
            let size = Size::new(cols.parse()?, rows.parse()?)?;
            rustix::process::setsid()?;
            server::serve(&Runtime::open()?, name, size)?;
        }
        _ => bail!("{USAGE}"),
    }
    Ok(0)
}

fn parse_size(options: &[&str]) -> Result<Size> {
    let measured = client::measured_size().unwrap_or(Size { cols: 80, rows: 24 });
    let [mut cols, mut rows] = [None, None];
    let (pairs, remainder) = options.as_chunks::<2>();
    for pair in pairs {
        match pair {
            ["--cols", value] if cols.is_none() => cols = Some(value.parse()?),
            ["--rows", value] if rows.is_none() => rows = Some(value.parse()?),
            _ => bail!("{USAGE}"),
        }
    }
    if !remainder.is_empty() {
        bail!("{USAGE}");
    }
    Ok(Size::new(
        cols.unwrap_or(measured.cols),
        rows.unwrap_or(measured.rows),
    )?)
}

fn request(runtime: &Runtime, name: &str, command: u8) -> io::Result<bool> {
    let mut stream = match runtime.connect(name) {
        Ok(stream) => stream,
        Err(err) if client::needs_server(&err) => return Ok(false),
        Err(err) => return Err(err),
    };
    stream.set_read_timeout(Some(client::REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(client::REQUEST_TIMEOUT))?;
    stream.write_all(&[command])?;
    Ok(byte(&mut stream)? == command)
}
