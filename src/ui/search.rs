//! The scrollback search bar: editing the query from key presses, and the text it shows.

use alacritty_terminal::index::Direction;
use winit::keyboard::{Key, ModifiersState, NamedKey};

/// Longest scrollback search query kept, in characters.
pub const MAX_SEARCH_QUERY_CHARS: usize = 200;

/// What a key press in the search bar asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SearchCmd {
    /// Close the bar and clear the search.
    Close,
    /// Search again for the (possibly edited) query in this direction.
    Search(Direction),
    /// Nothing changed.
    None,
}

/// Append printable (non-control) characters from `text` to `query`, capped at
/// [`MAX_SEARCH_QUERY_CHARS`]. Returns true when `query` changed.
pub fn push_search_text(query: &mut String, text: &str) -> bool {
    let before = query.len();
    let room = MAX_SEARCH_QUERY_CHARS.saturating_sub(query.chars().count());
    query.extend(
        text.chars()
            .filter(|&c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}'))
            .take(room),
    );
    query.len() != before
}

/// Apply a pressed key to `query`. Enter searches older matches (Shift+Enter newer ones),
/// Backspace and typed text edit the query; nothing ever reaches the PTY.
pub(crate) fn search_key(
    query: &mut String,
    key: &Key,
    text: Option<&str>,
    modifiers: ModifiersState,
) -> SearchCmd {
    match key {
        Key::Named(NamedKey::Escape) => SearchCmd::Close,
        Key::Named(NamedKey::Enter) => SearchCmd::Search(if modifiers.shift_key() {
            Direction::Right
        } else {
            Direction::Left
        }),
        Key::Named(NamedKey::Backspace) => {
            query.pop();
            SearchCmd::Search(Direction::Left)
        }
        _ => {
            if modifiers.control_key() || modifiers.alt_key() {
                return SearchCmd::None;
            }
            let Some(text) = text else {
                return SearchCmd::None;
            };
            if !push_search_text(query, text) {
                return SearchCmd::None;
            }
            SearchCmd::Search(Direction::Left)
        }
    }
}

/// The bar's text: the query, then "no matches" or the current match position.
/// `position` is only asked for when there is a match.
pub fn search_bar_text(
    query: &str,
    has_match: bool,
    position: impl FnOnce() -> Option<(usize, usize)>,
) -> String {
    let status = if query.is_empty() {
        String::new()
    } else if !has_match {
        "  no matches / مفيش نتائج".to_owned()
    } else {
        position().map_or(String::new(), |(i, n)| format!("  {i}/{n}"))
    };
    format!("Find / بحث: {query}{status}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_the_query() {
        let none = ModifiersState::empty();
        let mut q = String::new();
        let key = Key::Character("a".into());
        assert_eq!(
            search_key(&mut q, &key, Some("ab\u{7}"), none),
            SearchCmd::Search(Direction::Left)
        );
        assert_eq!(q, "ab");
        assert_eq!(
            search_key(&mut q, &Key::Named(NamedKey::Backspace), None, none),
            SearchCmd::Search(Direction::Left)
        );
        assert_eq!(q, "a");
        assert_eq!(
            search_key(&mut q, &key, Some("x"), ModifiersState::CONTROL),
            SearchCmd::None
        );
        assert_eq!(
            search_key(&mut q, &key, Some("\u{1b}"), none),
            SearchCmd::None
        );
        assert_eq!(q, "a");
    }

    #[test]
    fn enter_and_escape() {
        let mut q = "x".to_owned();
        let enter = Key::Named(NamedKey::Enter);
        assert_eq!(
            search_key(&mut q, &enter, None, ModifiersState::SHIFT),
            SearchCmd::Search(Direction::Right)
        );
        assert_eq!(
            search_key(
                &mut q,
                &Key::Named(NamedKey::Escape),
                None,
                ModifiersState::empty()
            ),
            SearchCmd::Close
        );
    }

    #[test]
    fn bar_text() {
        assert_eq!(
            search_bar_text("", false, || unreachable!()),
            "Find / بحث: "
        );
        assert_eq!(
            search_bar_text("ab", false, || unreachable!()),
            "Find / بحث: ab  no matches / مفيش نتائج"
        );
        assert_eq!(
            search_bar_text("ab", true, || Some((2, 5))),
            "Find / بحث: ab  2/5"
        );
        assert_eq!(search_bar_text("ab", true, || None), "Find / بحث: ab");
    }
}
