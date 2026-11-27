//! Right-click menus: what a menu lists, where it opens, and which row the pointer or keyboard
//! picks. Drawing lives in `render::scene::context_menu`; the app builds the entries for what was
//! clicked (`app::context_menu`) and runs the chosen action.

use crate::layout::{PaneId, Rect};
use crate::ui::chrome::UiLanguage;

/// Logical sizes; multiply by the window scale.
pub const MENU_WIDTH: f32 = 248.0;
pub const ROW_HEIGHT: f32 = 28.0;
pub const SEPARATOR_HEIGHT: f32 = 9.0;
pub const PADDING: f32 = 4.0;
/// Distance kept from the window edges.
pub const MARGIN: f32 = 6.0;

/// What a menu row does. The app maps each to the code that runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    /// A built-in command by its stable name (`commands::action`).
    Command(&'static str),
    /// Copy the terminal selection of the pane.
    CopySelection,
    /// A command block's action (`block` is the block id in the pane).
    Block(BlockAction, u64),
    /// Tab actions by tab index.
    RenameTab(usize),
    CloseTab(usize),
    CloseOtherTabs(usize),
    /// Reconnect the SSH pane inside tab `index`.
    ReconnectSshTab(usize),
    /// Open a split connected to the same SSH host as `pane`.
    SplitSshSameHost(PaneId),
    /// Copy an SSH destination string to the clipboard.
    CopySshDestination(String),
    /// Cycle the workspace accent color tag of tab `index`.
    CycleTabColor(usize),
    /// Clear the workspace accent color tag of tab `index`.
    ClearTabColor(usize),
}

/// Actions on one command block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockAction {
    CopyCommand,
    CopyOutput,
    Rerun,
    ExplainError,
    AskAgent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    /// Shortcut shown at the trailing edge.
    pub shortcut: Option<String>,
    pub action: MenuAction,
    /// Drawn in the danger color (closing things).
    pub danger: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl MenuEntry {
    pub fn item(label: &str, action: MenuAction) -> Self {
        Self::Item(MenuItem {
            label: label.to_owned(),
            shortcut: None,
            action,
            danger: false,
        })
    }

    pub fn with_shortcut(mut self, shortcut: Option<String>) -> Self {
        if let Self::Item(item) = &mut self {
            item.shortcut = shortcut;
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let Self::Item(item) = &mut self {
            item.danger = true;
        }
        self
    }
}

/// Keys an open menu understands; the app maps keyboard events to these.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Escape,
    Next,
    Previous,
    /// Enter or Space: run the highlighted row.
    Activate,
    /// Anything else; the menu swallows it.
    Other,
}

/// Mouse buttons as the menu sees them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuButton {
    Primary,
    Secondary,
    Other,
}

/// What the app does after the menu took an input. The menu changes only itself; running an
/// action, closing and redrawing are the app's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEffect {
    /// Nothing visible changed.
    None,
    /// The highlight moved.
    Redraw,
    Close,
    /// Close the menu, then run this action.
    Run(MenuAction),
}

/// An open menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenu {
    pub entries: Vec<MenuEntry>,
    /// Where the pointer was (physical pixels).
    pub anchor: (f32, f32),
    /// Highlighted entry (an item, never a separator).
    pub selected: Option<usize>,
}

impl ContextMenu {
    /// A menu at `anchor` with separators trimmed from the ends and never doubled.
    pub fn new(entries: Vec<MenuEntry>, anchor: (f32, f32)) -> Option<Self> {
        let mut clean: Vec<MenuEntry> = Vec::with_capacity(entries.len());
        for entry in entries {
            let separator = entry == MenuEntry::Separator;
            if separator && clean.last().is_none_or(|e| *e == MenuEntry::Separator) {
                continue;
            }
            clean.push(entry);
        }
        while clean.last() == Some(&MenuEntry::Separator) {
            clean.pop();
        }
        (!clean.is_empty()).then_some(Self {
            entries: clean,
            anchor,
            selected: None,
        })
    }

    pub fn item(&self, index: usize) -> Option<&MenuItem> {
        match self.entries.get(index)? {
            MenuEntry::Item(item) => Some(item),
            MenuEntry::Separator => None,
        }
    }

    /// Move the highlight to the next (`down`) or previous item, wrapping around.
    pub fn move_selection(&mut self, down: bool) {
        let items: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.item(i).is_some())
            .collect();
        let Some(&first) = items.first() else {
            return;
        };
        let last = *items.last().unwrap_or(&first);
        self.selected = Some(
            match self
                .selected
                .and_then(|s| items.iter().position(|&i| i == s))
            {
                None if down => first,
                None => last,
                Some(pos) if down => items[(pos + 1) % items.len()],
                Some(pos) => items[(pos + items.len() - 1) % items.len()],
            },
        );
    }

    /// A key while the menu is open: arrows move, Activate runs the highlighted row, Escape
    /// closes, the rest is swallowed.
    pub fn key(&mut self, key: MenuKey) -> MenuEffect {
        match key {
            MenuKey::Escape => MenuEffect::Close,
            MenuKey::Next | MenuKey::Previous => {
                self.move_selection(key == MenuKey::Next);
                MenuEffect::Redraw
            }
            MenuKey::Activate => match self.selected.and_then(|i| self.item(i)) {
                Some(item) => MenuEffect::Run(item.action.clone()),
                None => MenuEffect::Close,
            },
            MenuKey::Other => MenuEffect::None,
        }
    }

    /// The pointer moved onto item `row` (or off the items): highlight it. Moving off keeps the
    /// last highlight so the keyboard can carry on from it.
    pub fn hover(&mut self, row: Option<usize>) -> MenuEffect {
        match row.filter(|&i| self.item(i).is_some()) {
            Some(i) if self.selected != Some(i) => {
                self.selected = Some(i);
                MenuEffect::Redraw
            }
            _ => MenuEffect::None,
        }
    }

    /// A press while the menu is open. The menu takes every press: on an item it runs it,
    /// anywhere else it closes.
    pub fn press(&self, row: Option<usize>, button: MenuButton) -> MenuEffect {
        match (row.and_then(|i| self.item(i)), button) {
            (Some(item), MenuButton::Primary | MenuButton::Secondary) => {
                MenuEffect::Run(item.action.clone())
            }
            _ => MenuEffect::Close,
        }
    }

    /// Geometry inside a `window` of physical pixels at `scale`.
    pub fn layout(&self, window: Rect, scale: f32, language: UiLanguage) -> MenuLayout {
        MenuLayout::new(self, window, scale, language)
    }
}

/// Menu geometry in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuLayout {
    pub rect: Rect,
    /// One rect per entry (separators included), in entry order.
    pub rows: Vec<Rect>,
}

impl MenuLayout {
    /// Opens down and toward the reading direction from the anchor (left in Arabic), flipping
    /// to the other side when the window edge is closer than the menu.
    pub fn new(menu: &ContextMenu, window: Rect, scale: f32, language: UiLanguage) -> Self {
        let px = |v: f32| (v * scale).round();
        let margin = px(MARGIN);
        let pad = px(PADDING);
        let heights: Vec<f32> = menu
            .entries
            .iter()
            .map(|e| match e {
                MenuEntry::Item(_) => px(ROW_HEIGHT),
                MenuEntry::Separator => px(SEPARATOR_HEIGHT),
            })
            .collect();
        let w = px(MENU_WIDTH).min((window.w - 2.0 * margin).max(0.0));
        let h = (heights.iter().sum::<f32>() + 2.0 * pad).min((window.h - 2.0 * margin).max(0.0));
        let (ax, ay) = menu.anchor;
        let (min_x, max_x) = (window.x + margin, window.right() - margin - w);
        let leftward = ax - w;
        let x = match language {
            UiLanguage::English if ax + w <= window.right() - margin => ax,
            UiLanguage::English => leftward,
            UiLanguage::Arabic if leftward >= min_x => leftward,
            UiLanguage::Arabic => ax,
        }
        .clamp(min_x, max_x.max(min_x));
        let (min_y, max_y) = (window.y + margin, window.bottom() - margin - h);
        let y = if ay + h <= window.bottom() - margin {
            ay
        } else {
            ay - h
        }
        .clamp(min_y, max_y.max(min_y));
        let rect = Rect::new(x.round(), y.round(), w, h);
        let mut top = rect.y + pad;
        let rows = heights
            .iter()
            .map(|&rh| {
                let row = Rect::new(rect.x + pad, top, (w - 2.0 * pad).max(0.0), rh);
                top += rh;
                row
            })
            .collect();
        Self { rect, rows }
    }

    /// The item under `(x, y)`, if any (separators and padding are not items).
    pub fn hit(&self, menu: &ContextMenu, x: f64, y: f64) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.contains(x, y))
            .filter(|&i| menu.item(i).is_some() && self.rows[i].bottom() <= self.rect.bottom())
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.rect.contains(x, y)
    }
}
