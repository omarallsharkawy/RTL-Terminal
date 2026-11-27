//! Events the event loop receives from shells and terminals (through the event proxy).

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use super::App;
use crate::control::{Request, Response};
use crate::layout::PaneId;
use crate::session::pane::Notification;
use crate::shell_integration::{MarkKind, ShellEvent, Signal};

#[derive(Debug)]
pub enum AppEvent {
    /// The shell of a pane wrote output; it is waiting in the pane's queue.
    PtyOutput(PaneId),
    Terminal(PaneId, TermEvent),
    PtyExit(PaneId, Option<u32>),
    SshAttached(PaneId),
    /// A request from the local control socket, with its reply channel and cancellation flag.
    Control(Request, mpsc::Sender<Response>, Arc<AtomicBool>),
    /// Listening TCP ports of each pane's processes, from the port scanner thread.
    Ports(HashMap<PaneId, Vec<u16>>),
    /// Git branch names resolved off the UI thread for sidebar tab directories.
    Branches(HashMap<PathBuf, Option<String>>),
    /// The review panel's changes, loaded on a worker thread (tagged with its load number).
    Review(
        u64,
        Result<crate::review::Snapshot, crate::review::LoadError>,
    ),
    GitDiff(u64, Option<crate::ui::git_diff::DiffStat>),
    /// Shell history loaded off the UI thread for the command composer.
    ComposerHistory(Vec<String>),
    /// Agent skills, slash commands and MCP servers found for the composer's `/` menu.
    ComposerSlash(Box<crate::composer::slash::SlashCatalog>),
    /// Directory entries loaded off the UI thread for composer path completion.
    ComposerDir(PathBuf, Vec<crate::composer::complete::DirEntryInfo>),
    /// Dynamic completion generator results produced off the UI thread.
    ComposerGenerator(
        crate::composer::generators::GeneratorKey,
        Vec<crate::composer::generators::GeneratedItem>,
    ),
    /// `$PATH` executables and/or local path-existence checks resolved off the UI thread.
    ComposerPathInfo {
        commands: Option<std::collections::HashSet<String>>,
        existing: Vec<(PathBuf, bool)>,
    },
    /// Project directories (MRU + scanned git repos) resolved off the UI thread.
    Projects(u64, Vec<crate::projects::ProjectItem>),
    /// Result of a non-interactive built-in AI CLI request.
    AiComplete(u64, crate::ai::AiResult),
    /// Event emitted by the agent engine thread (`twitty_ai::engine::model`).
    Agent(twitty_ai::engine::model::AgentEvent),
    /// Assembled `ContextBundle` returned off-thread from `twitty_ai::context::assemble`.
    AgentContext(u64, twitty_ai::engine::model::ContextBundle),
    /// Project memory snapshot loaded off the UI thread for the Memory screen.
    MemorySnapshot(u64, twitty_store::ProjectMemorySnapshot),
    /// Off-thread Settings → AI keyring, CLI PATH, and external themes probe result.
    SettingsAiProbed {
        generation: u64,
        detected_cli_agents: Vec<String>,
        saved_keys: [bool; 3],
        external_themes: Option<Vec<String>>,
        notice: Option<(bool, String)>,
        restart_engine: bool,
    },
    /// Off-thread Agent engine config built from `$PATH` + system keyring.
    AgentEngineConfigReady {
        generation: u64,
        config: Box<twitty_ai::engine::EngineConfig>,
        detected_cli_agents: Vec<String>,
    },
    /// Off-thread `$PATH` probe of installed local coding-agent CLIs.
    AgentClisProbed(Vec<String>),
    /// Assembled context ready to start an agent session without blocking the UI thread.
    AgentStartReady {
        generation: u64,
        session: twitty_ai::engine::model::SessionId,
        request: String,
        cwd: PathBuf,
        context: twitty_ai::engine::model::ContextBundle,
    },
    /// Project + config workflows loaded off the UI thread for palette & composer.
    WorkflowsLoaded {
        cwd: Option<PathBuf>,
        workflows: Box<crate::workflows::WorkflowSet>,
    },
}

/// Longest title kept from a program, in characters.
const MAX_TITLE_CHARS: usize = 256;

/// Maximum bytes accepted from a program's OSC 52 clipboard write (1 MiB).
pub const MAX_OSC52_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Accept an OSC 52 clipboard payload only if it stays within [`MAX_OSC52_CLIPBOARD_BYTES`].
pub fn accept_osc52_clipboard(text: String) -> Option<String> {
    (text.len() <= MAX_OSC52_CLIPBOARD_BYTES).then_some(text)
}

/// A program-set title as stored: no control, bidi-override or line-separator characters, at
/// most `MAX_TITLE_CHARS`. It is shaped for the tab bar on every frame, so a multi-megabyte one
/// would freeze drawing.
pub fn clean_title(title: &str) -> String {
    crate::shell_integration::bounded_text(
        title.chars().filter(|&c| {
            !c.is_control()
                && !crate::terminal::is_bidi_control(c)
                && !matches!(
                    c,
                    '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
                )
        }),
        MAX_TITLE_CHARS,
    )
}

impl App {
    /// A shell signal from pane `id` (see `shell_integration`).
    pub(super) fn on_shell_event(&mut self, id: PaneId, event: ShellEvent) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let running_block_id = pane
            .blocks
            .blocks()
            .next_back()
            .filter(|b| b.is_running())
            .map(crate::blocks::Block::id);
        pane.blocks.on_event(&event, &pane.terminal, Instant::now());
        let command_ended = matches!(event.signal, Signal::Mark(MarkKind::CommandEnd { .. }));
        let mut mark_kind = None;
        let mut cmd_line = None;
        let is_local = pane.remote.is_none();
        let mut recorded_cwd = None;
        let finished_block = running_block_id
            .and_then(|block_id| pane.blocks.block_by_id(block_id))
            .filter(|b| !b.is_running())
            .cloned();
        let finished_cwd = finished_block.as_ref().and_then(|_| pane.meta.cwd.clone());
        let meta = &mut pane.meta;
        match event.signal {
            Signal::Cwd(path) => {
                if is_local {
                    recorded_cwd = Some(path.clone());
                }
                meta.cwd = Some(path);
                self.mark_session_dirty();
            }
            Signal::Notify { title, body } => {
                self.notify(id, title, body);
                return;
            }
            Signal::CommandLine(line) => {
                cmd_line = Some(line.clone());
                // Leading spaces only mark ignorespace commands for history; the card shows
                // the command itself.
                meta.last_command = Some(line.trim_start().to_owned());
            }
            Signal::Mark(kind) => {
                meta.prompt_state.on_mark(kind);
                mark_kind = Some(kind);
                match kind {
                    MarkKind::OutputStart => meta.running = true,
                    MarkKind::CommandEnd { exit } => {
                        meta.running = false;
                        meta.last_exit = exit;
                        self.git_diff.refresh(id);
                    }
                    MarkKind::PromptStart | MarkKind::CommandStart => {}
                }
            }
        }
        // A command typed with a leading space stays out of every history (ignorespace).
        if let Some(line) = cmd_line.filter(|l| !l.starts_with(' ')) {
            self.on_composer_command_line(&line);
        }
        if let Some(kind) = mark_kind {
            self.on_composer_shell_mark(id, kind);
            self.on_welcome_shell_mark();
        }
        if let Some(cwd) = recorded_cwd {
            self.record_project_cwd(&cwd);
            self.maybe_refresh_project_env(&cwd);
        }
        if let Some(block) = &finished_block {
            crate::history::record_finished_block_with_excludes(
                &self.store,
                self.config.record_history,
                &self.config.memory_exclude,
                block,
                finished_cwd.as_deref(),
                is_local,
                self.shell_name.as_deref(),
            );
        }
        if command_ended {
            self.agent_panel_command_ended(id, finished_block.as_ref());
            self.review_command_ended(id);
        }
        self.window.request_redraw();
    }

    /// A program in pane `id` asked for the user's attention. It is marked for the sidebar
    /// unless the user is already looking at that pane.
    pub(super) fn notify(&mut self, id: PaneId, title: Option<String>, body: String) {
        self.notify_with_desktop(id, title, body, true);
    }

    /// Like [`App::notify`], allowing callers (such as `notification.create` with
    /// `send_desktop: false`) to suppress the OS desktop notification while still recording
    /// notification history and sidebar attention.
    pub(super) fn notify_with_desktop(
        &mut self,
        id: PaneId,
        title: Option<String>,
        body: String,
        send_desktop: bool,
    ) {
        self.record_notification(id, title.as_deref(), &body);
        let Some(notice) = crate::notify::unseen_notice(
            self.window_focused,
            self.focused_id(),
            id,
            title.as_deref(),
            &body,
        ) else {
            return;
        };
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        pane.meta.attention = Some(Notification {
            title: notice.title.clone(),
            body: notice.body.clone(),
            at: std::time::SystemTime::now(),
        });
        if send_desktop {
            let language = self.window.ui_language();
            self.window
                .notify(self.config.desktop_notifications, id, &notice, || {
                    self.ws
                        .tabs()
                        .iter()
                        .position(|t| t.root.contains(id))
                        .map(|i| super::window::sidebar_title(&self.ws, i, language))
                        .unwrap_or_default()
                });
        }
        self.window.request_redraw();
    }

    /// Something the terminal of pane `id` asks for: replies, title, bell, clipboard.
    pub(super) fn on_terminal_event(&mut self, id: PaneId, event: TermEvent) {
        if self.ws.pane(id).is_none() {
            return;
        }
        match event {
            TermEvent::PtyWrite(text) => self.write_to(id, text.as_bytes()),
            TermEvent::Title(title) => {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.title = Some(clean_title(&title));
                }
                self.refresh_window_title();
                self.window.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.title = None;
                }
                self.refresh_window_title();
                self.window.request_redraw();
            }
            TermEvent::Bell => self.window.bell(),
            TermEvent::ColorRequest(index, format) => {
                // OSC 4/10/11/12 queries (used by vim, fzf, delta to detect dark themes).
                // A color the program redefined itself (OSC 4) answers with its new value.
                let redefined = self.term(id, |t| t.program_palette_color(index)).flatten();
                let Some(rgb) = redefined.or_else(|| self.window.palette_rgb(index)) else {
                    return;
                };
                self.write_to(id, format(rgb).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let Some((cell_width, cell_height)) = self.window.cell_size() else {
                    return;
                };
                let Some((cols, rows)) = self.ws.pane(id).map(|p| p.size) else {
                    return;
                };
                let size = WindowSize {
                    num_lines: rows as u16,
                    num_cols: cols as u16,
                    cell_width,
                    cell_height,
                };
                self.write_to(id, format(size).as_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                // OSC 52 copy (e.g. from tmux/vim over SSH), bounded to prevent memory exhaustion.
                if let Some(text) = accept_osc52_clipboard(text) {
                    self.clipboard.set_text(text);
                }
            }
            // OSC 52 reads are refused: any program could silently exfiltrate the clipboard.
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}
