//! The shell sessions: panes (terminal + PTY) and how they are arranged into tabs.

pub mod pane;
pub(crate) mod restore;
pub mod shell_setup;
pub mod spawn;
pub mod workspace;
