//! The window and its renderer. Every call into the renderer lives in this file.

use alacritty_terminal::index::Side;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, UserAttentionType, WindowAttributes};

use super::{git_diff, App};
use crate::blocks::cards::BlockHover;
use crate::config::TwittyConfig;
use crate::layout::{PaneId, Rect, SidebarPosition};
use crate::render::scene::palette::{PaletteIcons, PaletteView};
use crate::render::scene::window_chrome::{ChromeView, PaneHeaderView};
use crate::render::{PaneFrame, Renderer, TabLabel};
use crate::session::workspace::{GridGeometry, Workspace};
use crate::sidebar::cards::{Drag, ScrollState, View as SidebarView};
use crate::sidebar::{
    git::BranchCache, localized_entry_title, pick_tab_agent_status, Entry, Notice, Placement,
};
use crate::ui::chrome::{
    ChromeAction, PaneFlash, PaneHeaderLayout, TopBarLayout, UiLanguage, WindowLayout,
};
use crate::ui::git_diff::DiffStat;
use crate::ui::hit::{hit_test, Hit, HitGeometry};
use crate::ui::motion::ChromeAnimations;
use crate::ui::notifications::NotificationHistory;
use crate::ui::settings::{clamp_font_size, step_opacity};

pub(super) const WINDOW_TITLE: &str = "Twitty · RTL Terminal";
#[cfg(any(
    target_os = "linux",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
))]
pub(super) const WINDOW_APP_ID: &str = "twitty";

impl GridGeometry for Window {
    fn content_rect(&self, tab_count: usize) -> Rect {
        self.layout(tab_count)
            .map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |layout| layout.content)
    }

    fn gap(&self) -> f32 {
        self.gap()
    }

    fn grid_size(&self, rect: Rect) -> (usize, usize) {
        self.renderer
            .as_ref()
            .map_or((2, 1), |r| r.compute_grid_size(self.pane_layout(rect).grid))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct LayoutKey {
    metrics: crate::render::CellMetrics,
    surface: (f32, f32),
    tab_count: usize,
    sidebar: (SidebarPosition, f32),
    top_bar: bool,
    review_open: bool,
    agent_panel_open: bool,
    agent_panel_width_tenths: Option<u32>,
    language: UiLanguage,
}

/// The window, once it is open: its renderer, the pending-redraw flag and the cursor icon.
pub(super) struct Window {
    renderer: Option<Renderer>,
    perf: Option<crate::perf::Perf>,
    needs_redraw: bool,
    layout_cache: RefCell<Option<(LayoutKey, Rc<WindowLayout>)>>,
    cursor_icon: CursorIcon,
    sidebar: Placement,
    sidebar_width: f32,
    top_bar: bool,
    pane_headers: bool,
    occluded: bool,
    chrome_hover: Option<ChromeAction>,
    tab_hover: Option<usize>,
    pub(super) language: UiLanguage,
    sidebar_view: Option<SidebarView>,
    sidebar_drag: Option<Drag>,
    motion: ChromeAnimations,
    motion_frame: Option<Instant>,
    block_hover: Option<BlockHover>,
    tooltip_shown: bool,
    pane_flash: PaneFlash,
    diff: Option<(git_diff::Target, DiffStat)>,
    sidebar_scroll: ScrollState,
    branches: BranchCache,
    notifications: crate::notify::DesktopNotifier,
    /// Listening TCP ports per pane, from the last port scan.
    ports: HashMap<PaneId, Vec<u16>>,
    /// The code review panel is open (it takes the right part of the pane area).
    review_open: bool,
    /// The docked Agent side panel is open.
    agent_panel_open: bool,
    /// Optional user-dragged logical width for the Agent side panel.
    agent_panel_width: Option<f32>,
}

impl Window {
    pub(super) fn new(perf: Option<crate::perf::Perf>) -> Self {
        Self {
            renderer: None,
            perf,
            needs_redraw: false,
            layout_cache: RefCell::default(),
            cursor_icon: CursorIcon::Default,
            sidebar: Placement::new(SidebarPosition::Right),
            sidebar_width: crate::layout::DEFAULT_SIDEBAR_WIDTH,
            top_bar: true,
            pane_headers: true,
            occluded: false,
            chrome_hover: None,
            tab_hover: None,
            language: {
                let language = UiLanguage::from_environment();
                language.set_current();
                language
            },
            sidebar_view: None,
            sidebar_drag: None,
            motion: ChromeAnimations::default(),
            motion_frame: None,
            block_hover: None,
            tooltip_shown: false,
            pane_flash: PaneFlash::default(),
            diff: None,
            sidebar_scroll: ScrollState::default(),
            branches: BranchCache::default(),
            notifications: crate::notify::DesktopNotifier::default(),
            ports: HashMap::new(),
            review_open: false,
            agent_panel_open: false,
            agent_panel_width: None,
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
        #[cfg(any(
            target_os = "linux",
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        let window_attrs = {
            use winit::platform::wayland::WindowAttributesExtWayland;
            // Sets both Wayland xdg_toplevel app_id and X11 WM_CLASS to match twitty.desktop.
            WindowAttributesExtWayland::with_name(window_attrs, WINDOW_APP_ID, WINDOW_APP_ID)
        };

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

        let mut renderer =
            match pollster::block_on(Renderer::new(window, config.font_size, self.perf.as_mut())) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("failed to initialize renderer: {e:?}");
                    event_loop.exit();
                    return false;
                }
            };

        self.motion = ChromeAnimations::new(config.reduce_motion);
        renderer.set_reduce_motion(config.reduce_motion);
        renderer.set_font_family(config.font_family.as_deref());
        renderer.set_ui_style(config.ui_font_family.as_deref(), config.ui_accent_color());
        renderer.set_opacity(config.background_opacity);
        renderer.set_cursor_style(config.cursor_style.clone());
        renderer.set_palette(config.palette());
        renderer.set_block_cards(config.block_cards);
        self.sidebar = Placement::new(config.sidebar.parse().unwrap_or_default());
        self.sidebar_width = config.sidebar_width;
        self.top_bar = config.top_bar;
        self.pane_headers = config.pane_headers;
        self.language = config.effective_ui_language();
        self.language.set_current();
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

    fn layout(&self, tab_count: usize) -> Option<Rc<WindowLayout>> {
        let r = self.renderer.as_ref()?;
        let key = LayoutKey {
            metrics: r.metrics(),
            surface: r.surface_size(),
            tab_count,
            sidebar: (self.sidebar.position, self.sidebar_width),
            top_bar: self.top_bar,
            review_open: self.review_open,
            agent_panel_open: self.agent_panel_open,
            agent_panel_width_tenths: self.agent_panel_width.map(|w| (w * 10.0).round() as u32),
            language: self.language,
        };
        let mut cached = self.layout_cache.borrow_mut();
        if let Some((_, layout)) = cached.as_ref().filter(|(old, _)| *old == key) {
            return Some(Rc::clone(layout));
        }
        let mut layout = WindowLayout::new(
            key.metrics,
            key.surface,
            key.tab_count,
            key.sidebar,
            key.top_bar,
        )
        .with_language(key.language);
        if key.review_open {
            // The review panel takes the right part of what the sidebar leaves to the panes.
            let (content, review) = crate::review::layout::split_content(
                layout.content,
                key.surface.0,
                key.metrics.scale,
            );
            layout.content = content;
            layout.review = review;
        } else if key.agent_panel_open {
            let (content, agent) = crate::ui::agent_panel::split_agent_content(
                layout.content,
                key.metrics.scale,
                self.agent_panel_width,
                key.language,
            );
            layout.content = content;
            layout.agent_panel = agent;
        }
        let layout = Rc::new(layout);
        *cached = Some((key, Rc::clone(&layout)));
        Some(layout)
    }

    /// Open or close the review panel's area (the caller lays the panes out again).
    pub(super) fn set_review_open(&mut self, open: bool) {
        self.review_open = open;
        if !open {
            self.set_review(None);
        }
    }

    /// Where the review panel is drawn, while it is open and fits.
    pub(super) fn review_rect(&self, ws: &Workspace) -> Option<Rect> {
        self.layout(ws.tabs().len())?.review
    }

    /// Hand the review panel's view to the renderer (`None` hides it).
    pub(super) fn set_review(&mut self, view: Option<crate::review::panel::View>) {
        if let Some(ref mut r) = self.renderer {
            r.set_review(view);
        }
    }

    /// Open or close the docked Agent side panel area.
    pub(super) fn set_agent_panel_open(&mut self, open: bool, width: Option<f32>) {
        self.agent_panel_open = open;
        self.agent_panel_width = width;
        if !open {
            self.set_agent_panel(None);
        }
    }

    /// Where the Agent side panel is drawn, while open and fitting.
    pub(super) fn agent_panel_rect(&self, ws: &Workspace) -> Option<Rect> {
        self.layout(ws.tabs().len())?.agent_panel
    }

    /// Full tab content rectangle (terminal panes + agent panel combined) for divider dragging.
    pub(super) fn agent_panel_tab_content_rect(&self, ws: &Workspace) -> Option<Rect> {
        let layout = self.layout(ws.tabs().len())?;
        let Some(panel) = layout.agent_panel else {
            return Some(layout.content);
        };
        let left = layout.content.x.min(panel.x);
        let right = layout.content.right().max(panel.right());
        Some(Rect::new(
            left,
            layout.content.y,
            (right - left).max(0.0),
            layout.content.h,
        ))
    }

    /// Hand the Agent panel view to the renderer (`None` hides it).
    pub(super) fn set_agent_panel(&mut self, view: Option<crate::ui::agent_panel::AgentPanelView>) {
        if let Some(ref mut r) = self.renderer {
            r.set_agent_panel(view);
        }
    }

    /// Hand the command composer's view to the renderer (`None` hides it).
    pub(super) fn set_composer(
        &mut self,
        view: Option<crate::render::scene::composer::ComposerView>,
    ) {
        if let Some(ref mut r) = self.renderer {
            r.set_composer(view);
        }
    }

    /// Hand the failed-block AI explanation/fix popover view to the renderer (`None` hides it).
    pub(super) fn set_block_ai(&mut self, view: Option<crate::render::scene::ai::BlockAiView>) {
        if let Some(ref mut r) = self.renderer {
            r.set_block_ai(view);
        }
    }

    /// Hand an open right-click menu to the renderer (`None` hides it).
    pub(super) fn set_context_menu(&mut self, menu: Option<crate::ui::context_menu::ContextMenu>) {
        if let Some(ref mut r) = self.renderer {
            r.set_context_menu(menu);
        }
    }

    /// Where `menu` is drawn, for hit testing (`None` before the renderer exists).
    pub(super) fn context_menu_layout(
        &self,
        menu: &crate::ui::context_menu::ContextMenu,
    ) -> Option<crate::ui::context_menu::MenuLayout> {
        let r = self.renderer.as_ref()?;
        Some(r.context_menu_layout(menu, self.language))
    }

    /// Hand the one-time AI privacy consent modal view to the renderer (`None` hides it).
    pub(super) fn set_ai_consent(
        &mut self,
        view: Option<crate::render::scene::ai::ConsentOverlayView>,
    ) {
        if let Some(ref mut r) = self.renderer {
            r.set_ai_consent(view);
        }
    }

    pub(super) fn ai_consent_hit(&self, x: f64, y: f64) -> crate::render::scene::ai::ConsentHit {
        self.renderer
            .as_ref()
            .map_or(crate::render::scene::ai::ConsentHit::Outside, |r| {
                r.ai_consent_hit(x, y)
            })
    }

    pub(super) fn block_ai_hit(&self, x: f64, y: f64) -> crate::render::scene::ai::BlockAiHit {
        self.renderer
            .as_ref()
            .map_or(crate::render::scene::ai::BlockAiHit::Outside, |r| {
                r.block_ai_hit(x, y)
            })
    }

    pub(super) fn ui_language(&self) -> UiLanguage {
        self.language
    }

    pub(super) fn set_ui_language(&mut self, language: UiLanguage) {
        language.set_current();
        if self.language != language {
            self.language = language;
            self.request_redraw();
        }
    }

    pub(super) fn pane_headers_enabled(&self) -> bool {
        self.pane_headers
    }

    pub(super) fn pane_layout(&self, rect: Rect) -> PaneHeaderLayout {
        PaneHeaderLayout::new(
            rect,
            self.metrics().map_or(1.0, |m| m.scale),
            self.pane_headers,
        )
        .with_language(self.language)
    }

    pub(super) fn diff_visible(&self, tab_count: usize) -> bool {
        let Some(r) = self.renderer.as_ref() else {
            return false;
        };
        let Some(layout) = self.layout(tab_count) else {
            return false;
        };
        !self.occluded && TopBarLayout::can_show_diff(layout.top_bar, r.metrics().scale)
    }

    pub(super) fn is_occluded(&self) -> bool {
        self.occluded
    }

    pub(super) fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
        if occluded {
            self.needs_redraw = false;
            self.motion_frame = None;
            self.motion.suspend();
        } else {
            self.request_redraw();
        }
    }

    pub(super) fn set_diff(&mut self, diff: Option<(git_diff::Target, DiffStat)>) {
        if self.diff != diff {
            self.diff = diff;
            self.request_redraw();
        }
    }

    fn diff(&self, ws: &Workspace) -> Option<DiffStat> {
        let (target, stats) = self.diff.as_ref()?;
        (git_diff::target(ws).as_ref() == Some(target)).then_some(*stats)
    }

    pub(super) fn chrome_hover(&self) -> Option<ChromeAction> {
        self.chrome_hover
    }

    pub(super) fn block_hover(&self) -> Option<BlockHover> {
        self.block_hover
    }

    /// Hand the visible notices to the renderer.
    pub(super) fn set_toasts(&mut self, toasts: Vec<crate::ui::toast::Toast>) {
        if let Some(ref mut r) = self.renderer {
            r.set_toasts(toasts);
        }
    }

    /// Show a tooltip (`text` near pointer `at`) or hide it.
    pub(super) fn set_tooltip(&mut self, tip: Option<(String, (f32, f32))>) {
        self.tooltip_shown = tip.is_some();
        if let Some(ref mut r) = self.renderer {
            r.set_tooltip(tip);
        }
    }

    pub(super) fn tooltip_shown(&self) -> bool {
        self.tooltip_shown
    }

    pub(super) fn set_chrome_hover(&mut self, hover: Option<ChromeAction>, tab: Option<usize>) {
        if self.chrome_hover != hover || self.tab_hover != tab {
            self.chrome_hover = hover;
            self.tab_hover = tab;
            self.request_redraw();
        }
    }

    pub(super) fn set_block_hover(&mut self, hover: Option<BlockHover>) {
        if self.block_hover != hover {
            self.block_hover = hover;
            if let Some(ref mut r) = self.renderer {
                r.set_block_hover(hover);
            }
            self.request_redraw();
        }
    }

    pub(super) fn flash_pane(&mut self, id: PaneId) {
        if !self.motion.reduced() {
            self.pane_flash.trigger(id, Instant::now());
            self.request_redraw();
        }
    }

    /// The sidebar is on screen.
    pub(super) fn sidebar_shown(&self, ws: &Workspace) -> bool {
        self.layout(ws.tabs().len())
            .is_some_and(|layout| layout.sidebar.is_some())
    }

    /// Local directories currently displayed in the sidebar, for off-thread git branch lookups.
    pub(super) fn sidebar_branch_dirs(&self, ws: &Workspace) -> Vec<PathBuf> {
        ws.tabs()
            .iter()
            .filter_map(|tab| {
                if sidebar_remote(ws, tab).is_some() {
                    return None;
                }
                ws.pane(tab.focused)?.meta.cwd.clone()
            })
            .collect()
    }

    /// The cached git branch of `dir`, when the sidebar has looked it up.
    pub(super) fn branch_for(&self, dir: &std::path::Path) -> Option<String> {
        self.branches.cached(dir)
    }

    /// Apply git branch lookups produced off the UI thread; returns true when any changed.
    pub(super) fn set_branches(
        &mut self,
        branches: HashMap<PathBuf, Option<String>>,
        now: Instant,
    ) -> bool {
        self.branches.update(branches, now)
    }

    /// New port scan results; true when they differ from what is shown.
    pub(super) fn set_ports(&mut self, ports: HashMap<PaneId, Vec<u16>>) -> bool {
        let changed = ports != self.ports;
        self.ports = ports;
        changed
    }

    pub(super) fn sidebar_contains(&self, ws: &Workspace, x: f64, y: f64) -> bool {
        self.layout(ws.tabs().len())
            .and_then(|layout| layout.sidebar)
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
        if !self.sidebar_contains(ws, x, y) {
            return false;
        }
        if let Some(view) = &self.sidebar_view {
            let line_height = self.line_height().unwrap_or(20.0);
            self.sidebar_scroll
                .scroll((-lines * line_height) as f32, view);
        }
        self.request_redraw();
        true
    }

    pub(super) fn start_sidebar_drag(&mut self, tab_id: u64, y: f64) {
        self.sidebar_drag = Some(Drag::new(tab_id, y));
    }

    pub(super) fn cancel_sidebar_drag(&mut self) {
        if self.sidebar_drag.take().is_some() {
            self.request_redraw();
        }
    }

    pub(super) fn update_sidebar_drag(&mut self) {
        if self.sidebar_drag.is_some() {
            self.request_redraw();
        }
    }

    pub(super) fn sidebar_drop(&mut self, ws: &Workspace, pointer: (f64, f64)) -> Option<usize> {
        let drag = self.sidebar_drag.take()?;
        self.request_redraw();
        let view = self.sidebar_view.as_ref()?;
        let area = self.layout(ws.tabs().len())?.sidebar?;
        if !view.matches(area, ws.tabs().len()) || ws.active_tab()?.id != drag.tab_id {
            return None;
        }
        let target = drag.target(view, ws.active(), pointer)?;
        self.reveal_active_tab();
        Some(target)
    }

    pub(super) fn poll_motion(&mut self, now: Instant) -> Option<Instant> {
        if self.motion_frame.is_some_and(|deadline| now >= deadline) {
            self.motion_frame = None;
            self.request_redraw();
        }
        self.motion_frame
    }

    pub(super) fn poll_perf(&mut self, now: Instant) -> Option<Instant> {
        self.perf.as_mut().map(|perf| perf.poll(now))
    }

    /// Ask for one redraw; repeated requests before it happens are merged.
    pub(super) fn request_redraw(&mut self) {
        if self.occluded {
            return;
        }
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

    pub(super) fn set_command_palette(&mut self, view: Option<(PaletteView, PaletteIcons)>) {
        if let Some(ref mut r) = self.renderer {
            r.set_command_palette(view);
        }
    }

    pub(super) fn set_settings_panel(
        &mut self,
        view: Option<crate::render::scene::settings::SettingsView>,
    ) {
        if let Some(ref mut r) = self.renderer {
            r.set_settings_panel(view);
        }
    }

    pub(super) fn set_memory_panel(
        &mut self,
        view: Option<crate::render::scene::memory::MemoryView>,
    ) {
        if let Some(ref mut r) = self.renderer {
            r.set_memory_panel(view);
        }
    }

    pub(super) fn set_welcome(&mut self, view: Option<crate::render::scene::welcome::WelcomeView>) {
        if let Some(ref mut r) = self.renderer {
            r.set_welcome(view);
        }
    }

    pub(super) fn font_families(&self) -> (Vec<String>, Vec<String>) {
        self.renderer
            .as_ref()
            .map_or_else(|| (Vec::new(), Vec::new()), Renderer::font_families)
    }

    pub(super) fn surface_size(&self) -> Option<(f32, f32)> {
        self.renderer.as_ref().map(Renderer::surface_size)
    }

    pub(super) fn set_palette(&mut self, palette: crate::color::Palette) {
        if let Some(ref mut r) = self.renderer {
            r.set_palette(palette);
        }
    }

    pub(super) fn set_font_family(&mut self, family: Option<&str>) {
        if let Some(ref mut r) = self.renderer {
            r.set_font_family(family);
        }
    }

    pub(super) fn set_ui_font_family(&mut self, family: Option<&str>) {
        if let Some(ref mut r) = self.renderer {
            r.set_ui_font_family(family);
        }
    }

    pub(super) fn set_ui_accent(&mut self, accent: Option<crate::color::Rgba>) {
        if let Some(ref mut r) = self.renderer {
            r.set_ui_accent(accent);
        }
    }

    pub(super) fn set_cursor_style(&mut self, style: String) {
        if let Some(ref mut r) = self.renderer {
            r.set_cursor_style(style);
        }
    }

    pub(super) fn set_sidebar_position(&mut self, position: SidebarPosition) {
        self.sidebar = Placement::new(position);
    }

    pub(super) fn set_sidebar_width(&mut self, width: f32) {
        self.sidebar_width = crate::layout::clamp_sidebar_width(width);
    }

    pub(super) fn set_top_bar(&mut self, enabled: bool) {
        self.top_bar = enabled;
    }

    pub(super) fn set_pane_headers(&mut self, enabled: bool) {
        self.pane_headers = enabled;
    }

    pub(super) fn set_reduce_motion(&mut self, reduced: bool) {
        self.motion = ChromeAnimations::new(reduced);
        if let Some(ref mut r) = self.renderer {
            r.set_reduce_motion(reduced);
        }
    }

    pub(super) fn set_block_cards(&mut self, enabled: bool) {
        if let Some(ref mut r) = self.renderer {
            r.set_block_cards(enabled);
        }
    }

    pub(super) fn palette_rows(&self, max: usize) -> usize {
        self.renderer.as_ref().map_or(max, |r| r.palette_rows(max))
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
        self.pane_flash.forget(id);
        if self.block_hover.is_some_and(|h| h.pane == id) {
            self.block_hover = None;
            if let Some(r) = self.renderer.as_mut() {
                r.set_block_hover(None);
            }
        }
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
        let Some(layout) = self.layout(n) else {
            return Hit::Nothing;
        };
        if layout.review.is_some_and(|area| area.contains(x, y))
            || layout.agent_panel.is_some_and(|area| area.contains(x, y))
        {
            // Docked side panels handle their own pointer input.
            return Hit::Nothing;
        }
        if let Some(hit) = TopBarLayout::new(layout.top_bar, r.metrics().scale, self.diff(ws))
            .with_language(self.language)
            .hit(x, y)
        {
            return hit;
        }
        if let Some(area) = layout.sidebar.filter(|area| area.contains(x, y)) {
            return self
                .sidebar_view
                .as_ref()
                .filter(|view| view.matches(area, n))
                .and_then(|view| view.pointer_hit(x, y))
                .unwrap_or(Hit::TabBar);
        }
        if let Some(hit) = layout.tab_hit(x, y) {
            return hit;
        }
        if self.pane_headers && layout.content.contains(x, y) {
            for (id, rect) in ws.active_layout(self) {
                if let Some(action) = self.pane_layout(rect).hit(id, x, y) {
                    return Hit::Chrome(action);
                }
            }
        }
        let geometry = HitGeometry {
            tab_bar_height: 0.0,
            tabs: Vec::new(),
            content: layout.content,
            gap: r.divider_gap(),
            slop: (r.metrics().scale * 3.0).max(2.0),
        };
        let zoomed_root = ws.zoomed_pane().map(crate::layout::Node::Leaf);
        let root = zoomed_root
            .as_ref()
            .or_else(|| ws.active_tab().map(|t| &t.root));
        hit_test(&geometry, root, x, y)
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

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        ws: &Workspace,
        panes: &[PaneFrame],
        dividers: &[Rect],
        blink: bool,
        broadcast: bool,
        history: &NotificationHistory,
        pointer: (f64, f64),
        frame_start: Option<Instant>,
    ) {
        let Some(metrics) = self.metrics() else {
            return;
        };
        let Some(layout) = self.layout(ws.tabs().len()) else {
            return;
        };
        let now = Instant::now();
        let ids: Vec<_> = ws.tabs().iter().map(|tab| tab.id).collect();
        self.pane_flash.tick(now);
        self.motion
            .begin_frame(now, ws.active_tab().map(|tab| tab.id), self.chrome_hover);
        self.sidebar_view = layout.sidebar.and_then(|area| {
            let mut entries = sidebar_entries(ws, &self.ports, history, self.language);
            let directories = entries.iter().filter_map(|e| e.cwd.clone()).collect();
            self.branches.retain(&directories);
            for entry in &mut entries {
                entry.branch = entry.cwd.as_deref().and_then(|p| self.branches.cached(p));
            }
            let cards = self.renderer.as_mut()?.sidebar_cards(entries, area);
            let mut view = self.sidebar_scroll.view(
                &cards,
                ws.active(),
                area,
                metrics.scale,
                self.sidebar.position,
                self.language,
            );
            self.motion
                .reorder(&mut view, &cards, &ids, ws.active(), now);
            view.update_hover(pointer);
            view.drop_target = self
                .sidebar_drag
                .and_then(|drag| drag.target(&view, ws.active(), pointer));
            Some(view)
        });
        if self.sidebar_view.is_none() {
            self.branches.clear();
        }
        let hover_tab = match layout.tab_hit(pointer.0, pointer.1) {
            Some(Hit::Tab(i) | Hit::Chrome(ChromeAction::CloseTab(i))) => Some(i),
            _ => None,
        };
        self.motion.apply(
            self.sidebar_view.as_mut(),
            hover_tab,
            &ids,
            now,
            self.occluded,
        );
        let zoomed_id = ws.zoomed_pane();
        let pane_headers = ws
            .active_layout(self)
            .into_iter()
            .filter_map(|(id, rect)| {
                let pane = ws.pane(id)?;
                let layout = self.pane_layout(rect);
                let focused = ws.focused_id() == Some(id);
                let attention = pane.meta.needs_attention() && !(focused && self.is_focused());
                let zoomed = zoomed_id == Some(id);
                let mut flash = self.pane_flash.intensity(id, now);
                if broadcast {
                    flash = flash.max(0.45);
                }
                if layout.header.h == 0.0 && !attention && flash <= 0.0 {
                    return None;
                }
                let (mut title, agent) = pane_header_title(pane);
                if broadcast {
                    title = crate::input::broadcast::format_broadcast_header_title(
                        &title,
                        self.language,
                    );
                }
                Some(PaneHeaderView {
                    id,
                    layout,
                    title,
                    agent,
                    focused,
                    attention,
                    needs_input: pane.meta.waiting_for_input(),
                    zoomed,
                    flash,
                })
            })
            .collect::<Vec<_>>();
        let diff = self.diff(ws);
        let tabs: Vec<_> = (0..layout.tabs.len())
            .map(|i| TabLabel {
                title: ws.tab_title_for_ui(i, self.language),
                active: i == ws.active(),
            })
            .collect();
        let chrome = ChromeView {
            layout: &layout,
            top: TopBarLayout::new(layout.top_bar, metrics.scale, diff)
                .with_language(self.language),
            diff,
            unread: history.unseen(),
            sidebar_visible: self.sidebar_view.is_some(),
            hover: self.chrome_hover,
            hover_tab,
            language: self.language,
            panes: &pane_headers,
            tabs: &tabs,
        };
        if let Some(ref mut r) = self.renderer {
            r.set_cursor_blink(blink);
            let motion = self.motion.sample(&ids, broadcast, now);
            let rendered = r.render(
                &chrome,
                &motion,
                panes,
                dividers,
                self.sidebar_view.as_ref(),
                self.perf.as_mut().zip(frame_start),
            );
            self.motion_frame = if self.occluded || self.motion.reduced() {
                None
            } else {
                [
                    r.next_motion_frame(now),
                    self.motion.next_frame(now),
                    self.pane_flash.tick(now),
                ]
                .into_iter()
                .flatten()
                .min()
            };
            if let Err(e) = rendered {
                tracing::error!("render error: {e:?}");
            }
        }
    }
}

fn pane_header_title(pane: &crate::session::pane::Pane) -> (String, bool) {
    let program = pane.pty.as_ref().and_then(|pty| pty.program_name());
    let command = pane
        .meta
        .last_command
        .as_deref()
        .filter(|_| pane.meta.running);
    let agent_name = [
        program,
        command.and_then(|command| command.split_whitespace().next()),
    ]
    .into_iter()
    .flatten()
    .find_map(|command| {
        let name = std::path::Path::new(command).file_stem()?.to_str()?;
        crate::agent::KNOWN_AGENTS
            .iter()
            .copied()
            .find(|agent| name.eq_ignore_ascii_case(agent))
    });
    let title = agent_name
        .or(command)
        .or(pane.title.as_deref())
        .or(program)
        .map(|title| crate::shell_integration::clean_text(title, 256))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| crate::ui::chrome::tr("Terminal", "الطرفية").to_owned());
    (
        title,
        agent_name.is_some() || pane.meta.agent_status.is_some(),
    )
}

fn sidebar_entries(
    ws: &Workspace,
    ports: &HashMap<PaneId, Vec<u16>>,
    history: &NotificationHistory,
    language: UiLanguage,
) -> Vec<Entry> {
    let history = history.newest_first();
    ws.tabs()
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let focused = ws.pane(tab.focused);
            let panes = tab.root.leaves();
            let mut tab_ports: Vec<u16> = panes
                .iter()
                .filter_map(|id| ports.get(id))
                .flatten()
                .copied()
                .collect();
            tab_ports.sort_unstable();
            tab_ports.dedup();
            let pending = panes
                .iter()
                .filter_map(|&id| Some((id, ws.pane(id)?.meta.attention.as_ref()?)));
            let focused_status = focused.and_then(|p| p.meta.agent_status.as_ref());
            let other_statuses = panes
                .iter()
                .copied()
                .filter(|&id| id != tab.focused)
                .filter_map(|id| ws.pane(id)?.meta.agent_status.as_ref());
            let agent_status = pick_tab_agent_status(focused_status, other_statuses);
            let remote = sidebar_remote(ws, tab);
            let mut entry = Entry::new(
                &localized_sidebar_title(ws, index, language),
                focused
                    .filter(|_| remote.is_none())
                    .and_then(|p| p.meta.cwd.as_deref()),
                focused.is_some_and(|p| p.meta.running),
                focused.and_then(|p| p.meta.last_exit),
                [],
            )
            .with_ports(tab_ports)
            .with_agent_status(agent_status)
            .with_notification_history(&panes, &history, pending);
            entry.remote_state = remote.map(|remote| remote.state);
            entry.last_command = focused
                .and_then(|p| p.meta.last_command.as_deref())
                .filter(|command| !command.trim().is_empty())
                .map(str::to_owned);
            entry
        })
        .collect()
}

pub(super) fn sidebar_title(ws: &Workspace, index: usize, language: UiLanguage) -> String {
    localized_sidebar_title(ws, index, language)
}

fn localized_sidebar_title(ws: &Workspace, index: usize, language: UiLanguage) -> String {
    let tab = ws.tabs().get(index);
    if let Some(custom) = tab
        .and_then(|t| t.title.as_deref())
        .map(|s| crate::shell_integration::clean_text(s, 256))
        .filter(|s| !s.is_empty())
    {
        return custom;
    }
    if let Some(remote) = tab.and_then(|tab| sidebar_remote(ws, tab)) {
        return crate::shell_integration::clean_text(remote.target.destination(), 256);
    }
    let pane = tab.and_then(|tab| ws.pane(tab.focused));
    localized_entry_title(
        pane.and_then(|p| p.title.as_deref()),
        pane.and_then(|p| p.pty.as_ref()?.program_name()),
        index,
        language,
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
        if self.window.is_occluded() {
            return;
        }
        let frame_start = self.window.perf.as_ref().map(|_| Instant::now());
        if self.modifiers.control_key()
            && !self.palette_open()
            && !self.settings_open()
            && !self.welcome_open()
        {
            self.refresh_pointer_hover();
        }
        // A notification counts as seen once its pane is on screen, focused, in a focused window.
        if self.window_focused {
            if let Some(pane) = self.focused_id().and_then(|id| self.ws.pane_mut(id)) {
                pane.meta.attention = None;
                pane.meta.mark_agent_input_seen();
                self.notification_history.mark_seen(pane.id);
            }
        }
        self.sync_port_watch();
        self.sync_branch_watch();
        let review = self.review_view();
        self.window.set_review(review);
        let agent_panel = self.agent_panel_view();
        self.window.set_agent_panel(agent_panel);
        let composer = self.composer_view();
        self.window.set_composer(composer);
        let block_ai = self.block_ai_view();
        self.window.set_block_ai(block_ai);
        let consent = self.ai_consent_view();
        self.window.set_ai_consent(consent);
        self.window
            .set_context_menu(self.context_menu.as_ref().map(|h| h.menu.clone()));
        self.sync_tooltip(Instant::now());
        self.window.set_toasts(self.toasts.visible());
        let (panes, dividers) = self.frame();
        let visible = self.cursor_blink.visible();
        let broadcast = self.active_tab_broadcast();
        self.window.render(
            &self.ws,
            &panes,
            &dividers,
            visible,
            broadcast,
            &self.notification_history,
            if self.pointer.capture.is_none() && self.pointer.drag.is_none() {
                self.pointer.pos
            } else {
                (f64::NEG_INFINITY, f64::NEG_INFINITY)
            },
            frame_start,
        );
    }

    /// Snapshot everything the renderer draws for the active tab.
    fn frame(&self) -> (Vec<PaneFrame>, Vec<Rect>) {
        let Some(r) = self.window.grid() else {
            return (Vec::new(), Vec::new());
        };
        let focused = self.focused_id();
        let broadcast_status = self.active_tab_broadcast() && !self.window.pane_headers_enabled();
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
                    .or_else(|| pane.ui.hover_target.as_ref().map(|t| format!("→ {t}")))
                    .or_else(|| {
                        (broadcast_status && focused == Some(id))
                            .then(|| crate::input::broadcast::broadcast_status_text().to_owned())
                    });
                Some(PaneFrame {
                    id,
                    rect: self.window.pane_layout(rect).grid,
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
        (panes, dividers)
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
            self.window.cancel_sidebar_drag();
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
    }

    pub(super) fn set_sidebar_shown(&mut self, visible: bool) {
        if self.window.sidebar_visible() != visible {
            self.toggle_sidebar();
        }
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
    use crate::layout::sidebar_layout;
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
                180.0,
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
            (SidebarPosition::Left, (62, 30), (180.0, 0.0)),
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

    #[test]
    fn closed_window_layout_returns_none_without_panicking() {
        let win = Window::new(None);
        let ws = Workspace::new();
        assert!(win.layout(0).is_none());
        assert!(win.layout(2).is_none());
        assert_eq!(win.content_rect(1), Rect::new(0.0, 0.0, 0.0, 0.0));
        assert!(win.review_rect(&ws).is_none());
        assert!(!win.diff_visible(0));
        assert!(!win.sidebar_shown(&ws));
        assert!(!win.sidebar_contains(&ws, 0.0, 0.0));
        assert_eq!(win.hit_test(&ws, 0.0, 0.0), Hit::Nothing);
    }
}
