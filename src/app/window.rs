//! The window and its renderer. Every call into the renderer lives in this file.

use alacritty_terminal::index::Side;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, UserAttentionType, WindowAttributes};

use super::App;
use crate::config::TwittyConfig;
use crate::layout::{sidebar_layout, PaneId, Rect, SidebarPosition};
use crate::render::{PaneFrame, Renderer, TabLabel};
use crate::session::workspace::{GridGeometry, Workspace};
use crate::sidebar::{entry_title, git::BranchCache, Entry, Notice, Placement, ScrollState};
use crate::ui::hit::{hit_test, Hit, HitGeometry};
use crate::ui::settings::{clamp_font_size, step_opacity};

pub(super) const WINDOW_TITLE: &str = "Twitty · RTL Terminal";

impl GridGeometry for Window {
    fn content_rect(&self, tab_count: usize) -> Rect {
        self.layout(tab_count).0
    }

    fn gap(&self) -> f32 {
        self.gap()
    }

    fn grid_size(&self, rect: Rect) -> (usize, usize) {
        self.renderer
            .as_ref()
            .map_or((2, 1), |r| r.compute_grid_size(rect))
    }
}

/// The window, once it is open: its renderer, the pending-redraw flag and the cursor icon.
pub(super) struct Window {
    renderer: Option<Renderer>,
    needs_redraw: bool,
    cursor_icon: CursorIcon,
    sidebar: Placement,
    sidebar_width: f32,
    sidebar_scroll: ScrollState,
    branches: BranchCache,
    notifications: crate::notify::DesktopNotifier,
    /// Listening TCP ports per pane, from the last port scan.
    ports: HashMap<PaneId, Vec<u16>>,
}

impl Window {
    pub(super) fn new() -> Self {
        Self {
            renderer: None,
            needs_redraw: false,
            cursor_icon: CursorIcon::Default,
            sidebar: Placement::new(SidebarPosition::Right),
            sidebar_width: 220.0,
            sidebar_scroll: ScrollState::default(),
            branches: BranchCache::default(),
            notifications: crate::notify::DesktopNotifier::default(),
            ports: HashMap::new(),
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.renderer.is_some()
    }

    /// Create the window and its renderer. On failure the event loop is told to exit.
    pub(super) fn open(&mut self, event_loop: &ActiveEventLoop, config: &TwittyConfig) -> bool {
        let window_attrs = WindowAttributes::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 580.0))
            .with_transparent(true);

        let window = match event_loop.create_window(window_attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("failed to create window: {e:?}");
                event_loop.exit();
                return false;
            }
        };

        window.set_ime_allowed(true);
        window.set_ime_purpose(winit::window::ImePurpose::Terminal);

        let mut renderer = match pollster::block_on(Renderer::new(window, config.font_size)) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to initialize renderer: {e:?}");
                event_loop.exit();
                return false;
            }
        };

        renderer.set_font_family(config.font_family.as_deref());
        renderer.set_opacity(config.background_opacity);
        renderer.set_cursor_style(config.cursor_style.clone());
        renderer.set_palette(config.palette());
        self.sidebar = Placement::new(config.sidebar.parse().unwrap_or_default());
        self.sidebar_width = config.sidebar_width;
        self.renderer = Some(renderer);
        true
    }

    pub(super) fn close(&mut self) {
        self.renderer = None;
    }

    pub(super) fn sidebar_visible(&self) -> bool {
        self.sidebar.position != SidebarPosition::Hidden
    }

    pub(super) fn set_sidebar_visible(&mut self, visible: bool) {
        if self.sidebar_visible() != visible {
            self.sidebar.toggle();
        }
    }

    /// Cell metrics, once the window is open.
    pub(super) fn metrics(&self) -> Option<crate::render::CellMetrics> {
        self.renderer.as_ref().map(Renderer::metrics)
    }

    /// Pane geometry, once the window is open.
    pub(super) fn grid(&self) -> Option<&Self> {
        self.renderer.as_ref().map(|_| self)
    }

    fn layout(&self, tab_count: usize) -> (Rect, Option<Rect>) {
        let Some(r) = &self.renderer else {
            return (Rect::new(0.0, 0.0, 0.0, 0.0), None);
        };
        sidebar_layout(
            r.content_rect(0),
            r.content_rect(tab_count),
            self.sidebar.position,
            self.sidebar_width,
            r.metrics().scale,
        )
    }

    /// The sidebar is on screen.
    pub(super) fn sidebar_shown(&self, ws: &Workspace) -> bool {
        self.layout(ws.tabs().len()).1.is_some()
    }

    /// New port scan results; true when they differ from what is shown.
    pub(super) fn set_ports(&mut self, ports: HashMap<PaneId, Vec<u16>>) -> bool {
        let changed = ports != self.ports;
        self.ports = ports;
        changed
    }

    pub(super) fn sidebar_contains(&self, ws: &Workspace, x: f64, y: f64) -> bool {
        self.layout(ws.tabs().len())
            .1
            .is_some_and(|area| area.contains(x, y))
    }

    pub(super) fn notify(
        &mut self,
        enabled: bool,
        id: PaneId,
        notice: &Notice,
        tab_title: impl FnOnce() -> String,
    ) {
        self.notifications.show(enabled, id, notice, tab_title);
    }

    pub(super) fn scroll_sidebar(&mut self, ws: &Workspace, x: f64, y: f64, lines: f64) -> bool {
        let Some(area) = self.layout(ws.tabs().len()).1.filter(|a| a.contains(x, y)) else {
            return false;
        };
        let line_height = self
            .renderer
            .as_ref()
            .map_or(20.0, |r| r.metrics().line_height);
        self.sidebar_scroll.scroll(
            -lines as f32 * line_height,
            &sidebar_entries(ws, &self.ports),
            area,
            line_height,
        );
        self.request_redraw();
        true
    }

    /// Ask for one redraw; repeated requests before it happens are merged.
    pub(super) fn request_redraw(&mut self) {
        if let Some(ref r) = self.renderer {
            if !self.needs_redraw {
                self.needs_redraw = true;
                r.window().request_redraw();
            }
        }
    }

    /// Ask the system for a redraw directly, bypassing the merge.
    pub(super) fn request_redraw_now(&self) {
        if let Some(ref r) = self.renderer {
            r.window().request_redraw();
        }
    }

    pub(super) fn cursor_icon(&self) -> CursorIcon {
        self.cursor_icon
    }

    pub(super) fn set_cursor_icon(&mut self, icon: CursorIcon) {
        if self.cursor_icon != icon {
            self.cursor_icon = icon;
            if let Some(ref r) = self.renderer {
                r.window().set_cursor(icon);
            }
        }
    }

    pub(super) fn set_title(&self, title: &str) {
        if let Some(ref r) = self.renderer {
            r.window().set_title(title);
        }
    }

    pub(super) fn bell(&self) {
        if let Some(ref r) = self.renderer {
            r.window()
                .request_user_attention(Some(UserAttentionType::Informational));
        }
    }

    pub(super) fn set_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        if let Some(ref mut r) = self.renderer {
            r.set_preedit(preedit);
        }
    }

    pub(super) fn set_command_palette(
        &mut self,
        view: Option<crate::render::scene::palette::PaletteView>,
    ) {
        if let Some(ref mut r) = self.renderer {
            r.set_command_palette(view);
        }
    }

    pub(super) fn palette_hit(&self, x: f64, y: f64) -> crate::render::scene::palette::PaletteHit {
        self.renderer
            .as_ref()
            .map_or(crate::render::scene::palette::PaletteHit::Outside, |r| {
                r.palette_hit(x, y)
            })
    }

    pub(super) fn reveal_active_tab(&mut self) {
        self.sidebar_scroll.reveal_active();
    }

    /// The window has keyboard focus.
    pub(super) fn is_focused(&self) -> bool {
        self.renderer.as_ref().is_some_and(Renderer::is_focused)
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        if let Some(ref mut r) = self.renderer {
            r.set_focused(focused);
            r.window().request_redraw();
        }
    }

    pub(super) fn resize(&mut self, width: u32, height: u32) {
        if let Some(ref mut r) = self.renderer {
            r.resize(width, height);
        }
    }

    pub(super) fn set_scale_factor(&mut self, scale: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_scale_factor(scale);
        }
    }

    pub(super) fn set_font_size(&mut self, size: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_font_size(size);
        }
    }

    pub(super) fn set_opacity(&mut self, opacity: f32) {
        if let Some(ref mut r) = self.renderer {
            r.set_opacity(opacity);
        }
    }

    /// Pane `id` was closed: drop what the renderer cached for it.
    pub(super) fn forget_pane(&mut self, id: PaneId) {
        self.notifications.forget(id);
        if let Some(r) = self.renderer.as_mut() {
            r.forget_pane(id);
        }
    }

    /// What lies under `(x, y)` in the window.
    pub(super) fn hit_test(&self, ws: &Workspace, x: f64, y: f64) -> Hit {
        let Some(r) = self.renderer.as_ref() else {
            return Hit::Nothing;
        };
        let n = ws.tabs().len();
        let (content, sidebar) = self.layout(n);
        if let Some(area) = sidebar {
            if area.contains(x, y) {
                let view = self.sidebar_scroll.view(
                    &sidebar_entries(ws, &self.ports),
                    ws.active(),
                    area,
                    r.metrics().line_height,
                    self.sidebar.position,
                );
                return view.pointer_hit(x, y).unwrap_or(Hit::TabBar);
            }
        }
        let hidden = sidebar.is_none();
        let geometry = HitGeometry {
            tab_bar_height: if hidden { r.tab_bar_height(n) } else { 0.0 },
            tabs: if hidden { r.tab_rects(n) } else { Vec::new() },
            content,
            gap: r.divider_gap(),
            slop: (r.metrics().scale * 3.0).max(2.0),
        };
        hit_test(&geometry, ws.active_tab().map(|t| &t.root), x, y)
    }

    /// The cell of pane `id` (drawn in `rect`) under `(x, y)`, and which side of it.
    pub(super) fn cell_at(
        &self,
        id: PaneId,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> Option<(usize, usize, Side)> {
        let r = self.renderer.as_ref()?;
        Some(r.pixel_to_visual_cell(id, rect, x, y))
    }

    /// Height of a text line in pixels.
    pub(super) fn line_height(&self) -> Option<f64> {
        self.renderer
            .as_ref()
            .map(|r| r.metrics().line_height as f64)
    }

    /// Cell width and height in pixels.
    pub(super) fn cell_size(&self) -> Option<(u16, u16)> {
        let r = self.renderer.as_ref()?;
        let m = r.metrics();
        Some((m.char_width as u16, m.line_height as u16))
    }

    /// Divider width, for dragging dividers.
    pub(super) fn gap(&self) -> f32 {
        self.renderer.as_ref().map_or(1.0, Renderer::divider_gap)
    }

    /// The color OSC 4/10/11/12 asks for: palette entry `index`, or 256 foreground, 257
    /// background, 258 cursor.
    pub(super) fn palette_rgb(&self, index: usize) -> Option<Rgb> {
        let r = self.renderer.as_ref()?;
        let rgba = match index {
            256 => r.palette().foreground,
            257 => r.palette().background,
            258 => r.palette().cursor,
            i if i < 256 => r.palette().resolve(AnsiColor::Indexed(i as u8), false),
            _ => r.palette().foreground,
        };
        let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        Some(Rgb {
            r: to8(rgba.r),
            g: to8(rgba.g),
            b: to8(rgba.b),
        })
    }

    /// The drawn cursor blink phase differs from `visible`.
    pub(super) fn blink_differs(&self, visible: bool) -> bool {
        self.renderer
            .as_ref()
            .is_some_and(|r| r.cursor_blink() != visible)
    }

    fn render(
        &mut self,
        ws: &Workspace,
        tabs: &[TabLabel],
        panes: &[PaneFrame],
        dividers: &[Rect],
        blink: bool,
    ) {
        let sidebar = self.layout(ws.tabs().len()).1.map(|area| {
            let line_height = self
                .renderer
                .as_ref()
                .map_or(20.0, |r| r.metrics().line_height);
            let mut entries = sidebar_entries(ws, &self.ports);
            let directories = entries.iter().filter_map(|e| e.cwd.clone()).collect();
            self.branches.retain(&directories);
            let now = Instant::now();
            for entry in &mut entries {
                entry.branch = entry.cwd.as_deref().and_then(|p| self.branches.get(p, now));
            }
            self.sidebar_scroll
                .update(&entries, ws.active(), area, line_height);
            self.sidebar_scroll.view(
                &entries,
                ws.active(),
                area,
                line_height,
                self.sidebar.position,
            )
        });
        if sidebar.is_none() {
            self.branches.clear();
        }
        let attention: Vec<Rect> = panes
            .iter()
            .filter(|p| ws.pane(p.id).is_some_and(|p| p.meta.attention.is_some()))
            .map(|p| p.rect)
            .collect();
        if let Some(ref mut r) = self.renderer {
            r.set_cursor_blink(blink);
            if let Err(e) = r.render(tabs, panes, dividers, sidebar.as_ref(), &attention) {
                tracing::error!("render error: {e:?}");
            }
        }
    }
}

fn sidebar_entries(ws: &Workspace, ports: &HashMap<PaneId, Vec<u16>>) -> Vec<Entry> {
    ws.tabs()
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let focused = ws.pane(tab.focused);
            let mut tab_ports: Vec<u16> = tab
                .root
                .leaves()
                .into_iter()
                .filter_map(|id| ports.get(&id))
                .flatten()
                .copied()
                .collect();
            tab_ports.sort_unstable();
            tab_ports.dedup();
            let mut notifications: Vec<_> = tab
                .root
                .leaves()
                .into_iter()
                .filter_map(|id| ws.pane(id)?.meta.attention.as_ref())
                .collect();
            notifications.sort_by_key(|n| n.at);
            let remote = sidebar_remote(ws, tab);
            let mut entry = Entry::new(
                &sidebar_title(ws, index),
                focused
                    .filter(|_| remote.is_none())
                    .and_then(|p| p.meta.cwd.as_deref()),
                focused.is_some_and(|p| p.meta.running),
                focused.and_then(|p| p.meta.last_exit),
                notifications
                    .into_iter()
                    .map(|n| Notice::new(n.title.as_deref(), &n.body)),
            )
            .with_ports(tab_ports);
            entry.remote_state = remote.map(|remote| remote.state);
            entry
        })
        .collect()
}

pub(super) fn sidebar_title(ws: &Workspace, index: usize) -> String {
    let tab = ws.tabs().get(index);
    if let Some(remote) = tab.and_then(|tab| sidebar_remote(ws, tab)) {
        return crate::shell_integration::clean_text(remote.target.destination(), 256);
    }
    let pane = tab.and_then(|tab| ws.pane(tab.focused));
    entry_title(
        pane.and_then(|p| p.title.as_deref()),
        pane.and_then(|p| p.pty.as_ref()?.program_name()),
        index,
    )
}

fn sidebar_remote<'a>(
    ws: &'a Workspace,
    tab: &crate::session::workspace::Tab,
) -> Option<&'a crate::remote::RemotePane> {
    std::iter::once(tab.focused)
        .chain(tab.root.leaves())
        .find_map(|id| ws.pane(id)?.remote.as_ref())
}

impl App {
    /// Draw the active tab.
    pub(super) fn redraw(&mut self) {
        self.window.needs_redraw = false;
        if self.modifiers.control_key() && !self.palette_open() {
            self.refresh_pointer_hover();
        }
        // A notification counts as seen once its pane is on screen, focused, in a focused window.
        if self.window_focused {
            if let Some(pane) = self.focused_id().and_then(|id| self.ws.pane_mut(id)) {
                pane.meta.attention = None;
                self.notification_history.mark_seen(pane.id);
            }
        }
        self.sync_port_watch();
        let (tabs, panes, dividers) = self.frame();
        let visible = self.cursor_blink.visible();
        self.window
            .render(&self.ws, &tabs, &panes, &dividers, visible);
    }

    /// Snapshot everything the renderer draws for the active tab.
    fn frame(&self) -> (Vec<TabLabel>, Vec<PaneFrame>, Vec<Rect>) {
        let tabs = (0..self.ws.tabs().len())
            .map(|i| TabLabel {
                title: self.ws.tab_title(i),
                active: i == self.ws.active(),
            })
            .collect();
        let Some(r) = self.window.grid() else {
            return (tabs, Vec::new(), Vec::new());
        };
        let focused = self.focused_id();
        let panes = self
            .ws
            .active_layout(r)
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = self.ws.pane(id)?;
                let (lines, cursor) = pane.terminal.snapshot();
                let status = pane
                    .ui
                    .search_query
                    .as_deref()
                    .map(|q| self.search_bar_text(id, q))
                    .or_else(|| pane.ui.hover_target.as_ref().map(|t| format!("→ {t}")));
                Some(PaneFrame {
                    id,
                    rect,
                    lines,
                    cursor,
                    focused: focused == Some(id),
                    status,
                    blocks: pane.blocks.decor(&pane.terminal),
                })
            })
            .collect();
        let dividers = self
            .ws
            .active_dividers(r)
            .into_iter()
            .map(|d| d.rect)
            .collect();
        (tabs, panes, dividers)
    }

    /// The window gained or lost keyboard focus.
    pub(super) fn on_window_focus(&mut self, focused: bool) {
        // The release of a drag that ends outside the window may never arrive.
        if !focused {
            let selecting: Vec<PaneId> = self
                .ws
                .panes()
                .filter(|p| p.ui.selection.selecting)
                .map(|p| p.id)
                .collect();
            for id in selecting {
                self.finish_selection(id);
            }
            self.release_held_buttons();
            self.pointer.cancel();
        }
        self.cursor_blink.set_focused(Instant::now(), focused);
        self.window.set_focused(focused);
        self.window_focused = focused;
        self.window.request_redraw();
        if let Some(id) = self.focused_id() {
            self.report_focus(id, focused);
        }
    }

    pub(super) fn update_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) {
        self.preedit_active = preedit.is_some();
        self.window.set_preedit(preedit);
        self.window.request_redraw();
    }

    /// Zoom by `steps` points.
    pub(super) fn zoom_by(&mut self, steps: i32) {
        let size = self.config.font_size + steps as f32;
        self.update_font_size(size);
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.reset_pointer_gesture();
        self.config.sidebar = self.window.sidebar.toggle().as_str().to_owned();
        self.config_dirty = true;
        self.apply_layout();
        self.refresh_pointer_hover();
    }

    pub(super) fn update_font_size(&mut self, new_size: f32) {
        let clamped = clamp_font_size(new_size);
        self.config.font_size = clamped;
        // Written once on exit instead of on every zoom step.
        self.config_dirty = true;
        self.window.set_font_size(clamped);
        self.apply_layout();
    }

    /// Change the background opacity by `steps` wheel notches.
    pub(super) fn step_opacity_by(&mut self, steps: i32) {
        let opacity = step_opacity(self.config.background_opacity, steps);
        self.config.background_opacity = opacity;
        self.config_dirty = true;
        self.window.set_opacity(opacity);
        self.window.request_redraw();
    }

    /// The window title follows the focused pane.
    pub(super) fn refresh_window_title(&self) {
        let title = self
            .ws
            .focused_title()
            .unwrap_or_else(|| WINDOW_TITLE.to_owned());
        self.window.set_title(&title);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::CellMetrics;
    use crate::session::{pane::Pane, workspace::PaneFactory};
    use crate::terminal::Terminal;
    use std::cell::Cell;
    use std::path::PathBuf;

    struct TestGrid {
        sidebar: Placement,
        metrics: CellMetrics,
        sizes: Cell<usize>,
    }

    impl GridGeometry for TestGrid {
        fn content_rect(&self, tabs: usize) -> Rect {
            let size = (800.0, 600.0);
            sidebar_layout(
                self.metrics.content_rect(0, size),
                self.metrics.content_rect(tabs, size),
                self.sidebar.position,
                220.0,
                1.0,
            )
            .0
        }

        fn gap(&self) -> f32 {
            1.0
        }

        fn grid_size(&self, rect: Rect) -> (usize, usize) {
            self.sizes.set(self.sizes.get() + 1);
            self.metrics.grid_size(rect)
        }
    }

    struct TestPanes;

    impl PaneFactory for TestPanes {
        fn spawn(
            &mut self,
            id: PaneId,
            cols: usize,
            rows: usize,
            _: Option<PathBuf>,
        ) -> Option<Pane> {
            Some(Pane::new(
                id,
                Terminal::new(cols, rows, |_| {}),
                None,
                (cols, rows),
            ))
        }
    }

    #[test]
    fn one_layout_pass_per_toggle_resizes_active_and_inactive_tabs() {
        let mut grid = TestGrid {
            sidebar: Placement::new(SidebarPosition::Left),
            metrics: CellMetrics {
                font_size: 14.0,
                scale: 1.0,
                char_width: 10.0,
                line_height: 20.0,
                padding_left: 0.0,
                padding_top: 0.0,
            },
            sizes: Cell::new(0),
        };
        let mut ws = Workspace::new();
        let one = ws.new_tab(&grid, &mut TestPanes).unwrap();
        let two = ws.new_tab(&grid, &mut TestPanes).unwrap();
        for (side, size, origin) in [
            (SidebarPosition::Hidden, (80, 29), (0.0, 20.0)),
            (SidebarPosition::Left, (58, 30), (220.0, 0.0)),
        ] {
            assert_eq!(grid.sidebar.toggle(), side);
            grid.sizes.set(0);
            ws.apply_layout(&grid);
            assert_eq!(grid.sizes.get(), 2);
            assert_eq!(ws.pane(one).unwrap().size, size);
            assert_eq!(ws.pane(two).unwrap().size, size);
            let rect = ws.pane_rect(&grid, two).unwrap();
            assert_eq!((rect.x, rect.y), origin);
        }
    }
}
