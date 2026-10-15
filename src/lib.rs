pub mod app;
pub mod blink;
pub mod color;
pub mod config;
pub mod ime;
pub mod input;
pub mod layout;
pub mod links;
pub mod pty;
pub mod quad;
pub mod render;
pub mod session;
pub mod terminal;
pub mod ui;

/// The renderer's historical path; everything lives in `render` now.
pub use render as renderer;
/// Font setup, now under `render::text`.
pub use render::text::fonts;
/// Older path of `session::pane`.
pub use session::pane;
