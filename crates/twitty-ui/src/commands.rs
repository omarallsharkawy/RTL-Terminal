//! Every Twitty command by a stable name, for the command palette, the control socket
//! (`twitty ctl`) and anything else that triggers actions without a key press.
//!
//! The names are a public interface: scripts and agents call them, so a name, once shipped,
//! is never renamed or reused. Shortcuts live in `input::keys`; the code that runs a command
//! is `App::run_action`.

use twitty_core::en;
use twitty_core::language::UiLanguage;

use crate::input::InputAction;
use crate::layout::FocusDir;

/// One command as the palette lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Stable name, e.g. `split.right`.
    pub name: &'static str,
    pub title_en: &'static str,
    /// From the Arabic catalog (the palette shows both).
    pub title_ar: &'static str,
    /// Shortcut as shown to the user, if it has one.
    pub shortcut: Option<&'static str>,
}

macro_rules! commands {
    ($( $name:literal, $en:expr, $keys:expr => $action:expr; )*) => {
        /// Every command, in palette order.
        pub fn all() -> Vec<Command> {
            vec![$(Command {
                name: $name,
                title_en: $en,
                title_ar: UiLanguage::Arabic.t($en),
                shortcut: $keys,
            }),*]
        }

        /// The action a command name runs.
        pub fn action(name: &str) -> Option<InputAction> {
            match name {
                $($name => Some($action),)*
                _ => None,
            }
        }
    };
}

commands! {
    "palette.open", en!("Command palette"), Some("Ctrl+Shift+P") => InputAction::CommandPalette;
    "composer.open", en!("Compose command"), Some("Ctrl+Shift+Space") => InputAction::ToggleComposer;
    "settings.open", en!("Settings"), Some("Ctrl+,") => InputAction::Settings;
    "memory.open", en!("Project memory"), None => InputAction::ProjectMemory;
    "welcome.open", en!("Welcome"), None => InputAction::Welcome;
    "tab.new", en!("New tab"), Some("Ctrl+Shift+T") => InputAction::NewTab;
    "tab.next", en!("Next tab"), Some("Ctrl+Tab") => InputAction::NextTab;
    "tab.prev", en!("Previous tab"), Some("Ctrl+Shift+Tab") => InputAction::PrevTab;
    "tab.move-left", en!("Move tab left"), Some("Ctrl+Shift+PageUp") => InputAction::MoveTabLeft;
    "tab.move-right", en!("Move tab right"), Some("Ctrl+Shift+PageDown") => InputAction::MoveTabRight;
    "tab.reopen", en!("Reopen closed tab"), Some("Ctrl+Shift+Alt+T") => InputAction::ReopenClosedTab;
    "tab.rename", en!("Rename tab…"), Some("Ctrl+Shift+N") => InputAction::RenameTab;
    "project.open", en!("Open project"), Some("Ctrl+Shift+L") => InputAction::OpenProject;
    "layout.save", en!("Save layout…"), None => InputAction::SaveLayout;
    "layout.open", en!("Open layout…"), None => InputAction::OpenLayout;
    "layout.delete", en!("Delete layout…"), None => InputAction::DeleteLayout;
    "input.broadcast", en!("Type in all panes of this tab"), None => InputAction::ToggleBroadcast;
    "pane.zoom", en!("Toggle pane zoom"), Some("Ctrl+Shift+Z") => InputAction::TogglePaneZoom;
    "pane.flash", en!("Flash focused pane"), Some("Ctrl+Shift+H") => InputAction::FlashPane;
    "pane.close-others", en!("Close other panes"), Some("Ctrl+Shift+K") => InputAction::CloseOtherPanes;
    "pane.close", en!("Close pane"), Some("Ctrl+Shift+W") => InputAction::ClosePane;
    "split.right", en!("Split right"), Some("Ctrl+Shift+E") => InputAction::SplitRight;
    "split.down", en!("Split down"), Some("Ctrl+Shift+O") => InputAction::SplitDown;
    "focus.left", en!("Focus pane on the left"), Some("Alt+Left") => InputAction::Focus(FocusDir::Left);
    "focus.right", en!("Focus pane on the right"), Some("Alt+Right") => InputAction::Focus(FocusDir::Right);
    "focus.up", en!("Focus pane above"), Some("Alt+Up") => InputAction::Focus(FocusDir::Up);
    "focus.down", en!("Focus pane below"), Some("Alt+Down") => InputAction::Focus(FocusDir::Down);
    "sidebar.toggle", en!("Toggle sidebar"), Some("Ctrl+Shift+B") => InputAction::ToggleSidebar;
    "attention.next", en!("Go to next notification"), Some("Ctrl+Shift+U") => InputAction::NextAttention;
    "notifications.list", en!("Notifications"), Some("Ctrl+Shift+I") => InputAction::NotificationList;
    "notifications.jump-unread", en!("Jump to newest unread notification"), Some("Ctrl+Shift+J") => InputAction::JumpUnread;
    "block.prev", en!("Previous command block"), Some("Ctrl+Shift+Up") => InputAction::PrevBlock;
    "block.next", en!("Next command block"), Some("Ctrl+Shift+Down") => InputAction::NextBlock;
    "block.copy-output", en!("Copy command output"), Some("Ctrl+Shift+Y") => InputAction::CopyBlockOutput;
    "ai.explain-error", en!("Explain error"), None => InputAction::ExplainBlockError;
    "ai.suggest-fix", en!("Suggest a fix"), None => InputAction::SuggestBlockFix;
    "agent.ask", en!("Ask an agent about this output"), Some("Ctrl+Shift+A") => InputAction::AskAgent;
    "agent.panel", en!("Agent panel"), Some("Ctrl+Shift+.") => InputAction::ToggleAgentPanel;
    "review.toggle", en!("Review changes"), Some("Ctrl+Shift+R") => InputAction::ToggleReview;
    "ssh.reconnect", en!("Reconnect SSH"), None => InputAction::ReconnectSsh;
    "edit.copy", en!("Copy"), Some("Ctrl+Shift+C") => InputAction::Copy;
    "edit.paste", en!("Paste"), Some("Ctrl+Shift+V") => InputAction::Paste;
    "search.open", en!("Find in scrollback"), Some("Ctrl+Shift+F") => InputAction::Search;
    "search.all", en!("Search all terminals"), Some("Ctrl+Shift+G") => InputAction::SearchAll;
    "zoom.in", en!("Zoom in"), Some("Ctrl+=") => InputAction::ZoomIn;
    "zoom.out", en!("Zoom out"), Some("Ctrl+-") => InputAction::ZoomOut;
    "zoom.reset", en!("Reset zoom"), Some("Ctrl+0") => InputAction::ZoomReset;
}
