//! Text: fonts, bidi and row shaping (CPU only, no GPU types).

pub mod labels;
pub mod ui;

pub use twitty_core::text::{bidi, fonts, segment, shaper};
#[doc(hidden)]
pub use twitty_style::fit::fit_text;
