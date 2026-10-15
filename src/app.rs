use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use alacritty_terminal::index::{Direction, Side};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{CursorIcon, UserAttentionType, WindowAttributes, WindowId};

use crate::blink::{BlinkState, BLINK_INTERVAL};
use crate::config::TwittyConfig;
use crate::input::{
    encode_mouse, encode_paste, handle_key, key_shortcut, terminal_key, InputAction,
};
use crate::layout::{
    dividers, layout, neighbor, ratio_at, set_ratio, tab_rects, Divider, FocusDir, Node, PaneId,
    Rect, SplitDir,
};
use crate::pane::Pane;
use crate::pty::Pty;
use crate::renderer::{PaneFrame, Renderer, TabLabel};
use crate::terminal::Terminal;

const WINDOW_TITLE: &str = "Twitty · RTL Terminal";
const DEFAULT_FONT_SIZE: f32 = 14.5;
const LINES_PER_WHEEL_NOTCH: f64 = 3.0;
/// Clicks closer together than this (on the same cell) count as double/triple clicks.
const MULTI_CLICK: Duration = Duration::from_millis(400);
/// How long a copied selection stays highlighted before it is cleared.
const COPIED_HIGHLIGHT: Duration = Duration::from_millis(350);
/// A pane is only split when both halves keep at least this many columns / rows.
const MIN_SPLIT_COLS: usize = 24;
const MIN_SPLIT_ROWS: usize = 10;

#[derive(Debug)]
pub enum AppEvent {
    PtyData(PaneId, Vec<u8>),
    Terminal(PaneId, TermEvent),
    PtyExit(PaneId),
    /// Clear the highlight of a copied selection, unless a newer selection was started.
    ClearSelection(PaneId, u64),
}

/// A tab: a tree of split panes and the one that has keyboard focus.
struct Tab {
    root: Node,
    focused: PaneId,
}

/// What lies under the pointer.
enum Hit {
    Tab(usize),
    TabBar,
    Divider(Divider),
    Pane(PaneId),
    Nothing,
}

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    panes: HashMap<PaneId, Pane>,
    tabs: Vec<Tab>,
    active: usize,
    next_pane_id: u64,
    renderer: Option<Renderer>,
    modifiers: ModifiersState,
    /// Last pointer position in physical pixels.
    pointer: (f64, f64),
    /// The pane a pressed mouse button belongs to until it is released.
    capture: Option<PaneId>,
    /// The pane the pointer last moved over.
    hover_pane: Option<PaneId>,
    /// A divider is being dragged.
    drag_divider: Option<Divider>,
    cursor_icon: CursorIcon,
    /// Kept alive for the whole session: on Linux, copied text is served by this object and
    /// would vanish if it were dropped right after copying.
    clipboard: Option<arboard::Clipboard>,
    config: TwittyConfig,
    config_dirty: bool,
    needs_redraw: bool,
    /// An IME composition (preedit) is on screen.
    preedit_active: bool,
    cursor_blink: BlinkState,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        Self {
            proxy,
            panes: HashMap::new(),
            tabs: Vec::new(),
            active: 0,
            next_pane_id: 0,
            renderer: None,
            modifiers: ModifiersState::empty(),
            pointer: (0.0, 0.0),
            capture: None,
            hover_pane: None,
            drag_divider: None,
            cursor_icon: CursorIcon::Default,
            clipboard: None,
            config: TwittyConfig::load(),
            config_dirty: false,
            needs_redraw: false,
            preedit_active: false,
            cursor_blink: BlinkState::new(BLINK_INTERVAL),
        }
    }

    // ---- panes, tabs and layout ----

    fn alloc_pane_id(&mut self) -> PaneId {
        self.next_pane_id += 1;
        PaneId(self.next_pane_id)
    }

    /// A terminal and shell for pane `id` with the given grid size.
    fn spawn_pane(&self, id: PaneId, cols: usize, rows: usize) -> Option<Pane> {
        let proxy = self.proxy.clone();
        let mut terminal = Terminal::with_events(cols, rows, move |event| {
            let _ = proxy.send_event(AppEvent::Terminal(id, event));
        });
        terminal.set_default_cursor_blink(self.config.cursor_blink);
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        let pty = match Pty::spawn(
            cols as u16,
            rows as u16,
            move |data| {
                let _ = proxy.send_event(AppEvent::PtyData(id, data));
            },
            move || {
                let _ = proxy_exit.send_event(AppEvent::PtyExit(id));
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to spawn PTY: {:?}", e);
                return None;
            }
        };
        Some(Pane::new(
            id,
            Arc::new(Mutex::new(terminal)),
            Some(pty),
            (cols, rows),
        ))
    }

    fn focused_id(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Run `f` on the terminal of pane `id`.
    fn term<R>(&self, id: PaneId, f: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        let pane = self.panes.get(&id)?;
        let mut term = pane.terminal.lock().ok()?;
        Some(f(&mut term))
    }

    fn mode_of(&self, id: PaneId) -> TermMode {
        self.panes.get(&id).map_or(TermMode::NONE, Pane::mode)
    }

    fn write_to(&self, id: PaneId, bytes: &[u8]) {
        if let Some(pane) = self.panes.get(&id) {
            pane.write(bytes);
        }
    }

    fn content_rect(&self, tab_count: usize) -> Option<Rect> {
        self.renderer.as_ref().map(|r| r.content_rect(tab_count))
    }

    fn gap(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, Renderer::divider_gap)
    }

    /// Where each pane of the active tab goes.
    fn active_layout(&self) -> Vec<(PaneId, Rect)> {
        let (Some(tab), Some(rect)) = (
            self.tabs.get(self.active),
            self.content_rect(self.tabs.len()),
        ) else {
            return Vec::new();
        };
        layout(&tab.root, rect, self.gap())
    }

    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        self.active_layout()
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, r)| r)
    }

    fn pane_at(&self, x: f64, y: f64) -> Option<PaneId> {
        self.active_layout()
            .into_iter()
            .find(|(_, r)| r.contains(x, y))
            .map(|(id, _)| id)
    }

    fn hit_test(&self, x: f64, y: f64) -> Hit {
        let Some(r) = self.renderer.as_ref() else {
            return Hit::Nothing;
        };
        let n = self.tabs.len();
        if y < r.tab_bar_height(n) as f64 {
            let max_tab = r.char_width * 28.0;
            let tabs = tab_rects(n, r.config.width as f32, r.line_height, max_tab);
            return match tabs.iter().position(|t| t.contains(x, y)) {
                Some(i) => Hit::Tab(i),
                None => Hit::TabBar,
            };
        }
        if let (Some(tab), Some(content)) = (self.tabs.get(self.active), self.content_rect(n)) {
            // The line is thin, so grab it from a few pixels either side.
            let slop = (r.scale * 3.0).max(2.0);
            for d in dividers(&tab.root, content, self.gap()) {
                let grab = Rect::new(
                    d.rect.x - slop,
                    d.rect.y - slop,
                    d.rect.w + slop * 2.0,
                    d.rect.h + slop * 2.0,
                );
                if grab.contains(x, y) {
                    return Hit::Divider(d);
                }
            }
        }
        match self.pane_at(x, y) {
            Some(id) => Hit::Pane(id),
            None => Hit::Nothing,
        }
    }

    /// Give every pane of every tab the grid its rectangle allows.
    fn apply_layout(&mut self) {
        let Some(content) = self.content_rect(self.tabs.len()) else {
            return;
        };
        let gap = self.gap();
        let mut sizes = Vec::new();
        if let Some(r) = self.renderer.as_ref() {
            for tab in &self.tabs {
                for (id, rect) in layout(&tab.root, content, gap) {
                    sizes.push((id, r.compute_grid_size(rect)));
                }
            }
        }
        for (id, (cols, rows)) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                pane.resize(cols, rows);
            }
        }
        self.request_redraw();
    }

    fn new_tab(&mut self) {
        let Some(content) = self.content_rect(self.tabs.len() + 1) else {
            return;
        };
        let Some((cols, rows)) = self.renderer.as_ref().map(|r| r.compute_grid_size(content))
        else {
            return;
        };
        let id = self.alloc_pane_id();
        let Some(pane) = self.spawn_pane(id, cols, rows) else {
            return;
        };
        let previous = self.focused_id();
        self.panes.insert(id, pane);
        self.tabs.push(Tab {
            root: Node::Leaf(id),
            focused: id,
        });
        self.active = self.tabs.len() - 1;
        self.capture = None;
        self.drag_divider = None;
        // A second tab shows the tab bar, which shrinks every pane.
        self.apply_layout();
        self.focus_changed(previous, id);
    }

    fn split_focused(&mut self, dir: SplitDir) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let focused = tab.focused;
        let Some((cols, rows)) = self.panes.get(&focused).map(|p| p.size) else {
            return;
        };
        let fits = match dir {
            SplitDir::Horizontal => cols >= MIN_SPLIT_COLS * 2,
            SplitDir::Vertical => rows >= MIN_SPLIT_ROWS * 2,
        };
        if !fits {
            return;
        }
        let mut root = tab.root.clone();
        let new_id = self.alloc_pane_id();
        root.split(focused, new_id, dir);
        let Some(content) = self.content_rect(self.tabs.len()) else {
            return;
        };
        let rect = layout(&root, content, self.gap())
            .into_iter()
            .find(|(id, _)| *id == new_id)
            .map(|(_, r)| r);
        let Some((cols, rows)) = rect
            .zip(self.renderer.as_ref())
            .map(|(rect, r)| r.compute_grid_size(rect))
        else {
            return;
        };
        let Some(pane) = self.spawn_pane(new_id, cols, rows) else {
            return;
        };
        self.panes.insert(new_id, pane);
        self.tabs[self.active].root = root;
        self.tabs[self.active].focused = new_id;
        self.apply_layout();
        self.focus_changed(Some(focused), new_id);
    }

    /// Keyboard focus moved from `old` to `new`: tell the programs, restart the cursor blink and
    /// show the new pane's title.
    fn focus_changed(&mut self, old: Option<PaneId>, new: PaneId) {
        if old == Some(new) {
            return;
        }
        if self.renderer.as_ref().is_some_and(|r| r.focused) {
            if let Some(old) = old {
                if self.mode_of(old).contains(TermMode::FOCUS_IN_OUT) {
                    self.write_to(old, b"\x1b[O");
                }
            }
            if self.mode_of(new).contains(TermMode::FOCUS_IN_OUT) {
                self.write_to(new, b"\x1b[I");
            }
        }
        self.cursor_blink.reset(Instant::now());
        self.refresh_window_title();
        self.request_redraw();
    }

    fn focus_pane(&mut self, id: PaneId) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.focused == id || !tab.root.contains(id) {
            return;
        }
        let old = tab.focused;
        tab.focused = id;
        self.focus_changed(Some(old), id);
    }

    fn set_active_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active {
            return;
        }
        let old = self.focused_id();
        self.active = index;
        self.capture = None;
        self.drag_divider = None;
        if let Some(new) = self.focused_id() {
            self.focus_changed(old, new);
        }
    }

    fn cycle_tab(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.set_active_tab((self.active as isize + step).rem_euclid(n) as usize);
        }
    }

    fn focus_direction(&mut self, dir: FocusDir) {
        let Some(from) = self.focused_id() else {
            return;
        };
        if let Some(to) = neighbor(&self.active_layout(), from, dir) {
            self.focus_pane(to);
        }
    }

    /// Close one pane. Returns true when it was the last one and the window should close.
    fn close_pane(&mut self, id: PaneId) -> bool {
        let Some(ti) = self.tabs.iter().position(|t| t.root.contains(id)) else {
            return false;
        };
        let previous = self.focused_id();
        let tab = self.tabs.remove(ti);
        let leaves = tab.root.leaves();
        let pos = leaves.iter().position(|l| *l == id).unwrap_or(0);
        match tab.root.remove(id) {
            Some(root) => {
                // Focus the pane that was next to the closed one in reading order.
                let focused = if tab.focused == id {
                    let near = if pos > 0 { pos - 1 } else { pos + 1 };
                    leaves.get(near).copied().unwrap_or(leaves[0])
                } else {
                    tab.focused
                };
                self.tabs.insert(ti, Tab { root, focused });
            }
            None => {
                if ti < self.active {
                    self.active -= 1;
                }
            }
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        // Dropping the pane ends its shell.
        self.panes.remove(&id);
        if let Some(r) = self.renderer.as_mut() {
            r.forget_pane(id);
        }
        if self.capture == Some(id) {
            self.capture = None;
        }
        if self.hover_pane == Some(id) {
            self.hover_pane = None;
        }
        self.drag_divider = None;
        let Some(now) = self.focused_id() else {
            return true;
        };
        self.apply_layout();
        self.focus_changed(previous.filter(|p| self.panes.contains_key(p)), now);
        self.refresh_window_title();
        false
    }

    fn tab_title(&self, index: usize) -> String {
        self.tabs
            .get(index)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| format!("Tab {}", index + 1))
    }

    /// The window title follows the focused pane.
    fn refresh_window_title(&self) {
        let title = self
            .focused_id()
            .and_then(|id| self.panes.get(&id))
            .and_then(|p| p.title.clone())
            .unwrap_or_else(|| WINDOW_TITLE.to_owned());
        if let Some(ref r) = self.renderer {
            r.window.set_title(&title);
        }
    }

    // ---- window-level helpers ----

    fn update_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.preedit_active = preedit.is_some();
        if let Some(ref mut r) = self.renderer {
            r.set_preedit(preedit);
        }
        self.request_redraw();
    }

    fn request_redraw(&mut self) {
        if let Some(ref r) = self.renderer {
            if !self.needs_redraw {
                self.needs_redraw = true;
                r.window.request_redraw();
            }
        }
    }

    /// Any input sent to the shell snaps the view back to the live screen.
    fn scroll_to_bottom(&mut self, id: PaneId) {
        let scrolled = self
            .term(id, |term| {
                let scrolled = term.display_offset() > 0;
                if scrolled {
                    term.scroll_to_bottom();
                }
                scrolled
            })
            .unwrap_or(false);
        if scrolled {
            self.request_redraw();
        }
    }

    fn update_font_size(&mut self, new_size: f32) {
        let clamped = new_size.clamp(8.0, 48.0);
        self.config.font_size = clamped;
        // Written once on exit instead of on every zoom step.
        self.config_dirty = true;
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(clamped);
        }
        self.apply_layout();
    }

    fn paste_clipboard(&mut self, id: PaneId) {
        let Some(Ok(text)) = self.clipboard().map(|c| c.get_text()) else {
            return;
        };
        let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
        self.write_to(id, &encode_paste(&text, bracketed));
        self.cursor_blink.reset(Instant::now());
        self.scroll_to_bottom(id);
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }

    fn set_clipboard(&mut self, text: String) {
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text);
        }
    }

    fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            if let Some(ref r) = self.renderer {
                r.window.set_cursor(icon);
            }
        }
    }

    // ---- selection, links, mouse ----

    fn clear_selection(&mut self, id: PaneId) {
        let had = self
            .term(id, |term| {
                let had = term.has_selection();
                if had {
                    term.clear_selection();
                }
                had
            })
            .unwrap_or(false);
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.selecting = false;
        }
        if had {
            self.request_redraw();
        }
    }

    /// Copy the selection to the clipboard. Returns false when nothing is selected.
    fn copy_selection(&mut self, id: PaneId) -> bool {
        let text = self.term(id, |t| t.selection_text()).flatten();
        let Some(text) = text else { return false };
        self.set_clipboard(text);
        true
    }

    fn start_selection(&mut self, id: PaneId) {
        let now = Instant::now();
        let side = self.panes.get(&id).map_or(Side::Left, |p| p.mouse_side);
        let Some(pane) = self.panes.get_mut(&id) else {
            return;
        };
        let (col, row) = (pane.mouse_col, pane.mouse_row);
        pane.click_count = match pane.last_click {
            Some((t, c, r)) if now - t < MULTI_CLICK && (c, r) == (col, row) => {
                pane.click_count % 3 + 1
            }
            _ => 1,
        };
        pane.last_click = Some((now, col, row));
        pane.selection_generation += 1;
        let clicks = pane.click_count;
        if let Ok(mut term) = pane.terminal.lock() {
            term.start_selection(col, row, side, clicks);
        }
        pane.selecting = true;
        self.request_redraw();
    }

    /// Mouse released: copy what was selected, flash it briefly, then clear the highlight.
    fn finish_selection(&mut self, id: PaneId) {
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.selecting = false;
        }
        if !self.copy_selection(id) {
            self.clear_selection(id);
            return;
        }
        let Some(generation) = self.panes.get(&id).map(|p| p.selection_generation) else {
            return;
        };
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(COPIED_HIGHLIGHT);
            let _ = proxy.send_event(AppEvent::ClearSelection(id, generation));
        });
    }

    /// Underline the link under the pointer while Ctrl is held, and show a pointer cursor.
    fn update_link_hover(&mut self, id: PaneId) {
        let Some((col, row)) = self.panes.get(&id).map(|p| (p.mouse_col, p.mouse_row)) else {
            return;
        };
        let ctrl = self.modifiers.control_key();
        let result = self.term(id, |t| {
            let link = if ctrl { t.link_at(col, row) } else { None };
            let hover = link.as_ref().map(|l| (l.row, l.cols.clone()));
            (t.set_link_hover(hover), link.is_some())
        });
        if let Some((true, over_link)) = result {
            self.set_cursor_icon(if over_link {
                CursorIcon::Pointer
            } else {
                CursorIcon::Text
            });
            self.request_redraw();
        }
    }

    /// The pointer left a pane: its link underline goes away.
    fn clear_link_hover(&mut self, id: PaneId) {
        if self.term(id, |t| t.set_link_hover(None)) == Some(true) {
            self.request_redraw();
        }
    }

    fn send_mouse(&self, id: PaneId, button: u8, pressed: bool) {
        let Some(pane) = self.panes.get(&id) else {
            return;
        };
        let sgr = pane.mode().contains(TermMode::SGR_MOUSE);
        let seq = encode_mouse(
            button,
            pane.mouse_col,
            pane.mouse_row,
            pressed,
            self.modifiers,
            sgr,
        );
        pane.write(&seq);
    }

    /// A mouse button was pressed or released inside pane `id`.
    fn handle_mouse_button(&mut self, id: PaneId, button: MouseButton, pressed: bool) {
        let mode = self.mode_of(id);
        let code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };
        // Ctrl+Click opens a link and takes priority over mouse reporting.
        if button == MouseButton::Left {
            let Some((col, row, link_press)) = self
                .panes
                .get(&id)
                .map(|p| (p.mouse_col, p.mouse_row, p.link_press))
            else {
                return;
            };
            if pressed && self.modifiers.control_key() {
                let link = self.term(id, |t| t.link_at(col, row)).flatten();
                if let Some(link) = link {
                    crate::links::open_url(&link.uri);
                    if let Some(p) = self.panes.get_mut(&id) {
                        p.link_press = true;
                    }
                    return;
                }
            } else if !pressed && link_press {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.link_press = false;
                }
                return;
            }
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.mouse_button_down = if pressed { Some(code) } else { None };
        }

        // Shift bypasses mouse reporting, as in xterm and Windows Terminal.
        if mode.intersects(TermMode::MOUSE_MODE) && !self.modifiers.shift_key() {
            self.send_mouse(id, code, pressed);
        } else if button == MouseButton::Left {
            if pressed {
                self.start_selection(id);
            } else if self.panes.get(&id).is_some_and(|p| p.selecting) {
                self.finish_selection(id);
            }
        } else if pressed && button == MouseButton::Middle {
            self.paste_clipboard(id);
        }
    }

    fn drag_divider_to(&mut self, divider: &Divider) {
        let pos = match divider.dir {
            SplitDir::Horizontal => self.pointer.0,
            SplitDir::Vertical => self.pointer.1,
        };
        let ratio = ratio_at(divider, pos, self.gap());
        if let Some(tab) = self.tabs.get_mut(self.active) {
            if set_ratio(&mut tab.root, &divider.path, ratio) {
                self.apply_layout();
            }
        }
    }

    // ---- terminal events ----

    fn handle_terminal_event(&mut self, id: PaneId, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => self.write_to(id, text.as_bytes()),
            TermEvent::Title(title) => {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.title = Some(title);
                }
                self.refresh_window_title();
                self.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.title = None;
                }
                self.refresh_window_title();
                self.request_redraw();
            }
            TermEvent::Bell => {
                if let Some(ref r) = self.renderer {
                    r.window
                        .request_user_attention(Some(UserAttentionType::Informational));
                }
            }
            TermEvent::ColorRequest(index, format) => {
                // OSC 4/10/11/12 queries (used by vim, fzf, delta to detect dark themes).
                let Some(ref r) = self.renderer else { return };
                let rgba = match index {
                    256 => r.palette.foreground,
                    257 => r.palette.background,
                    258 => r.palette.cursor,
                    i if i < 256 => r.palette.resolve(AnsiColor::Indexed(i as u8), false),
                    _ => r.palette.foreground,
                };
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                let rgb = Rgb {
                    r: to8(rgba.r),
                    g: to8(rgba.g),
                    b: to8(rgba.b),
                };
                self.write_to(id, format(rgb).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let Some(ref r) = self.renderer else { return };
                let Some((cols, rows)) = self.panes.get(&id).map(|p| p.size) else {
                    return;
                };
                let size = WindowSize {
                    num_lines: rows as u16,
                    num_cols: cols as u16,
                    cell_width: r.char_width as u16,
                    cell_height: r.line_height as u16,
                };
                self.write_to(id, format(size).as_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                // OSC 52 copy (e.g. from tmux/vim over SSH).
                self.set_clipboard(text);
            }
            // OSC 52 reads are refused: any program could silently exfiltrate the clipboard.
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}

impl App {
    /// Text of the search bar for the current query and match position.
    fn search_bar_text(&self, id: PaneId, query: &str) -> String {
        let status = self
            .term(id, |term| {
                if query.is_empty() {
                    String::new()
                } else if !term.has_search_match() {
                    "  no matches".to_owned()
                } else {
                    term.search_position()
                        .map_or(String::new(), |(i, n)| format!("  {i}/{n}"))
                }
            })
            .unwrap_or_default();
        format!("Find: {query}{status}")
    }

    fn open_search(&mut self, id: PaneId) {
        self.clear_selection(id);
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.search_query = Some(String::new());
        }
        self.request_redraw();
    }

    fn close_search(&mut self, id: PaneId) {
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.search_query = None;
        }
        self.term(id, |t| t.clear_search());
        self.request_redraw();
    }

    /// Run the search for the current query in `direction` and update the bar.
    fn run_search(&mut self, id: PaneId, direction: Direction) {
        let Some(query) = self.panes.get(&id).and_then(|p| p.search_query.clone()) else {
            return;
        };
        self.term(id, |t| t.search(&query, direction));
        self.request_redraw();
    }

    /// Key press while the search bar is open. Nothing reaches the PTY.
    fn handle_search_key(&mut self, id: PaneId, event: &KeyEvent) {
        if !event.state.is_pressed() {
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Escape) => self.close_search(id),
            Key::Named(NamedKey::Enter) => {
                let direction = if self.modifiers.shift_key() {
                    Direction::Right
                } else {
                    Direction::Left
                };
                self.run_search(id, direction);
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(query) = self
                    .panes
                    .get_mut(&id)
                    .and_then(|p| p.search_query.as_mut())
                {
                    query.pop();
                }
                self.run_search(id, Direction::Left);
            }
            _ => {
                if self.modifiers.control_key() || self.modifiers.alt_key() {
                    return;
                }
                let Some(text) = event.text.as_deref() else {
                    return;
                };
                let text: String = text.chars().filter(|c| !c.is_control()).collect();
                if text.is_empty() {
                    return;
                }
                if let Some(query) = self
                    .panes
                    .get_mut(&id)
                    .and_then(|p| p.search_query.as_mut())
                {
                    query.push_str(&text);
                }
                self.run_search(id, Direction::Left);
            }
        }
    }

    /// Run a shortcut action on the focused pane. Returns true when the window should close.
    fn run_action(&mut self, id: PaneId, action: InputAction) -> bool {
        match action {
            InputAction::Bytes(bytes) => {
                self.clear_selection(id);
                self.write_to(id, &bytes);
                self.cursor_blink.reset(Instant::now());
                self.scroll_to_bottom(id);
            }
            InputAction::ZoomIn => {
                let cur = self.config.font_size;
                self.update_font_size(cur + 1.0);
            }
            InputAction::ZoomOut => {
                let cur = self.config.font_size;
                self.update_font_size(cur - 1.0);
            }
            InputAction::ZoomReset => self.update_font_size(DEFAULT_FONT_SIZE),
            InputAction::Paste => self.paste_clipboard(id),
            InputAction::Copy => {
                self.copy_selection(id);
            }
            InputAction::Search => self.open_search(id),
            InputAction::NewTab => self.new_tab(),
            InputAction::ClosePane => return self.close_pane(id),
            InputAction::NextTab => self.cycle_tab(1),
            InputAction::PrevTab => self.cycle_tab(-1),
            InputAction::SplitRight => self.split_focused(SplitDir::Horizontal),
            InputAction::SplitDown => self.split_focused(SplitDir::Vertical),
            InputAction::Focus(dir) => self.focus_direction(dir),
        }
        false
    }

    /// Snapshot everything the renderer draws for the active tab.
    fn frame(&self) -> (Vec<TabLabel>, Vec<PaneFrame>, Vec<Rect>) {
        let tabs = (0..self.tabs.len())
            .map(|i| TabLabel {
                title: self.tab_title(i),
                active: i == self.active,
            })
            .collect();
        let focused = self.focused_id();
        let panes = self
            .active_layout()
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = self.panes.get(&id)?;
                let (lines, cursor) = pane.terminal.lock().ok()?.snapshot();
                let status = pane
                    .search_query
                    .as_deref()
                    .map(|q| self.search_bar_text(id, q));
                Some(PaneFrame {
                    id,
                    rect,
                    lines,
                    cursor,
                    focused: focused == Some(id),
                    status,
                })
            })
            .collect();
        let dividers = match (
            self.tabs.get(self.active),
            self.content_rect(self.tabs.len()),
        ) {
            (Some(tab), Some(content)) => dividers(&tab.root, content, self.gap())
                .into_iter()
                .map(|d| d.rect)
                .collect(),
            _ => Vec::new(),
        };
        (tabs, panes, dividers)
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let window_attrs = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0))
            .with_transparent(true);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("Failed to create window: {:?}", e);
                event_loop.exit();
                return;
            }
        };

        window.set_ime_allowed(true);
        window.set_ime_purpose(winit::window::ImePurpose::Terminal);

        let mut renderer =
            match pollster::block_on(Renderer::new(window.clone(), self.config.font_size)) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Failed to initialize renderer: {:?}", e);
                    event_loop.exit();
                    return;
                }
            };

        renderer.set_font_family(self.config.font_family.as_deref());
        renderer.set_opacity(self.config.background_opacity);
        renderer.cursor_style = self.config.cursor_style.clone();
        renderer.palette = self.config.palette();
        self.renderer = Some(renderer);

        self.new_tab();
        if self.tabs.is_empty() {
            self.renderer = None;
            event_loop.exit();
            return;
        }
        window.request_redraw();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyData(id, data) => {
                let Some(pane) = self.panes.get(&id) else {
                    return;
                };
                let mut moved = false;
                if let Ok(mut term) = pane.terminal.lock() {
                    let before = term.cursor_position();
                    term.process_bytes(&data);
                    moved = term.cursor_position() != before;
                }
                // Output that moves the cursor keeps it solid while text is appearing.
                if moved && self.focused_id() == Some(id) {
                    self.cursor_blink.reset(Instant::now());
                }
                if self
                    .tabs
                    .get(self.active)
                    .is_some_and(|t| t.root.contains(id))
                {
                    self.request_redraw();
                }
            }
            AppEvent::Terminal(id, event) => self.handle_terminal_event(id, event),
            // The shell of a pane exited (`exit`, crash, or closed by the OS): close that pane,
            // and the window once nothing is left.
            AppEvent::PtyExit(id) => {
                if self.close_pane(id) {
                    event_loop.exit();
                }
            }
            AppEvent::ClearSelection(id, generation) => {
                let current = self
                    .panes
                    .get(&id)
                    .is_some_and(|p| p.selection_generation == generation && !p.selecting);
                if current {
                    self.clear_selection(id);
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let (blinking, shown) = self
            .focused_id()
            .and_then(|id| {
                self.term(id, |t| {
                    (
                        t.cursor_blinking(),
                        t.mode().contains(TermMode::SHOW_CURSOR),
                    )
                })
            })
            .unwrap_or((false, false));
        self.cursor_blink.set_enabled(now, blinking && shown);
        self.cursor_blink.tick(now);

        let visible = self.cursor_blink.visible();
        if self
            .renderer
            .as_ref()
            .is_some_and(|r| r.cursor_blink_on != visible)
        {
            self.request_redraw();
        }
        event_loop.set_control_flow(match self.cursor_blink.next_deadline() {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.config_dirty {
            self.config.save();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(new_size) => {
                if let Some(ref mut r) = self.renderer {
                    r.resize(new_size.width, new_size.height);
                }
                self.apply_layout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(ref mut r) = self.renderer {
                    r.set_scale_factor(scale_factor as f32);
                }
                self.apply_layout();
            }
            WindowEvent::Focused(focused) => {
                // The release of a drag that ends outside the window may never arrive.
                if !focused {
                    let selecting: Vec<PaneId> = self
                        .panes
                        .values()
                        .filter(|p| p.selecting)
                        .map(|p| p.id)
                        .collect();
                    for id in selecting {
                        if let Some(p) = self.panes.get_mut(&id) {
                            p.mouse_button_down = None;
                        }
                        self.finish_selection(id);
                    }
                    self.capture = None;
                    self.drag_divider = None;
                }
                self.cursor_blink.set_focused(Instant::now(), focused);
                if let Some(ref mut r) = self.renderer {
                    r.set_focused(focused);
                    r.window.request_redraw();
                }
                if let Some(id) = self.focused_id() {
                    if self.mode_of(id).contains(TermMode::FOCUS_IN_OUT) {
                        self.write_to(id, if focused { b"\x1b[I" } else { b"\x1b[O" });
                    }
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                if let Some(id) = self.hover_pane.or(self.focused_id()) {
                    self.update_link_hover(id);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = (position.x, position.y);
                if let Some(divider) = self.drag_divider.clone() {
                    self.drag_divider_to(&divider);
                    return;
                }
                let hit = self.hit_test(position.x, position.y);
                match &hit {
                    Hit::Divider(d) if self.capture.is_none() => {
                        self.set_cursor_icon(match d.dir {
                            SplitDir::Horizontal => CursorIcon::ColResize,
                            SplitDir::Vertical => CursorIcon::RowResize,
                        });
                    }
                    _ if matches!(
                        self.cursor_icon,
                        CursorIcon::ColResize | CursorIcon::RowResize
                    ) =>
                    {
                        self.set_cursor_icon(CursorIcon::Default)
                    }
                    _ => {}
                }
                let target = self.capture.or(match hit {
                    Hit::Pane(id) => Some(id),
                    _ => None,
                });
                if self.hover_pane != target {
                    if let Some(old) = self.hover_pane {
                        self.clear_link_hover(old);
                    }
                    self.hover_pane = target;
                }
                let Some(id) = target else { return };
                let Some(rect) = self.pane_rect(id) else {
                    return;
                };
                let Some(ref r) = self.renderer else { return };
                let (col, row, side) = r.pixel_to_visual_cell(id, rect, position.x, position.y);
                let Some(pane) = self.panes.get_mut(&id) else {
                    return;
                };
                pane.mouse_side = side;
                if pane.selecting {
                    if let Ok(mut term) = pane.terminal.lock() {
                        term.update_selection(col, row, side);
                    }
                    self.request_redraw();
                }
                let Some(pane) = self.panes.get_mut(&id) else {
                    return;
                };
                if (col, row) == (pane.mouse_col, pane.mouse_row) {
                    return;
                }
                pane.mouse_col = col;
                pane.mouse_row = row;
                let down = pane.mouse_button_down;
                self.update_link_hover(id);

                // 1003 reports every motion, 1002 only while a button is held.
                let mode = self.mode_of(id);
                let button = match down {
                    Some(b) if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) => {
                        Some(b)
                    }
                    None if mode.contains(TermMode::MOUSE_MOTION) => Some(3),
                    _ => None,
                };
                if let Some(b) = button {
                    self.send_mouse(id, 32 + b, true);
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = state == ElementState::Pressed;
                if !pressed && self.drag_divider.take().is_some() {
                    return;
                }
                if pressed {
                    match self.hit_test(self.pointer.0, self.pointer.1) {
                        Hit::Tab(i) => {
                            if button == MouseButton::Left {
                                self.set_active_tab(i);
                            }
                            return;
                        }
                        Hit::Divider(d) => {
                            if button == MouseButton::Left {
                                self.drag_divider = Some(d);
                            }
                            return;
                        }
                        Hit::TabBar | Hit::Nothing => return,
                        Hit::Pane(id) => {
                            self.focus_pane(id);
                            self.capture = Some(id);
                        }
                    }
                }
                let Some(id) = self.capture else { return };
                self.handle_mouse_button(id, button, pressed);
                if !pressed {
                    self.capture = None;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let line_height = self
                    .renderer
                    .as_ref()
                    .map_or(20.0, |r| r.line_height as f64);
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64 * LINES_PER_WHEEL_NOTCH,
                    MouseScrollDelta::PixelDelta(p) => p.y / line_height,
                };

                if self.modifiers.control_key() && self.modifiers.shift_key() {
                    // Ctrl + Shift + wheel: background opacity.
                    let step = if lines > 0.0 { 0.05 } else { -0.05 };
                    let opacity = (self.config.background_opacity + step).clamp(0.2, 1.0);
                    self.config.background_opacity = opacity;
                    self.config_dirty = true;
                    if let Some(ref mut r) = self.renderer {
                        r.set_opacity(opacity);
                    }
                    self.request_redraw();
                    return;
                }
                if self.modifiers.control_key() {
                    // Ctrl + wheel: zoom one step per notch.
                    if lines > 0.0 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur + 1.0);
                    } else if lines < 0.0 {
                        let cur = self.config.font_size;
                        self.update_font_size(cur - 1.0);
                    }
                    return;
                }

                // The wheel scrolls the pane under the pointer.
                let Some(id) = self.pane_at(self.pointer.0, self.pointer.1) else {
                    return;
                };
                // Accumulate so trackpads scroll smoothly instead of 3 lines per tiny event.
                let Some(pane) = self.panes.get_mut(&id) else {
                    return;
                };
                pane.scroll_accum += lines;
                let steps = pane.scroll_accum.trunc() as i32;
                if steps == 0 {
                    return;
                }
                pane.scroll_accum -= steps as f64;

                let mode = pane.mode();
                if mode.intersects(TermMode::MOUSE_MODE) {
                    let btn = if steps > 0 { 64 } else { 65 };
                    for _ in 0..steps.unsigned_abs() {
                        self.send_mouse(id, btn, true);
                    }
                } else if mode.contains(TermMode::ALT_SCREEN)
                    && mode.contains(TermMode::ALTERNATE_SCROLL)
                {
                    // Alternate screen without mouse mode (less, man): send arrow keys.
                    let key: &[u8] = match (steps > 0, mode.contains(TermMode::APP_CURSOR)) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    self.write_to(id, &key.repeat(steps.unsigned_abs() as usize));
                } else if !mode.contains(TermMode::ALT_SCREEN) {
                    self.term(id, |t| t.scroll_display(steps));
                    self.request_redraw();
                }
            }
            WindowEvent::Ime(ime) => match ime {
                Ime::Enabled => {}
                Ime::Disabled => self.update_preedit(None),
                Ime::Preedit(text, cursor) => {
                    self.update_preedit(crate::ime::normalize_preedit(text, cursor))
                }
                Ime::Commit(text) => {
                    self.update_preedit(None);
                    let Some(id) = self.focused_id() else { return };
                    // While searching, composed text belongs to the query, not the shell.
                    if let Some(query) = self
                        .panes
                        .get_mut(&id)
                        .and_then(|p| p.search_query.as_mut())
                    {
                        query.push_str(&text);
                        self.run_search(id, Direction::Left);
                        return;
                    }
                    self.clear_selection(id);
                    self.write_to(id, text.as_bytes());
                    self.scroll_to_bottom(id);
                }
            },
            WindowEvent::KeyboardInput { .. }
                if !crate::ime::should_forward_key(self.preedit_active) => {}
            WindowEvent::KeyboardInput { event, .. } => {
                let Some(id) = self.focused_id() else { return };
                let mode = self.mode_of(id);
                let mut action = handle_key(&event, self.modifiers, mode);
                // With a single pane / tab these shortcuts have nothing to do, so the key goes
                // to the program instead (Alt+Left in fish, Ctrl+PageUp in vim...).
                // Decided from the shortcut itself, so the release follows its press.
                let idle = match key_shortcut(&event, self.modifiers) {
                    Some(InputAction::Focus(_)) => self
                        .tabs
                        .get(self.active)
                        .is_some_and(|t| matches!(t.root, Node::Leaf(_))),
                    Some(InputAction::NextTab | InputAction::PrevTab) => self.tabs.len() < 2,
                    _ => false,
                };
                if idle {
                    action = terminal_key(&event, self.modifiers, mode).map(InputAction::Bytes);
                }
                // Layout shortcuts keep working while the search bar has the keyboard.
                let layout_action = matches!(
                    action,
                    Some(
                        InputAction::NewTab
                            | InputAction::ClosePane
                            | InputAction::NextTab
                            | InputAction::PrevTab
                            | InputAction::SplitRight
                            | InputAction::SplitDown
                            | InputAction::Focus(_)
                    )
                );
                if !layout_action
                    && self
                        .panes
                        .get(&id)
                        .is_some_and(|p| p.search_query.is_some())
                {
                    self.handle_search_key(id, &event);
                    return;
                }
                if let Some(action) = action {
                    if self.run_action(id, action) {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                self.needs_redraw = false;
                let (tabs, panes, dividers) = self.frame();
                let visible = self.cursor_blink.visible();
                if let Some(ref mut r) = self.renderer {
                    r.cursor_blink_on = visible;
                    if let Err(e) = r.render(&tabs, &panes, &dividers) {
                        eprintln!("Render error: {:?}", e);
                    }
                }
            }
            _ => {}
        }
    }
}
