pub mod app;
pub mod blink;
pub mod color;
pub mod config;
pub mod ime;
pub mod input;
pub mod layout;
pub mod links;
pub mod pane;
pub mod pty;
pub mod quad;
pub mod render;
pub mod terminal;

/// The renderer's historical path; everything lives in `render` now.
pub use render as renderer;
/// Font setup, now under `render::text`.
pub use render::text::fonts;
