#![forbid(unsafe_code)]

//! The terminal grid, shell events, command blocks, and CPU-only text shaping.

pub mod blocks;
pub mod boxdraw;
pub mod color;
pub mod language;
pub mod layout;
pub mod links;
pub mod paste;
pub mod pty;
pub mod remote;
pub mod row;
pub mod shell_integration;
pub mod style;
pub mod terminal;
pub mod text;
#[doc(hidden)]
pub mod warnings;
