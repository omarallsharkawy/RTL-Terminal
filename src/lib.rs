#![forbid(unsafe_code)]
pub mod app;
pub mod blink;
pub mod color;
pub mod config;
pub mod ime;
pub mod input;
pub mod layout;
pub mod links;
pub mod pty;
pub mod render;
pub mod session;
pub mod terminal;
pub mod ui;

/// Older path of `session::pane`.
pub use session::pane;
