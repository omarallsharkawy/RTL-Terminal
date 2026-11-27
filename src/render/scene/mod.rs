//! Building the frame on the CPU: every quad and text run, before any GPU work.

pub mod agent;
pub mod ai;
pub mod blocks;
pub mod chrome;
pub mod composer;
pub mod context_menu;
pub mod frame;
pub mod memory;
mod motion;
pub mod palette;
pub mod review;
pub mod settings;
pub mod sidebar;
pub mod welcome;
pub mod window_chrome;

pub use twitty_render::scene::{boxdraw, context, cursor, preedit, row_quads};
