//! Building the frame on the CPU: every quad and text run, before any GPU work.

pub mod ai;
pub mod blocks;
pub mod chrome;
pub mod composer;
pub mod frame;
mod motion;
pub mod palette;
pub mod review;
pub mod settings;
pub mod sidebar;
pub mod welcome;
pub mod window_chrome;

pub use twitty_render::scene::{boxdraw, context, cursor, preedit, row_quads};
