//! Events the event loop receives from shells and terminals (through the event proxy).

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use super::App;
use crate::control::{Request, Response};
use crate::layout::PaneId;
use crate::pty::Pty;
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

/// Longest stretch spent parsing output before the screen is redrawn.
const OUTPUT_BUDGET: Duration = Duration::from_millis(12);

impl App {
    /// Output from the shell of pane `id`: everything queued since the last event, in one go.
    pub(super) fn on_pty_output(&mut self, id: PaneId) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let term = &mut pane.terminal;
        let before = term.cursor_position();
        // Keep taking output that arrives meanwhile, but leave time for a frame: a flood never
        // freezes the screen, and an idle shell is drawn as soon as its output is parsed.
        let start = Instant::now();
        let mut any = false;
        loop {
            let data = pane.pty.as_ref().map(Pty::take_output).unwrap_or_default();
            if data.is_empty() {
                break;
            }
            any = true;
            term.process_bytes(&data);
            if start.elapsed() >= OUTPUT_BUDGET {
                break;
            }
        }
        if !any {
            return;
        }
        let events = term.take_shell_events();
        pane.blocks.refresh(&pane.terminal);
        let term = &pane.terminal;
        let moved = term.cursor_position() != before;
        // Output that moves the cursor keeps it solid while text is appearing.
        if moved && self.focused_id() == Some(id) {
            self.cursor_blink.reset(Instant::now());
        }
        if self.ws.is_visible(id) {
            self.window.request_redraw();
        }
        // The output may have replaced the link under a Ctrl-hovering pointer.
        if self.modifiers.control_key() && !self.palette_open() && !self.settings_open() {
            self.update_link_hover(id);
        }
        for event in events {
            self.on_shell_event(id, event);
        }
    }

    /// A shell signal from pane `id` (see `shell_integration`).
    fn on_shell_event(&mut self, id: PaneId, event: ShellEvent) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        pane.blocks.on_event(&event, &pane.terminal, Instant::now());
        let command_ended = matches!(event.signal, Signal::Mark(MarkKind::CommandEnd { .. }));
        let mut mark_kind = None;
        let mut cmd_line = None;
        let is_local = pane.remote.is_none();
        let mut recorded_cwd = None;
        let meta = &mut pane.meta;
        match event.signal {
            Signal::Cwd(path) => {
                if is_local {
                    recorded_cwd = Some(path.clone());
                }
                meta.cwd = Some(path);
            }
            Signal::Notify { title, body } => {
                self.notify(id, title, body);
                return;
            }
            Signal::CommandLine(line) => {
                cmd_line = Some(line.clone());
                meta.last_command = Some(line);
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
        if let Some(line) = cmd_line {
            self.on_composer_command_line(&line);
        }
        if let Some(kind) = mark_kind {
            self.on_composer_shell_mark(id, kind);
        }
        if let Some(cwd) = recorded_cwd {
            self.record_project_cwd(&cwd);
        }
        if command_ended {
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
            self.window
                .notify(self.config.desktop_notifications, id, &notice, || {
                    self.ws
                        .tabs()
                        .iter()
                        .position(|t| t.root.contains(id))
                        .map(|i| super::window::sidebar_title(&self.ws, i))
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
