//! Twitty's interface without a window or GPU: the config, the command table, key and mouse
//! input, and the state, geometry and hit testing of every panel (palette, settings, composer,
//! review, sidebar, agent panel, menus). Drawing lives in `twitty-scene` / `twitty-render`;
//! SSH remoting lives in `twitty-remote`; sessions, the control socket and the window live in
//! the `twitty` crate.
#![forbid(unsafe_code)]

pub mod ai;
pub mod blocks;
pub mod commands;
pub mod composer;
pub mod config;
pub mod input;
pub mod ports;
pub(crate) mod process;
pub mod projects;
pub mod review;
pub mod sidebar;
pub mod ui;
pub mod workflows;

pub use twitty_ai::agent;
pub use twitty_core::{color, layout, links, pty, redact, shell_integration, terminal};
