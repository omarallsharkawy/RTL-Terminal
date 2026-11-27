//! Building the frame on the CPU: every quad and text run, before any GPU work. Reads terminal
//! snapshots and the window-free UI state (`twitty-ui`); the `twitty` crate's renderer hands
//! the result to the GPU. Reached from the app as `twitty::render::scene`.
#![forbid(unsafe_code)]

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

use twitty_core::layout::{PaneId, Rect};
use twitty_core::terminal::{CursorState, LineData};

/// Everything the renderer needs to draw one pane.
pub struct PaneFrame {
    pub id: PaneId,
    /// Where the pane goes in the window (physical pixels, padding included).
    pub rect: Rect,
    pub lines: Vec<LineData>,
    pub cursor: CursorState,
    /// The pane keyboard input goes to; the others get a hollow cursor.
    pub focused: bool,
    /// One-line bar drawn over the pane's last row (search query), if any.
    pub status: Option<String>,
    /// Command blocks on screen (`BlockList::decor`); empty for most panes.
    pub blocks: Vec<twitty_ui::blocks::BlockDecor>,
}

/// A tab in the tab bar.
pub struct TabLabel {
    pub title: String,
    pub active: bool,
}
