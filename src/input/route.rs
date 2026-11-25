//! Where a key press goes: the search bar, a Twitty shortcut, or the program.
//! Pure decisions, so they can be tested without a window.

use super::InputAction;

impl InputAction {
    /// Tab and pane shortcuts. They keep working while the search bar has the keyboard.
    pub(crate) fn is_layout(&self) -> bool {
        matches!(
            self,
            InputAction::NewTab
                | InputAction::ClosePane
                | InputAction::CloseOtherPanes
                | InputAction::TogglePaneZoom
                | InputAction::FlashPane
                | InputAction::RenameTab
                | InputAction::JumpUnread
                | InputAction::NextTab
                | InputAction::PrevTab
                | InputAction::SplitRight
                | InputAction::SplitDown
                | InputAction::Focus(_)
        )
    }
}

/// A pane or tab shortcut with nothing to switch to (focus moves with a single pane, tab
/// cycling with a single tab), or a block shortcut in a pane without command blocks (or on the
/// alternate screen, `blocks_navigable` false). Its key then goes to the program instead
/// (Alt+Left in fish, Ctrl+PageUp in vim, Ctrl+Shift+Up in an editor...). Decided from the
/// shortcut itself, so the release follows its press.
pub(crate) fn shortcut_is_idle(
    shortcut: Option<&InputAction>,
    single_pane: bool,
    tab_count: usize,
    blocks_navigable: bool,
) -> bool {
    match shortcut {
        Some(InputAction::Focus(_)) => single_pane,
        Some(InputAction::NextTab | InputAction::PrevTab) => tab_count < 2,
        Some(InputAction::PrevBlock | InputAction::NextBlock | InputAction::CopyBlockOutput) => {
            !blocks_navigable
        }
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum KeyRoute {
    /// The search bar is open and takes the key; nothing reaches the PTY.
    SearchBar,
    /// Run this action on the focused pane.
    Action(InputAction),
    /// The key does nothing.
    Ignore,
}

/// Route a decoded key: layout shortcuts (and searching all panes, which a jump leaves the
/// search bar open for) always run, everything else goes to the search bar while it is open.
pub(crate) fn route_key(action: Option<InputAction>, search_open: bool) -> KeyRoute {
    match action {
        Some(a) if a.is_layout() || a == InputAction::SearchAll => KeyRoute::Action(a),
        _ if search_open => KeyRoute::SearchBar,
        Some(a) => KeyRoute::Action(a),
        None => KeyRoute::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::FocusDir;

    #[test]
    fn layout_shortcuts_bypass_the_search_bar() {
        assert_eq!(
            route_key(Some(InputAction::NewTab), true),
            KeyRoute::Action(InputAction::NewTab)
        );
        assert_eq!(
            route_key(Some(InputAction::Focus(FocusDir::Left)), true),
            KeyRoute::Action(InputAction::Focus(FocusDir::Left))
        );
        // A jump from "search all terminals" leaves the bar open; searching again still works.
        assert_eq!(
            route_key(Some(InputAction::SearchAll), true),
            KeyRoute::Action(InputAction::SearchAll)
        );
    }

    #[test]
    fn other_keys_go_to_the_open_search_bar() {
        assert_eq!(
            route_key(Some(InputAction::Bytes(b"a".to_vec())), true),
            KeyRoute::SearchBar
        );
        assert_eq!(
            route_key(Some(InputAction::Paste), true),
            KeyRoute::SearchBar
        );
        assert_eq!(route_key(None, true), KeyRoute::SearchBar);
    }

    #[test]
    fn without_search_actions_run_and_nothing_is_ignored() {
        assert_eq!(
            route_key(Some(InputAction::ZoomIn), false),
            KeyRoute::Action(InputAction::ZoomIn)
        );
        assert_eq!(route_key(None, false), KeyRoute::Ignore);
    }

    #[test]
    fn idle_shortcuts() {
        let focus = InputAction::Focus(FocusDir::Right);
        assert!(shortcut_is_idle(Some(&focus), true, 3, true));
        assert!(!shortcut_is_idle(Some(&focus), false, 1, false));
        assert!(shortcut_is_idle(
            Some(&InputAction::NextTab),
            false,
            1,
            true
        ));
        assert!(!shortcut_is_idle(
            Some(&InputAction::PrevTab),
            true,
            2,
            false
        ));
        assert!(!shortcut_is_idle(
            Some(&InputAction::NewTab),
            true,
            1,
            false
        ));
        assert!(!shortcut_is_idle(None, true, 1, false));
    }

    #[test]
    fn block_shortcuts_are_idle_without_blocks() {
        for action in [
            InputAction::PrevBlock,
            InputAction::NextBlock,
            InputAction::CopyBlockOutput,
        ] {
            assert!(
                shortcut_is_idle(Some(&action), false, 2, false),
                "{action:?}"
            );
            assert!(
                !shortcut_is_idle(Some(&action), true, 1, true),
                "{action:?}"
            );
        }
    }
}
