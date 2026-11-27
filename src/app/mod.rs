//! The winit application: owns the window, the workspace of tabs and panes, and the input
//! state, and routes each event to the module that handles it.
//!
//! - `event`: shell signals and terminal requests arriving through the event proxy.
//! - `output`: buffered PTY output and process exits.
//! - `window`: the window and renderer (every renderer call lives there).
//! - `panes`: opening, splitting, focusing and closing tabs and panes.
//! - `keyboard`: keys, input-method text, the search bar and shortcut actions.
//! - `palette`: the command palette and its notifications and search-all modes.
//! - `ports`: the listening-ports scanner behind the sidebar.
//! - `agent`: "Ask an agent" about the output.
//! - `review`: the code review panel (uncommitted changes, comments to an agent).
//! - `mouse`: pointer, mouse reporting, divider dragging and the wheel.
//! - `selection`: mouse selection and the clipboard.

mod agent;
mod agent_panel;
mod ai;
mod branches;
mod composer;
mod context_menu;
mod control;
mod event;
mod git_diff;
mod keyboard;
mod layouts;
mod memory;
mod mouse;
mod output;
mod palette;
mod panes;
mod ports;
mod projects;
mod review;
mod selection;
mod settings;
mod ssh;
mod tooltip;
mod welcome;
mod window;

pub use event::{accept_osc52_clipboard, clean_title, AppEvent, MAX_OSC52_CLIPBOARD_BYTES};
pub use palette::format_pane_warning;

/// Sanitize startup warning strings (from config/paths) before writing them to the terminal.
pub fn format_startup_warnings(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| crate::shell_integration::clean_text(w, 512))
        .filter(|w| !w.is_empty())
        .map(|w| format!("\x1b[33m{w}\x1b[0m\r\n"))
        .collect()
}

use alacritty_terminal::term::TermMode;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::WindowId;

use crate::blink::{BlinkState, BLINK_INTERVAL};
use crate::config::TwittyConfig;
use crate::control::ControlServer;
use crate::input::mouse::StepAccumulator;
use crate::input::pointer::PointerState;
use crate::layout::PaneId;
use crate::session::pane::Pane;
use crate::session::restore::{PreparedSession, SessionStore};
use crate::session::workspace::Workspace;
use crate::terminal::Terminal;
use crate::ui::clipboard::Clipboard;
use window::Window;

pub struct App {
    proxy: EventLoopProxy<AppEvent>,
    ws: Workspace,
    output: output::OutputWork,
    window: Window,
    modifiers: ModifiersState,
    pointer: PointerState,
    clipboard: Clipboard,
    config: TwittyConfig,
    config_dirty: bool,
    /// Compiled keyboard shortcut table (defaults + `config.keybindings`), updated live.
    keybindings: crate::input::keybindings::KeyBindings,
    session: SessionStore,
    startup_session: Option<PreparedSession>,
    startup_ssh: Option<crate::remote::Target>,
    /// An IME composition (preedit) is on screen.
    preedit_active: bool,
    cursor_blink: BlinkState,
    /// Fractional Ctrl+wheel notches not yet turned into zoom or opacity steps.
    zoom_steps: StepAccumulator,
    /// The window has keyboard focus (notifications from the focused pane are then seen).
    window_focused: bool,
    /// The command palette, while it is open (keys and IME text go to it).
    palette: Option<palette::OpenPalette>,
    /// The settings overlay, while it is open (keys, IME text and pointer input go to it).
    settings: Option<crate::ui::settings_panel::SettingsPanelState>,
    /// The project memory overlay, while it is open.
    memory: Option<crate::ui::memory_panel::MemoryPanelState>,
    /// Generation counter for off-thread project memory snapshots.
    memory_generation: u64,
    /// Project roots whose environment notes have already been refreshed in this session.
    refreshed_env_projects: std::collections::HashSet<std::path::PathBuf>,
    /// The first-run welcome wizard, while it is open.
    welcome: Option<crate::ui::welcome::WelcomeState>,
    /// Local control socket server (`twitty ctl`), when enabled.
    control: Option<ControlServer>,
    /// The last notifications of this window, for the palette's notifications mode.
    notification_history: crate::ui::notifications::NotificationHistory,
    /// The thread scanning listening ports while the sidebar is shown.
    ports: ports::PortWatch,
    /// The thread resolving git branches off the UI thread while the sidebar is shown.
    branches: branches::BranchWatch,
    /// The code review panel.
    review: crate::review::ReviewPanel,
    /// The docked Agent side panel (round 10).
    agent_panel: agent_panel::AgentPanelHost,
    git_diff: git_diff::GitDiffWatch,
    /// The docked command composer.
    composer: composer::ComposerHost,
    /// An open right-click menu.
    context_menu: Option<context_menu::ContextMenuHost>,
    tooltip: tooltip::TooltipState,
    /// Built-in non-interactive AI assistant (composer generation, block explain/fix, consent).
    ai: ai::AiHost,
    /// Project switcher MRU and off-thread git repo scan state.
    projects: projects::ProjectState,
    /// Persistent store of named tab layouts.
    layouts: crate::layouts::LayoutStore,
    /// Off-thread SQLite store (command history, memory, Drive index).
    store: twitty_store::StoreHandle,
    /// Basename of the user's configured shell for history metadata.
    shell_name: Option<String>,
    /// Bounded stack of recently closed tabs for reopening.
    closed_tabs: crate::session::closed::ClosedTabs,
    /// Per-tab broadcast input toggle and Esc-Esc state.
    broadcast: crate::input::broadcast::BroadcastState,
}

impl App {
    pub fn new(proxy: EventLoopProxy<AppEvent>) -> Self {
        let perf = crate::perf::Perf::from_env();
        let config = TwittyConfig::load();
        let keybindings = crate::input::keybindings::KeyBindings::from_config(&config.keybindings);
        let session = SessionStore::new();
        let startup_session = if config.restore_session {
            session.load()
        } else {
            None
        };
        Self {
            proxy,
            ws: Workspace::new(),
            output: output::OutputWork::default(),
            window: Window::new(perf),
            modifiers: ModifiersState::empty(),
            pointer: PointerState::default(),
            clipboard: Clipboard::default(),
            config,
            config_dirty: false,
            keybindings,
            session,
            startup_session,
            startup_ssh: None,
            preedit_active: false,
            cursor_blink: BlinkState::new(BLINK_INTERVAL),
            zoom_steps: StepAccumulator::default(),
            // Until the system says the window has focus (it does as soon as it gets it).
            window_focused: false,
            palette: None,
            settings: None,
            memory: None,
            memory_generation: 0,
            refreshed_env_projects: std::collections::HashSet::new(),
            welcome: None,
            control: None,
            notification_history: Default::default(),
            ports: ports::PortWatch::default(),
            branches: branches::BranchWatch::default(),
            review: Default::default(),
            agent_panel: agent_panel::AgentPanelHost::default(),
            git_diff: git_diff::GitDiffWatch::default(),
            composer: composer::ComposerHost::default(),
            context_menu: None,
            tooltip: tooltip::TooltipState::default(),
            ai: ai::AiHost::load(),
            projects: projects::ProjectState::load(),
            layouts: crate::layouts::LayoutStore::default(),
            store: twitty_store::StoreHandle::open_default(),
            shell_name: crate::history::detect_shell_name(),
            closed_tabs: crate::session::closed::ClosedTabs::new(),
            broadcast: crate::input::broadcast::BroadcastState::new(),
        }
    }

    pub fn with_startup_ssh(mut self, target: crate::remote::Target) -> Self {
        self.startup_ssh = Some(target);
        self.startup_session = None;
        self
    }

    fn focused_id(&self) -> Option<PaneId> {
        self.ws.focused_id()
    }

    /// Run `f` on the terminal of pane `id`.
    fn term<R>(&mut self, id: PaneId, f: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        self.ws.pane_mut(id).map(|p| f(&mut p.terminal))
    }

    fn mode_of(&self, id: PaneId) -> TermMode {
        self.ws.pane(id).map_or(TermMode::NONE, Pane::mode)
    }

    fn write_to(&self, id: PaneId, bytes: &[u8]) {
        if let Some(pane) = self.ws.pane(id) {
            pane.write(bytes);
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
            self.window.request_redraw();
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_open() {
            return;
        }
        if !self.window.open(event_loop, &self.config) {
            return;
        }
        self.start_control_server();
        self.start_session();
        if self.ws.is_empty() {
            self.window.close();
            event_loop.exit();
            return;
        }
        // Config and session problems found while starting.
        let warnings = crate::config::take_warnings();
        if let Some(id) = self.ws.focused_id().filter(|_| !warnings.is_empty()) {
            let text = format_startup_warnings(&warnings);
            if !text.is_empty() {
                self.term(id, |t| t.process_bytes(text.as_bytes()));
            }
        }
        self.maybe_open_first_run_welcome();
        self.window.request_redraw_now();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::PtyOutput(id) => self.output.schedule.queue(id),
            AppEvent::Terminal(id, event) => self.on_terminal_event(id, event),
            AppEvent::PtyExit(id, status) => self.on_pty_exit(event_loop, id, status),
            AppEvent::SshAttached(id) => self.on_ssh_attached(id),
            AppEvent::Control(request, reply, cancelled) => {
                // A read can observe already-queued output, within the same work budget.
                self.poll_pty_output(event_loop);
                if self.on_control_request(request, reply, cancelled) {
                    event_loop.exit();
                }
            }
            AppEvent::Ports(ports) => self.on_ports(ports),
            AppEvent::Branches(branches) => self.on_branches(branches),
            AppEvent::Review(generation, result) => self.on_review_loaded(generation, result),
            AppEvent::GitDiff(generation, stat) => self.on_git_diff(generation, stat),
            AppEvent::ComposerHistory(entries) => self.on_composer_history(entries),
            AppEvent::ComposerSlash(catalog) => self.on_composer_slash(*catalog),
            AppEvent::ComposerDir(dir, entries) => self.on_composer_dir(dir, entries),
            AppEvent::ComposerGenerator(key, items) => self.on_composer_generator(key, items),
            AppEvent::ComposerPathInfo { commands, existing } => {
                self.on_composer_path_info(commands, existing)
            }
            AppEvent::Projects(generation, items) => self.on_projects_loaded(generation, items),
            AppEvent::AiComplete(request_id, result) => self.on_ai_complete(request_id, result),
            AppEvent::Agent(event) => self.on_agent_event(event),
            AppEvent::AgentContext(generation, bundle) => {
                self.on_agent_context_assembled(generation, bundle)
            }
            AppEvent::MemorySnapshot(generation, snapshot) => {
                self.on_memory_snapshot(generation, snapshot)
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_pty_output(event_loop);
        let now = Instant::now();
        let output_due = self.output.schedule.end_turn(now);
        let (blinking, shown) = if self.window.is_occluded() {
            (false, false)
        } else if self.composer_has_keys() || self.agent_panel_has_keys() {
            (self.config.cursor_blink && !self.config.reduce_motion, true)
        } else {
            self.focused_id()
                .and_then(|id| {
                    self.term(id, |t| {
                        (
                            t.cursor_blinking(),
                            t.mode().contains(TermMode::SHOW_CURSOR),
                        )
                    })
                })
                .unwrap_or((false, false))
        };
        self.cursor_blink.set_enabled(now, blinking && shown);
        self.cursor_blink.tick(now);

        if !self.window.is_occluded() && self.window.blink_differs(self.cursor_blink.visible()) {
            self.window.request_redraw();
        }
        let search_due = self.poll_search_all(now);
        let selection_due = self.poll_selection_clears(now);
        let ssh_due = self.poll_ssh_retries(now);
        let deadline = [
            self.cursor_blink.next_deadline(),
            output_due,
            search_due,
            selection_due,
            ssh_due,
            self.poll_git_diff(now),
            self.poll_ai_animation(now),
            self.window.poll_motion(now),
            self.window.poll_perf(now),
            self.poll_tooltip(now),
        ]
        .into_iter()
        .flatten()
        .min();
        event_loop.set_control_flow(match deadline {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.ai.cancel_active();
        self.finish_projects();
        self.session.finish(&self.ws, self.window.sidebar_visible());
        self.control = None;
        self.store
            .flush_timeout(std::time::Duration::from_millis(250));
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
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                self.window.resize(size.width, size.height);
                self.sync_palette();
                self.sync_settings();
                self.sync_memory();
                self.sync_welcome();
                self.apply_layout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.window.set_scale_factor(scale_factor as f32);
                self.sync_palette();
                self.sync_settings();
                self.sync_memory();
                self.sync_welcome();
                self.apply_layout();
            }
            WindowEvent::Occluded(occluded) => self.window.set_occluded(occluded),
            WindowEvent::CursorLeft { .. } => self.on_cursor_left(),
            WindowEvent::Focused(focused) => self.on_window_focus(focused),
            WindowEvent::ModifiersChanged(mods) => self.on_modifiers(mods.state()),
            WindowEvent::CursorMoved { position, .. } => {
                self.on_cursor_moved(position.x, position.y)
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.on_mouse_input(button, state == ElementState::Pressed) {
                    event_loop.exit();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_wheel(delta),
            WindowEvent::Ime(ime) => self.on_ime(ime),
            // While an IME composes, keys belong to the IME.
            WindowEvent::KeyboardInput { .. }
                if !crate::ime::should_forward_key(self.preedit_active) => {}
            WindowEvent::KeyboardInput { event, .. } => {
                if self.on_key(&event) {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}
