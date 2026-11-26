#![forbid(unsafe_code)]

//! Local control socket and CLI client (`twitty ctl`) so scripts and coding agents can drive a
//! running Twitty window.
//!
//! On Unix, [`ControlServer`] binds a mode-0600 domain socket inside a mode-0700 user-owned
//! directory and accepts newline-delimited JSON requests. On other platforms the server and
//! client compile to stubs.

pub mod claude_hook;
pub mod cli;
pub mod client;
pub mod paths;
pub mod protocol;
pub mod server;
mod v1;

pub use protocol::{
    MethodCall, Request, Response, DEFAULT_READ_LINES, MAX_COMMAND_ARGS, MAX_COMMAND_BYTES,
    MAX_LINE_BYTES, MAX_READ_LINES, MAX_SEND_TEXT_BYTES,
};
pub use server::{
    ControlServer, Handler, ServerConfig, MAX_CLIENTS, READ_TIMEOUT, REQUEST_TIMEOUT,
};
