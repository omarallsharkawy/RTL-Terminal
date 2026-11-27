//! Right-click menus on panes, command blocks, tabs and the window chrome: what each lists and
//! running the chosen row. Geometry and the menu model live in `ui::context_menu`.

use alacritty_terminal::term::TermMode;
use winit::event::{ElementState, KeyEvent, MouseButton};
use winit::keyboard::{Key, NamedKey};

use super::App;
use crate::blocks::cards::BlockButton;
use crate::blocks::BlockStatus;
use crate::layout::PaneId;
use crate::ui::chrome::{tr, UiLanguage};
use crate::ui::context_menu::{
    BlockAction, ContextMenu, MenuAction, MenuButton, MenuEffect, MenuEntry, MenuKey,
};
use crate::ui::hit::Hit;

/// An open menu and the pane it was opened on.
#[derive(Clone, Debug)]
pub(super) struct ContextMenuHost {
    pub menu: ContextMenu,
    pub pane: Option<PaneId>,
}

impl App {
    pub(super) fn context_menu_open(&self) -> bool {
        self.context_menu.is_some()
    }

    pub(super) fn close_context_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.window.request_redraw();
        }
    }

    /// A row for built-in command `name`, titled and with its shortcut as the palette shows it.
    fn command_entry(&self, name: &'static str) -> MenuEntry {
        let title = crate::commands::all()
            .into_iter()
            .find(|c| c.name == name)
            .map_or(name, |c| match self.window.ui_language() {
                UiLanguage::English => c.title_en,
                UiLanguage::Arabic => c.title_ar,
            });
        MenuEntry::item(title, MenuAction::Command(name))
            .with_shortcut(self.keybindings.display_shortcut(name))
    }

    /// Open the menu for what is under the pointer. Returns false when there is nothing to show
    /// (the press then goes on as usual).
    pub(super) fn open_context_menu(&mut self, hit: &Hit) -> bool {
        let (x, y) = self.pointer.pos;
        let (entries, pane) = match *hit {
            Hit::Pane(id) => (self.pane_menu(id, x, y), Some(id)),
            Hit::Tab(index) => (self.tab_menu(index), None),
            Hit::TabBar | Hit::ChromeBackground | Hit::Nothing => (
                vec![
                    self.command_entry("tab.new"),
                    self.command_entry("palette.open"),
                    MenuEntry::Separator,
                    self.command_entry("settings.open"),
                ],
                None,
            ),
            Hit::Divider(_) | Hit::Chrome(_) => return false,
        };
        let Some(menu) = ContextMenu::new(entries, (x as f32, y as f32)) else {
            return false;
        };
        if let Some(id) = pane {
            self.focus_pane(id);
        }
        self.context_menu = Some(ContextMenuHost { menu, pane });
        self.window.set_block_hover(None);
        self.window.request_redraw();
        true
    }

    fn pane_menu(&self, id: PaneId, x: f64, y: f64) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        let has_selection = self.ws.pane(id).is_some_and(|p| p.terminal.has_selection());
        if has_selection {
            entries.push(
                MenuEntry::item(tr("Copy", "نسخ"), MenuAction::CopySelection)
                    .with_shortcut(self.keybindings.display_shortcut("edit.copy")),
            );
        }
        entries.push(self.command_entry("edit.paste"));
        entries.push(MenuEntry::Separator);
        if let Some((block, _)) = self.block_card_hit(id, x, y) {
            let failed = self
                .ws
                .pane(id)
                .and_then(|p| p.blocks.block_by_id(block))
                .is_some_and(|b| matches!(b.status(), BlockStatus::Failed(_)));
            let has_command = self
                .ws
                .pane(id)
                .and_then(|p| p.blocks.block_by_id(block))
                .is_some_and(|b| b.command.as_deref().is_some_and(|c| !c.trim().is_empty()));
            if has_command {
                entries.push(MenuEntry::item(
                    tr("Copy command", "انسخ الأمر"),
                    MenuAction::Block(BlockAction::CopyCommand, block),
                ));
            }
            entries.push(MenuEntry::item(
                tr("Copy output", "انسخ الناتج"),
                MenuAction::Block(BlockAction::CopyOutput, block),
            ));
            if has_command {
                entries.push(MenuEntry::item(
                    tr("Run again", "شغّله تاني"),
                    MenuAction::Block(BlockAction::Rerun, block),
                ));
            }
            if failed && self.config.ai {
                entries.push(MenuEntry::item(
                    tr("Explain error", "اشرح الخطأ"),
                    MenuAction::Block(BlockAction::ExplainError, block),
                ));
            }
            entries.push(MenuEntry::item(
                tr("Ask an agent about this", "اسأل وكيل عن ده"),
                MenuAction::Block(BlockAction::AskAgent, block),
            ));
            entries.push(MenuEntry::Separator);
        }
        entries.push(self.command_entry("split.right"));
        entries.push(self.command_entry("split.down"));
        if !self.ws.single_pane() {
            entries.push(self.command_entry("pane.zoom"));
        }
        entries.push(MenuEntry::Separator);
        entries.push(self.command_entry("search.open"));
        entries.push(self.command_entry("composer.open"));
        entries.push(MenuEntry::Separator);
        entries.push(self.command_entry("pane.close").danger());
        entries
    }

    fn tab_menu(&self, index: usize) -> Vec<MenuEntry> {
        let mut entries = vec![
            self.command_entry("tab.new"),
            MenuEntry::item(
                tr("Rename tab…", "غيّر اسم التاب…"),
                MenuAction::RenameTab(index),
            )
            .with_shortcut(self.keybindings.display_shortcut("tab.rename")),
            MenuEntry::Separator,
        ];
        if self.ws.tabs().len() > 1 {
            entries.push(MenuEntry::item(
                tr("Close other tabs", "اقفل باقي التابات"),
                MenuAction::CloseOtherTabs(index),
            ));
        }
        entries.push(
            MenuEntry::item(tr("Close tab", "اقفل التاب"), MenuAction::CloseTab(index)).danger(),
        );
        entries
    }

    /// The item under the pointer in the open menu.
    fn menu_row_under_pointer(&self) -> Option<usize> {
        let (x, y) = self.pointer.pos;
        let host = self.context_menu.as_ref()?;
        self.window
            .context_menu_layout(&host.menu)?
            .hit(&host.menu, x, y)
    }

    /// Carry out what the menu asked for. Returns true when the window should close.
    fn apply_menu_effect(&mut self, effect: MenuEffect) -> bool {
        match effect {
            MenuEffect::None => false,
            MenuEffect::Redraw => {
                self.window.request_redraw();
                false
            }
            MenuEffect::Close => {
                self.close_context_menu();
                false
            }
            MenuEffect::Run(action) => {
                let pane = self.context_menu.as_ref().and_then(|h| h.pane);
                self.close_context_menu();
                self.run_menu_action(pane, action)
            }
        }
    }

    /// Pointer moved while a menu is open: highlight the row under it.
    pub(super) fn context_menu_motion(&mut self) {
        let row = self.menu_row_under_pointer();
        if let Some(effect) = self.context_menu.as_mut().map(|h| h.menu.hover(row)) {
            self.apply_menu_effect(effect);
        }
        self.window
            .set_cursor_icon(winit::window::CursorIcon::Default);
    }

    /// A mouse button while a menu is open. The menu takes every press: a row runs, anywhere
    /// else closes the menu. Returns true when the window should close.
    pub(super) fn context_menu_mouse(&mut self, button: MouseButton, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        let row = self.menu_row_under_pointer();
        let button = match button {
            MouseButton::Left => MenuButton::Primary,
            MouseButton::Right => MenuButton::Secondary,
            _ => MenuButton::Other,
        };
        let Some(effect) = self
            .context_menu
            .as_ref()
            .map(|h| h.menu.press(row, button))
        else {
            return false;
        };
        self.apply_menu_effect(effect)
    }

    /// Keys while a menu is open. Returns `Some(close_window)` when the menu took the key.
    pub(super) fn context_menu_key(&mut self, event: &KeyEvent) -> Option<bool> {
        let host = self.context_menu.as_mut()?;
        if event.state != ElementState::Pressed {
            return Some(false);
        }
        let key = match &event.logical_key {
            Key::Named(NamedKey::Escape) => MenuKey::Escape,
            Key::Named(NamedKey::ArrowDown | NamedKey::Tab) => MenuKey::Next,
            Key::Named(NamedKey::ArrowUp) => MenuKey::Previous,
            Key::Named(NamedKey::Enter | NamedKey::Space) => MenuKey::Activate,
            _ => MenuKey::Other,
        };
        let effect = host.menu.key(key);
        Some(self.apply_menu_effect(effect))
    }

    /// Run a menu row's action for the menu opened on `pane`.
    fn run_menu_action(&mut self, pane: Option<PaneId>, action: MenuAction) -> bool {
        if let Some(id) = pane {
            if self.ws.pane(id).is_some() {
                self.focus_pane(id);
            }
        }
        match action {
            MenuAction::Command(name) => return self.run_command(name),
            MenuAction::CopySelection => {
                if pane.is_some_and(|id| self.copy_selection(id)) {
                    self.toast(crate::ui::toast::ToastKind::Success, tr("Copied", "اتنسخ"));
                }
            }
            MenuAction::Block(action, block) => {
                let Some(id) = pane else {
                    return false;
                };
                let button = match action {
                    BlockAction::CopyCommand => BlockButton::CopyCommand,
                    BlockAction::CopyOutput => BlockButton::CopyOutput,
                    BlockAction::Rerun => BlockButton::Rerun,
                    BlockAction::ExplainError => BlockButton::ExplainError,
                    BlockAction::AskAgent => BlockButton::AskAgent,
                };
                self.run_block_button(id, block, button);
            }
            MenuAction::RenameTab(index) => {
                self.set_active_tab(index);
                return self.run_command("tab.rename");
            }
            MenuAction::CloseTab(index) => return self.close_tab(index),
            MenuAction::CloseOtherTabs(index) => {
                let keep = self.ws.tabs().get(index).map(|t| t.id);
                for i in (0..self.ws.tabs().len()).rev() {
                    if self.ws.tabs().get(i).map(|t| t.id) != keep && self.close_tab(i) {
                        return true;
                    }
                }
            }
        }
        self.window.request_redraw();
        false
    }

    /// Whether a right press in pane `id` belongs to the program (mouse reporting without Shift).
    pub(super) fn right_click_goes_to_program(&self, id: PaneId) -> bool {
        self.mode_of(id).intersects(TermMode::MOUSE_MODE) && !self.modifiers.shift_key()
    }
}
