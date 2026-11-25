//! The code review panel: the focused repository's uncommitted changes next to the terminal,
//! with line comments sent to a coding agent.
//!
//! - `diff`: a unified diff parsed into files, hunks and numbered lines, within bounds.
//! - `git`: runs git (fixed arguments, `-C <repo>` only) off the event loop, bounded, plus the
//!   untracked files as added files.
//! - `rows`: the diff flattened into drawable rows, with scroll and selection math.
//! - `layout`: where the panel and its header, buttons and rows go, and hit testing.
//! - `comment`: formatting and cleaning a line comment, and choosing the agent pane for it.
//! - `panel`: the panel's state machine (keys, clicks, the comment box) and the `View` it hands
//!   to `render::scene::review`.
//!
//! Nothing here touches a window; the app hooks live in `app/review.rs`.

pub mod comment;
pub mod diff;
pub mod git;
pub mod layout;
pub mod panel;
pub mod rows;

pub use diff::{Diff, DiffLine, FileDiff, FileStatus, Hunk, LineKind};
pub use git::{LoadError, Snapshot};
pub use panel::{PanelCmd, ReviewPanel};
