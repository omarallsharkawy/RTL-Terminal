use alacritty_terminal::term::TermMode;
use twitty::input::{handle_key_mode_raw, InputAction};
use twitty::layout::FocusDir;
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState};

const D: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;
const EV: TermMode = TermMode::REPORT_EVENT_TYPES;
const ALL: TermMode = TermMode::REPORT_ALL_KEYS_AS_ESC;

const CTRL_SHIFT: ModifiersState = ModifiersState::CONTROL.union(ModifiersState::SHIFT);
const ALT_SHIFT: ModifiersState = ModifiersState::ALT.union(ModifiersState::SHIFT);

fn press(physical: KeyCode, m: ModifiersState, mode: TermMode) -> Option<InputAction> {
    // No logical key: the shortcut must be found from the physical key alone.
    handle_key_mode_raw(
        Some(physical),
        &Key::Dead(None),
        None,
        None,
        ElementState::Pressed,
        false,
        m,
        mode,
    )
}

fn modes() -> [TermMode; 5] {
    [TermMode::empty(), D, D | EV, ALL, D | EV | ALL]
}

#[test]
fn tab_and_split_shortcuts_work_in_every_keyboard_mode() {
    for mode in modes() {
        let k = |code, m| press(code, m, mode);
        assert_eq!(k(KeyCode::KeyT, CTRL_SHIFT), Some(InputAction::NewTab));
        assert_eq!(k(KeyCode::KeyW, CTRL_SHIFT), Some(InputAction::ClosePane));
        assert_eq!(k(KeyCode::KeyE, CTRL_SHIFT), Some(InputAction::SplitRight));
        assert_eq!(k(KeyCode::KeyO, CTRL_SHIFT), Some(InputAction::SplitDown));
    }
}

#[test]
fn tab_cycling_shortcuts_work_in_every_keyboard_mode() {
    let ctrl = ModifiersState::CONTROL;
    for mode in modes() {
        let k = |code, m| press(code, m, mode);
        assert_eq!(k(KeyCode::Tab, ctrl), Some(InputAction::NextTab));
        assert_eq!(k(KeyCode::Tab, CTRL_SHIFT), Some(InputAction::PrevTab));
        assert_eq!(k(KeyCode::PageDown, ctrl), Some(InputAction::NextTab));
        assert_eq!(k(KeyCode::PageUp, ctrl), Some(InputAction::PrevTab));
    }
}

#[test]
fn alt_arrows_move_focus_in_every_keyboard_mode() {
    let alt = ModifiersState::ALT;
    for mode in modes() {
        let k = |code, m| press(code, m, mode);
        assert_eq!(
            k(KeyCode::ArrowLeft, alt),
            Some(InputAction::Focus(FocusDir::Left))
        );
        assert_eq!(
            k(KeyCode::ArrowRight, alt),
            Some(InputAction::Focus(FocusDir::Right))
        );
        assert_eq!(
            k(KeyCode::ArrowUp, alt),
            Some(InputAction::Focus(FocusDir::Up))
        );
        assert_eq!(
            k(KeyCode::ArrowDown, alt),
            Some(InputAction::Focus(FocusDir::Down))
        );
    }
}

#[test]
fn other_modifier_combinations_are_not_pane_shortcuts() {
    for mode in modes() {
        let k = |code, m| press(code, m, mode);
        assert_ne!(
            k(KeyCode::KeyT, ModifiersState::CONTROL),
            Some(InputAction::NewTab)
        );
        assert_ne!(
            k(KeyCode::KeyT, ModifiersState::empty()),
            Some(InputAction::NewTab)
        );
        assert_ne!(
            k(KeyCode::ArrowLeft, ALT_SHIFT),
            Some(InputAction::Focus(FocusDir::Left))
        );
        assert_ne!(
            k(KeyCode::ArrowLeft, ModifiersState::empty()),
            Some(InputAction::Focus(FocusDir::Left))
        );
    }
}

fn press_logical(
    physical: KeyCode,
    logical: Key,
    m: ModifiersState,
    mode: TermMode,
) -> Option<InputAction> {
    handle_key_mode_raw(
        Some(physical),
        &logical,
        None,
        None,
        ElementState::Pressed,
        false,
        m,
        mode,
    )
}

#[test]
fn shortcuts_follow_the_typed_latin_key_then_the_physical_key() {
    let cs = ModifiersState::CONTROL | ModifiersState::SHIFT;
    for mode in modes() {
        // Virtual keyboards (wtype) report arbitrary physical codes: the typed letter wins.
        assert_eq!(
            press_logical(KeyCode::KeyQ, Key::Character("E".into()), cs, mode),
            Some(InputAction::SplitRight)
        );
        // Arabic layout: the typed letter is Arabic, so the physical key decides.
        assert_eq!(
            press_logical(KeyCode::KeyT, Key::Character("ف".into()), cs, mode),
            Some(InputAction::NewTab)
        );
    }
}
