//! Right-click menu model: separators, keyboard movement, placement near window edges and in
//! Arabic, and hit testing.
use twitty::layout::Rect;
use twitty::ui::chrome::UiLanguage;
use twitty::ui::context_menu::{
    ContextMenu, MenuAction, MenuButton, MenuEffect, MenuEntry, MenuKey, MENU_WIDTH,
};

fn entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::Separator,
        MenuEntry::item("Paste", MenuAction::Command("edit.paste")),
        MenuEntry::Separator,
        MenuEntry::Separator,
        MenuEntry::item("Split right", MenuAction::Command("split.right")),
        MenuEntry::item("Close pane", MenuAction::Command("pane.close")).danger(),
        MenuEntry::Separator,
    ]
}

#[test]
fn separators_are_trimmed_and_never_doubled() {
    let menu = ContextMenu::new(entries(), (10.0, 10.0)).unwrap();
    assert_eq!(menu.entries.len(), 4);
    assert_eq!(menu.entries[1], MenuEntry::Separator);
    assert!(menu.item(1).is_none());
    assert!(ContextMenu::new(vec![MenuEntry::Separator], (0.0, 0.0)).is_none());
}

#[test]
fn arrows_skip_separators_and_wrap() {
    let mut menu = ContextMenu::new(entries(), (10.0, 10.0)).unwrap();
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(0));
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(2));
    menu.move_selection(true);
    menu.move_selection(true);
    assert_eq!(menu.selected, Some(0));
    menu.move_selection(false);
    assert_eq!(menu.selected, Some(3));
}

#[test]
fn menu_opens_beside_the_pointer_and_stays_inside_the_window() {
    let window = Rect::new(0.0, 0.0, 800.0, 600.0);
    let menu = ContextMenu::new(entries(), (100.0, 100.0)).unwrap();
    let layout = menu.layout(window, 1.0, UiLanguage::English);
    assert_eq!((layout.rect.x, layout.rect.y), (100.0, 100.0));
    assert_eq!(layout.rect.w, MENU_WIDTH);

    // Arabic opens toward the left of the pointer.
    let ar = ContextMenu::new(entries(), (400.0, 100.0)).unwrap();
    let layout = ar.layout(window, 1.0, UiLanguage::Arabic);
    assert_eq!(layout.rect.right(), 400.0);

    // Near the bottom-right corner it flips up and left.
    let corner = ContextMenu::new(entries(), (790.0, 590.0)).unwrap();
    let layout = corner.layout(window, 2.0, UiLanguage::English);
    assert!(layout.rect.right() <= 800.0 && layout.rect.bottom() <= 600.0);
    assert!(layout.rect.x >= 0.0 && layout.rect.y >= 0.0);
    // It opened above the pointer, not clamped into a sliver below it.
    assert!(layout.rect.bottom() <= 590.0 && layout.rect.h > 100.0);
}

#[test]
fn hit_finds_items_but_not_separators() {
    let window = Rect::new(0.0, 0.0, 800.0, 600.0);
    let menu = ContextMenu::new(entries(), (50.0, 50.0)).unwrap();
    let layout = menu.layout(window, 1.0, UiLanguage::English);
    let center = |r: Rect| (f64::from(r.x + r.w / 2.0), f64::from(r.y + r.h / 2.0));
    let (x, y) = center(layout.rows[0]);
    assert_eq!(layout.hit(&menu, x, y), Some(0));
    let (x, y) = center(layout.rows[1]);
    assert_eq!(layout.hit(&menu, x, y), None);
    let (x, y) = center(layout.rows[3]);
    assert_eq!(layout.hit(&menu, x, y), Some(3));
    assert_eq!(layout.hit(&menu, 10.0, 10.0), None);
    assert!(!layout.contains(10.0, 10.0));
}

#[test]
fn keys_move_run_and_close_without_a_window() {
    let mut menu = ContextMenu::new(entries(), (10.0, 10.0)).unwrap();
    // Nothing highlighted yet: Enter just closes.
    assert_eq!(menu.key(MenuKey::Activate), MenuEffect::Close);
    assert_eq!(menu.key(MenuKey::Next), MenuEffect::Redraw);
    assert_eq!(menu.key(MenuKey::Next), MenuEffect::Redraw);
    assert_eq!(
        menu.key(MenuKey::Activate),
        MenuEffect::Run(MenuAction::Command("split.right"))
    );
    assert_eq!(menu.key(MenuKey::Other), MenuEffect::None);
    assert_eq!(menu.key(MenuKey::Escape), MenuEffect::Close);
}

#[test]
fn pointer_highlights_items_and_presses_run_or_close() {
    let mut menu = ContextMenu::new(entries(), (10.0, 10.0)).unwrap();
    assert_eq!(menu.hover(Some(2)), MenuEffect::Redraw);
    assert_eq!(menu.hover(Some(2)), MenuEffect::None);
    // A separator or empty space keeps the last highlight.
    assert_eq!(menu.hover(Some(1)), MenuEffect::None);
    assert_eq!(menu.hover(None), MenuEffect::None);
    assert_eq!(menu.selected, Some(2));

    assert_eq!(
        menu.press(Some(3), MenuButton::Secondary),
        MenuEffect::Run(MenuAction::Command("pane.close"))
    );
    assert_eq!(menu.press(Some(1), MenuButton::Primary), MenuEffect::Close);
    assert_eq!(menu.press(None, MenuButton::Primary), MenuEffect::Close);
    assert_eq!(menu.press(Some(0), MenuButton::Other), MenuEffect::Close);
}
