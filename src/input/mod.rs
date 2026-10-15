//! Keyboard and mouse input: what bytes a key or button sends, and where input is routed.
//!
//! `keys` holds the key and mouse encoders. `route` decides whether a key goes to the
//! search bar, a Twitty shortcut or the program; `mouse` and `pointer` are the window-free
//! mouse state machines.

mod keys;
pub mod mouse;
pub mod pointer;
pub mod route;

pub use keys::*;
