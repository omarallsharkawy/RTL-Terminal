//! Text: fonts, bidi and row shaping (CPU only, no GPU types).

mod fit;
pub mod labels;
pub mod ui;

#[doc(hidden)]
pub use fit::fit_text;
pub use twitty_core::text::{bidi, fonts, segment, shaper};
