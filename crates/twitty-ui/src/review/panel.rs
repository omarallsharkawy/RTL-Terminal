//! The review panel's state: whether it is open and focused, the loaded changes, scroll and
//! selection, collapsed files and the comment box. Keys and clicks come in as plain values
//! and come out as a [`PanelCmd`] for the app to carry out, so all of it is tested without a
//! window. [`ReviewPanel::view`] turns the visible part into a [`View`] for drawing.

use std::collections::HashSet;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use super::comment::{CommentTarget, MAX_COMMENT_CHARS};
use super::diff::{FileDiff, FileStatus, LineKind, OneLine};
use super::git::{LoadError, Snapshot};
use super::layout::PanelHit;
use super::rows::{
    build_rows, clamp_top, comment_line, nearest, next_file, reveal, step, LineRef, Note, Row,
};
use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;

/// The commit button's label.
pub fn commit_label() -> &'static str {
    crate::ui::chrome::t("Commit")
}
/// Tab stops in diff lines.
pub const TAB_WIDTH: usize = 4;
/// Most cells of a row's text drawn (the panel clips the rest anyway).
pub const MAX_ROW_COLS: usize = 400;

/// Where the changes come from, or why there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    Idle,
    Loading,
    Ready,
    /// The focused pane has no known local folder (a remote pane, or no shell report yet).
    NoFolder,
    NotRepo,
    NoGit,
    Failed(String),
}

/// What the app should do after a key or click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelCmd {
    None,
    /// Only the panel changed: draw again.
    Redraw,
    /// Give the keyboard back to the terminal (the panel stays open).
    Unfocus,
    Close,
    Refresh,
    /// Send the comment to the agent pane.
    Submit(CommentTarget),
    /// No agent pane was found and the user confirmed: start one with this comment.
    StartAgent(CommentTarget),
    /// Type `git commit` into the focused pane.
    Commit,
    /// Stage the selected file (`git add -- <path>`) off the UI thread and refresh.
    StageFile(String),
    /// Unstage the selected file (`git reset HEAD -- <path>`) off the UI thread and refresh.
    UnstageFile(String),
}

/// A one-line message under the rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub warn: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Editor {
    anchor: LineRef,
    text: String,
    /// No agent pane was found; Enter now starts one.
    offer_agent: bool,
}

/// How a row is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStyle {
    File {
        collapsed: bool,
        status: FileStatus,
    },
    Note,
    Hunk,
    Context,
    Add,
    Del,
    CommentInput,
    CommentHint,
    /// A state message in an empty body (loading, no changes, errors).
    Message,
}

/// One visible row, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewRow {
    pub style: RowStyle,
    /// Line numbers (diff lines only).
    pub gutter: String,
    pub text: String,
    /// `+`/`-` counts drawn at the right end (file rows only).
    pub counts: Option<(usize, usize)>,
    pub selected: bool,
}

/// What the panel draws this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub rect: Rect,
    pub focused: bool,
    /// Repository display name and optional branch.
    pub title: String,
    pub branch: Option<String>,
    /// File count.
    pub summary: String,
    /// Added and removed lines over all files, when loaded.
    pub totals: Option<(usize, usize)>,
    pub commit_label: &'static str,
    /// Width of the line-number gutter in cells (0 without diff lines).
    pub gutter_cols: usize,
    /// Visible rows, top to bottom.
    pub rows: Vec<ViewRow>,
    pub notice: Option<Notice>,
}

#[derive(Debug, Default)]
pub struct ReviewPanel {
    open: bool,
    focused: bool,
    load: Option<Load>,
    snapshot: Option<Snapshot>,
    /// A result that arrived while a comment was being typed (applied when it closes).
    waiting: Option<Result<Snapshot, LoadError>>,
    /// Collapsed files by path, kept across refreshes.
    collapsed: HashSet<String>,
    rows: Vec<Row>,
    gutter_cols: usize,
    top: usize,
    selected: Option<usize>,
    editor: Option<Editor>,
    notice: Option<Notice>,
    generation: u64,
    in_flight: bool,
    rerun: bool,
    /// Wheel movement not yet a whole row.
    wheel: f64,
}

fn notice(text: impl Into<String>, warn: bool) -> Option<Notice> {
    Some(Notice {
        text: text.into(),
        warn,
    })
}

impl ReviewPanel {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel takes keys (it is open and was clicked or just opened).
    pub fn is_focused(&self) -> bool {
        self.open && self.focused
    }

    /// A comment is being typed.
    pub fn is_editing(&self) -> bool {
        self.is_focused() && self.editor.is_some()
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused && self.open;
    }

    pub fn open(&mut self) {
        self.open = true;
        self.focused = true;
    }

    /// Close the panel, dropping a comment being typed.
    pub fn close(&mut self) {
        self.open = false;
        self.focused = false;
        self.editor = None;
        self.notice = None;
        self.rebuild();
    }

    pub fn load_state(&self) -> Load {
        self.load.clone().unwrap_or(Load::Idle)
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Cached line-number gutter width in cells for the currently applied snapshot (0 without one).
    pub fn gutter_cols(&self) -> usize {
        self.gutter_cols
    }

    pub fn top(&self) -> usize {
        self.top
    }

    /// The first row drawn with `page` body rows (what `view` shows and hits must use).
    pub fn visible_top(&self, page: usize) -> usize {
        clamp_top(self.top, self.rows.len(), page)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Start loading: the generation to tag the result with, or `None` when a load is already
    /// running (it is then repeated once it ends).
    pub fn begin_refresh(&mut self) -> Option<u64> {
        if self.in_flight {
            self.rerun = true;
            return None;
        }
        self.in_flight = true;
        self.generation += 1;
        if self.snapshot.is_none() {
            self.load = Some(Load::Loading);
        }
        Some(self.generation)
    }

    /// Nothing to load: the focused pane has no local folder.
    pub fn set_no_folder(&mut self) {
        self.generation += 1;
        self.in_flight = false;
        self.rerun = false;
        self.waiting = None;
        self.editor = None;
        self.snapshot = None;
        self.gutter_cols = 0;
        self.load = Some(Load::NoFolder);
        self.rebuild();
    }

    /// A load ended. Returns true when another one was asked for meanwhile.
    pub fn finish_refresh(&mut self, generation: u64, result: Result<Snapshot, LoadError>) -> bool {
        if generation != self.generation {
            return false;
        }
        self.in_flight = false;
        if self.editor.is_some() {
            self.waiting = Some(result);
        } else {
            self.apply(result);
        }
        std::mem::take(&mut self.rerun)
    }

    fn apply(&mut self, result: Result<Snapshot, LoadError>) {
        // Keep the selection on the same line (or file) and at the same height.
        let anchor = self.selected.and_then(|i| self.row_key(i));
        let offset = self.selected.map(|s| s.saturating_sub(self.top));
        match result {
            Ok(snapshot) => {
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|old| old.root != snapshot.root)
                {
                    self.collapsed.clear();
                    self.top = 0;
                    self.selected = None;
                }
                let digits = snapshot.diff.max_line_number().to_string().len().max(3);
                self.gutter_cols = 2 * digits + 1;
                self.snapshot = Some(snapshot);
                self.load = Some(Load::Ready);
            }
            Err(err) => {
                self.snapshot = None;
                self.gutter_cols = 0;
                self.load = Some(match err {
                    LoadError::NotRepo => Load::NotRepo,
                    LoadError::NoGit => Load::NoGit,
                    LoadError::Failed(why) => Load::Failed(why),
                });
            }
        }
        self.rebuild();
        if let Some(key) = anchor {
            if let Some(i) = (0..self.rows.len()).find(|&i| self.row_key(i).as_ref() == Some(&key))
            {
                self.selected = Some(i);
                self.top = i.saturating_sub(offset.unwrap_or(0));
            }
        }
    }

    /// What identifies a row across refreshes: the file path and the line's numbers.
    fn row_key(&self, index: usize) -> Option<(String, Option<u32>, Option<u32>)> {
        let snapshot = self.snapshot.as_ref()?;
        match *self.rows.get(index)? {
            Row::File(f) => Some((snapshot.diff.files.get(f)?.path().to_owned(), None, None)),
            Row::Line(r) => {
                let file = snapshot.diff.files.get(r.file)?;
                let line = file.hunks.get(r.hunk)?.lines.get(r.line)?;
                Some((file.path().to_owned(), line.old, line.new))
            }
            _ => None,
        }
    }

    fn rebuild(&mut self) {
        self.rows = match &self.snapshot {
            Some(s) => {
                let files = &s.diff.files;
                let collapsed = &self.collapsed;
                build_rows(
                    &s.diff,
                    |f| collapsed.contains(files[f].path()),
                    self.editor.as_ref().map(|e| e.anchor),
                )
            }
            None => Vec::new(),
        };
        self.selected = self
            .selected
            .filter(|&s| self.rows.get(s).is_some_and(Row::selectable))
            .or_else(|| self.selected.and_then(|s| nearest(&self.rows, s)));
    }

    fn collapse(&mut self, file: usize, collapsed: bool) {
        let Some(path) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.diff.files.get(file))
            .map(|f| f.path().to_owned())
        else {
            return;
        };
        if collapsed {
            self.collapsed.insert(path.clone());
            if self.editor.as_ref().is_some_and(|e| e.anchor.file == file) {
                self.close_comment();
                self.selected = self.file_row_by_path(&path);
                return;
            }
        } else {
            self.collapsed.remove(&path);
        }
        self.rebuild();
        self.selected = self.rows.iter().position(|r| *r == Row::File(file));
    }

    fn file_row_by_path(&self, path: &str) -> Option<usize> {
        let snapshot = self.snapshot.as_ref()?;
        self.rows.iter().position(|r| match *r {
            Row::File(f) => snapshot
                .diff
                .files
                .get(f)
                .is_some_and(|file| file.path() == path),
            _ => false,
        })
    }

    fn file_collapsed(&self, file: usize) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|s| s.diff.files.get(file))
            .is_some_and(|f| self.collapsed.contains(f.path()))
    }

    fn select(&mut self, index: Option<usize>, page: usize) {
        self.selected = index;
        if let Some(i) = index {
            self.top = reveal(self.top, i, page);
        }
        self.top = clamp_top(self.top, self.rows.len(), page);
    }

    /// Open the comment box under diff line row `index`.
    fn open_comment(&mut self, index: usize, page: usize) -> PanelCmd {
        let Some(Row::Line(anchor)) = self.rows.get(index).copied() else {
            return PanelCmd::None;
        };
        self.focused = true;
        self.editor = Some(Editor {
            anchor,
            text: String::new(),
            offer_agent: false,
        });
        self.notice = None;
        self.rebuild();
        let at = self.rows.iter().position(|r| *r == Row::Line(anchor));
        self.selected = at;
        if let Some(at) = at {
            // Show the line and the box under it.
            let end = (at + 2).min(self.rows.len().saturating_sub(1));
            self.top = reveal(reveal(self.top, end, page), at, page);
        }
        self.top = clamp_top(self.top, self.rows.len(), page);
        PanelCmd::Redraw
    }

    /// Close the comment box (sent or cancelled) and apply a refresh that waited for it.
    fn close_comment(&mut self) {
        self.editor = None;
        if let Some(result) = self.waiting.take() {
            self.apply(result);
        } else {
            self.rebuild();
        }
    }

    /// The comment was sent to an agent (`agent`: its program, when known): close the box and
    /// say where it went.
    pub fn comment_sent(&mut self, agent: Option<&str>) {
        self.close_comment();
        let text = match agent {
            Some(agent) => twitty_core::language::fill(
                crate::ui::chrome::t("Sent to {}: review it there and press Enter"),
                &[&agent],
            ),
            None => crate::ui::chrome::t("Sent to the agent pane: review it there and press Enter")
                .to_owned(),
        };
        self.notice = notice(text, false);
    }

    /// No agent pane was found: keep the comment and offer to start one.
    pub fn offer_agent(&mut self) {
        if let Some(editor) = self.editor.as_mut() {
            editor.offer_agent = true;
        }
    }

    /// Start an agent was asked for: close the box.
    pub fn agent_started(&mut self, agent: &str) {
        self.close_comment();
        self.notice = notice(
            twitty_core::language::fill(
                crate::ui::chrome::t("Started {} with the comment"),
                &[&agent],
            ),
            false,
        );
    }

    /// Show a warning under the rows.
    pub fn warn(&mut self, text: impl Into<String>) {
        self.notice = notice(text, true);
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// The comment being typed.
    pub fn comment_text(&self) -> Option<&str> {
        self.editor.as_ref().map(|e| e.text.as_str())
    }

    /// Text typed (or committed by an input method, or pasted) while the panel has focus.
    pub fn text_input(&mut self, text: &str) -> PanelCmd {
        let Some(editor) = self.editor.as_mut().filter(|_| self.focused) else {
            return PanelCmd::None;
        };
        let room = MAX_COMMENT_CHARS.saturating_sub(editor.text.chars().count());
        let before = editor.text.len();
        editor.text.extend(
            text.chars()
                .map(|c| {
                    if matches!(c, '\n' | '\r' | '\t') {
                        ' '
                    } else {
                        c
                    }
                })
                .filter(|&c| {
                    !c.is_control()
                        && !crate::terminal::is_bidi_control(c)
                        && !matches!(c, '\u{2028}' | '\u{2029}')
                })
                .take(room),
        );
        if editor.text.len() != before {
            PanelCmd::Redraw
        } else {
            PanelCmd::None
        }
    }

    fn submit(&mut self) -> PanelCmd {
        let Some(editor) = &self.editor else {
            return PanelCmd::None;
        };
        let comment = super::comment::clean_comment(&editor.text);
        if comment.is_empty() {
            return PanelCmd::None;
        }
        let Some(snapshot) = &self.snapshot else {
            return PanelCmd::None;
        };
        let r = editor.anchor;
        let Some((line, kind)) = comment_line(&snapshot.diff, r) else {
            return PanelCmd::None;
        };
        let file = &snapshot.diff.files[r.file];
        let target = CommentTarget {
            path: file.path().to_owned(),
            line,
            kind,
            code: file.hunks[r.hunk].lines[r.line].text.clone(),
            comment,
        };
        if editor.offer_agent {
            PanelCmd::StartAgent(target)
        } else {
            PanelCmd::Submit(target)
        }
    }

    /// A key press while the panel has focus. `physical` lets letter keys work with any layout
    /// (an Arabic layout types `ؤ` on the C key).
    pub fn key(
        &mut self,
        key: &Key,
        physical: Option<KeyCode>,
        text: Option<&str>,
        modifiers: ModifiersState,
        page: usize,
    ) -> PanelCmd {
        if !self.is_focused() {
            return PanelCmd::None;
        }
        let page = page.max(1);
        if self.editor.is_some() {
            return match key {
                Key::Named(NamedKey::Enter) => self.submit(),
                Key::Named(NamedKey::Escape) => {
                    self.close_comment();
                    PanelCmd::Redraw
                }
                Key::Named(NamedKey::Backspace) => {
                    if let Some(editor) = self.editor.as_mut() {
                        if let Some((i, _)) = editor.text.grapheme_indices(true).next_back() {
                            editor.text.truncate(i);
                        }
                    }
                    PanelCmd::Redraw
                }
                _ if modifiers.control_key() || modifiers.super_key() => PanelCmd::None,
                _ => text.map_or(PanelCmd::None, |t| self.text_input(t)),
            };
        }
        if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
            return PanelCmd::None;
        }
        let letter = letter_of(key, physical);
        let len = self.rows.len();
        match (key, letter) {
            (Key::Named(NamedKey::Escape), _) => PanelCmd::Unfocus,
            (Key::Named(NamedKey::ArrowDown), _) | (_, Some('j')) => {
                let next = step(&self.rows, self.selected, 1);
                self.select(next, page);
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::ArrowUp), _) | (_, Some('k')) => {
                let next = step(&self.rows, self.selected, -1);
                self.select(next, page);
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::PageDown), _) => {
                self.top = clamp_top(self.top + page, len, page);
                let target = self.selected.map_or(self.top, |s| s + page);
                let next = nearest(&self.rows, target.max(self.top));
                self.select(next, page);
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::PageUp), _) => {
                self.top = self.top.saturating_sub(page);
                let target = self.selected.map_or(self.top, |s| s.saturating_sub(page));
                let next = nearest(&self.rows, target);
                self.select(next, page);
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::Home), _) => {
                self.top = 0;
                let first = step(&self.rows, None, 0);
                self.select(first, page);
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::End), _) => {
                let last = self.rows.iter().rposition(Row::selectable);
                self.select(last, page);
                self.top = clamp_top(len, len, page);
                PanelCmd::Redraw
            }
            (_, Some('n')) => {
                let next = next_file(&self.rows, self.selected, true);
                self.select(next, page);
                // A file header goes to the top, so its lines follow it.
                if let Some(i) = next {
                    self.top = clamp_top(i, len, page);
                }
                PanelCmd::Redraw
            }
            (_, Some('p')) => {
                let prev = next_file(&self.rows, self.selected, false);
                self.select(prev, page);
                if let Some(i) = prev {
                    self.top = clamp_top(i, len, page);
                }
                PanelCmd::Redraw
            }
            (Key::Named(NamedKey::ArrowLeft), _) => match self.current_file() {
                Some(f) => {
                    self.collapse(f, true);
                    let at = self.selected;
                    self.select(at, page);
                    PanelCmd::Redraw
                }
                None => PanelCmd::None,
            },
            (Key::Named(NamedKey::ArrowRight), _) => match self.current_file() {
                Some(f) if self.file_collapsed(f) => {
                    self.collapse(f, false);
                    let at = self.selected;
                    self.select(at, page);
                    PanelCmd::Redraw
                }
                _ => PanelCmd::None,
            },
            (Key::Named(NamedKey::Enter | NamedKey::Space), _) => {
                match self.selected.and_then(|i| self.rows.get(i).copied()) {
                    Some(Row::File(f)) => {
                        let collapsed = !self.file_collapsed(f);
                        self.collapse(f, collapsed);
                        let at = self.selected;
                        self.select(at, page);
                        PanelCmd::Redraw
                    }
                    Some(Row::Line(_)) if *key == Key::Named(NamedKey::Enter) => {
                        let at = self.selected.unwrap_or(0);
                        self.open_comment(at, page)
                    }
                    _ => PanelCmd::None,
                }
            }
            (_, Some('c')) => match self.selected {
                Some(at) => self.open_comment(at, page),
                None => PanelCmd::None,
            },
            (_, Some('s')) => match self.current_file_path() {
                Some(path) => PanelCmd::StageFile(path),
                None => PanelCmd::None,
            },
            (_, Some('u')) => match self.current_file_path() {
                Some(path) => PanelCmd::UnstageFile(path),
                None => PanelCmd::None,
            },
            (_, Some('r')) => PanelCmd::Refresh,
            _ => PanelCmd::None,
        }
    }

    /// Repository-relative path of the currently selected file or diff line, if any.
    fn current_file_path(&self) -> Option<String> {
        let idx = self.current_file()?;
        self.snapshot
            .as_ref()?
            .diff
            .files
            .get(idx)
            .map(|f| f.path().to_owned())
    }

    /// The file of the selected row.
    fn current_file(&self) -> Option<usize> {
        self.selected
            .and_then(|i| self.rows.get(i))
            .and_then(Row::file)
    }

    /// A left click on `hit` (`page`: rows the body shows).
    pub fn click(&mut self, hit: PanelHit, page: usize) -> PanelCmd {
        if !self.open {
            return PanelCmd::None;
        }
        let page = page.max(1);
        match hit {
            PanelHit::Outside => {
                if self.focused {
                    self.focused = false;
                    return PanelCmd::Redraw;
                }
                PanelCmd::None
            }
            PanelHit::Close => PanelCmd::Close,
            PanelHit::Refresh => {
                self.focused = true;
                PanelCmd::Refresh
            }
            PanelHit::Commit => PanelCmd::Commit,
            PanelHit::Inside => {
                self.focused = true;
                PanelCmd::Redraw
            }
            PanelHit::Row { index, gutter } => {
                self.focused = true;
                match self.rows.get(index).copied() {
                    Some(Row::File(f)) => {
                        let collapsed = !self.file_collapsed(f);
                        self.collapse(f, collapsed);
                        let at = self.selected;
                        self.select(at, page);
                    }
                    Some(Row::Line(_)) if gutter => return self.open_comment(index, page),
                    Some(Row::Line(_)) => self.selected = Some(index),
                    _ => {}
                }
                PanelCmd::Redraw
            }
        }
    }

    /// Scroll by `lines` (positive: towards the top). Returns true when the view moved.
    pub fn wheel(&mut self, lines: f64, page: usize) -> bool {
        self.wheel += lines;
        let whole = self.wheel.trunc();
        if whole == 0.0 {
            return false;
        }
        self.wheel -= whole;
        let before = self.top;
        let top = self.top as f64 - whole;
        self.top = clamp_top(top.max(0.0) as usize, self.rows.len(), page);
        self.top != before
    }

    /// The panel's visible part in `rect`, with `page` body rows.
    pub fn view(&self, rect: Rect, page: usize) -> View {
        let page = page.max(1);
        let load = self.load_state();
        let (title, branch, summary, totals, gutter_cols) = match &self.snapshot {
            Some(s) => {
                let title = if s.name.is_empty() {
                    crate::ui::chrome::t("Repository").to_owned()
                } else {
                    s.name.clone()
                };
                let branch = s
                    .branch
                    .as_deref()
                    .map(|branch| crate::shell_integration::clean_text(branch, 256))
                    .filter(|branch| !branch.is_empty());
                let n = s.diff.file_count();
                let summary = format!(
                    "{n} {} / {n} {}",
                    if n == 1 { "file" } else { "files" },
                    if n == 1 {
                        UiLanguage::Arabic.t("file")
                    } else {
                        UiLanguage::Arabic.t("files")
                    }
                );
                (
                    display_text(&title, MAX_ROW_COLS),
                    branch,
                    summary,
                    Some(s.diff.totals()),
                    self.gutter_cols,
                )
            }
            None => (
                crate::ui::chrome::t("Review changes").to_owned(),
                None,
                String::new(),
                None,
                0,
            ),
        };
        let mut rows = Vec::new();
        if let Some(s) = &self.snapshot {
            let top = self.visible_top(page);
            let digits = (gutter_cols.saturating_sub(1)) / 2;
            for (i, row) in self.rows.iter().enumerate().skip(top).take(page) {
                let mut view = self.view_row(s, row, digits);
                view.selected = self.selected == Some(i);
                rows.push(view);
            }
            if self.rows.is_empty() {
                rows.push(message(crate::ui::chrome::t("No uncommitted changes")));
            }
        } else {
            rows.push(message(match &load {
                Load::Idle | Load::Loading | Load::Ready => {
                    crate::ui::chrome::t("Loading changes…").to_owned()
                }
                Load::NoFolder => crate::ui::chrome::t("This pane has no local folder").to_owned(),
                Load::NotRepo => crate::ui::chrome::t("Not a git repository").to_owned(),
                Load::NoGit => crate::ui::chrome::t("git is not installed").to_owned(),
                Load::Failed(why) => format!(
                    "{}: {}",
                    crate::ui::chrome::t("git failed"),
                    crate::shell_integration::clean_text(why, 200)
                ),
            }));
        }
        View {
            rect,
            focused: self.is_focused(),
            title,
            branch,
            summary,
            totals,
            commit_label: commit_label(),
            gutter_cols,
            rows,
            notice: self.notice.clone(),
        }
    }

    fn view_row(&self, s: &Snapshot, row: &Row, digits: usize) -> ViewRow {
        let files = &s.diff.files;
        let plain = |style, text: String| ViewRow {
            style,
            gutter: String::new(),
            text,
            counts: None,
            selected: false,
        };
        match *row {
            Row::File(f) => {
                let file = &files[f];
                let collapsed = self.collapsed.contains(file.path());
                ViewRow {
                    counts: Some((file.added, file.removed)),
                    ..plain(
                        RowStyle::File {
                            collapsed,
                            status: file.status,
                        },
                        display_text(&file.display_path(), MAX_ROW_COLS),
                    )
                }
            }
            Row::Note(f, note) => plain(RowStyle::Note, note_text(&files[f], note)),
            Row::Hunk(f, h) => plain(
                RowStyle::Hunk,
                display_text(&files[f].hunks[h].header(), MAX_ROW_COLS),
            ),
            Row::Line(r) => {
                let line = &files[r.file].hunks[r.hunk].lines[r.line];
                let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
                let (style, sign) = match line.kind {
                    LineKind::Context => (RowStyle::Context, ' '),
                    LineKind::Add => (RowStyle::Add, '+'),
                    LineKind::Del => (RowStyle::Del, '-'),
                };
                ViewRow {
                    gutter: format!(
                        "{:>digits$} {:>digits$}",
                        number(line.old),
                        number(line.new)
                    ),
                    ..plain(
                        style,
                        format!("{sign}{}", display_text(&line.text, MAX_ROW_COLS)),
                    )
                }
            }
            Row::NoNewline(_) => plain(
                RowStyle::Note,
                crate::ui::chrome::t("\\ No newline at end of file").to_owned(),
            ),
            Row::CommentInput => {
                let text = self.editor.as_ref().map_or("", |e| e.text.as_str());
                let shown = if text.is_empty() {
                    crate::ui::chrome::t("Comment for the agent…").to_owned()
                } else {
                    // The end of a long comment stays in view.
                    tail(text, MAX_ROW_COLS)
                };
                plain(RowStyle::CommentInput, format!("› {shown}▏"))
            }
            Row::CommentHint => {
                let offer = self.editor.as_ref().is_some_and(|e| e.offer_agent);
                plain(
                    RowStyle::CommentHint,
                    if offer {
                        crate::ui::chrome::t("No agent in this tab. Enter: start one with this comment · Esc: cancel")
                    } else {
                        crate::ui::chrome::t("Enter: send to the agent · Esc: cancel")
                    }
                    .to_owned(),
                )
            }
            Row::Omitted => plain(
                RowStyle::Note,
                format!(
                    "{} {}",
                    s.diff.omitted_files,
                    crate::ui::chrome::t("more files not shown")
                ),
            ),
            Row::Truncated => plain(
                RowStyle::Note,
                crate::ui::chrome::t("The diff is too large and was cut").to_owned(),
            ),
        }
    }
}

fn message(text: impl Into<String>) -> ViewRow {
    ViewRow {
        style: RowStyle::Message,
        gutter: String::new(),
        text: text.into(),
        counts: None,
        selected: false,
    }
}

fn note_text(file: &FileDiff, note: Note) -> String {
    match note {
        Note::Mode => format!(
            "{} {}",
            crate::ui::chrome::t("Mode"),
            file.mode_change().unwrap_or_default()
        ),
        Note::Empty => match file.similarity {
            Some(p) => format!("{} {p}%", crate::ui::chrome::t("Renamed, similarity")),
            None => crate::ui::chrome::t("No content changes").to_owned(),
        },
        Note::OneLine(OneLine::Binary) => crate::ui::chrome::t("Binary file").to_owned(),
        Note::OneLine(OneLine::TooLarge) => format!(
            "{} +{} −{}",
            crate::ui::chrome::t("Large change, not shown:"),
            file.added,
            file.removed
        ),
        Note::OneLine(OneLine::TooBig(bytes)) => format!(
            "{} ({} KiB)",
            crate::ui::chrome::t("Large file, not shown"),
            bytes.div_ceil(1024)
        ),
        Note::OneLine(OneLine::Special) => {
            crate::ui::chrome::t("Not a regular file (link or device)").to_owned()
        }
        Note::OneLine(OneLine::Unreadable) => {
            crate::ui::chrome::t("Cannot read the file").to_owned()
        }
    }
}

/// The letter a key types: its own when Latin, else the US-layout letter of its physical key.
fn letter_of(key: &Key, physical: Option<KeyCode>) -> Option<char> {
    if let Key::Character(s) = key {
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(c.to_ascii_lowercase());
            }
        }
    } else {
        return None;
    }
    Some(match physical? {
        KeyCode::KeyC => 'c',
        KeyCode::KeyJ => 'j',
        KeyCode::KeyK => 'k',
        KeyCode::KeyN => 'n',
        KeyCode::KeyP => 'p',
        KeyCode::KeyR => 'r',
        KeyCode::KeyS => 's',
        KeyCode::KeyU => 'u',
        _ => return None,
    })
}

/// Text as drawn in a row: tabs expanded to `TAB_WIDTH` stops, control characters and bidi
/// controls shown as `�` (a hidden override in a diff is something a reviewer must see), at
/// most `max_cols` cells.
pub fn display_text(text: &str, max_cols: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in text.chars() {
        if col >= max_cols {
            break;
        }
        if c == '\t' {
            let n = TAB_WIDTH - col % TAB_WIDTH;
            out.extend(std::iter::repeat_n(' ', n));
            col += n;
            continue;
        }
        let c = if c.is_control()
            || crate::terminal::is_bidi_control(c)
            || matches!(c, '\u{2028}' | '\u{2029}')
        {
            '\u{fffd}'
        } else {
            c
        };
        out.push(c);
        col += c.width().unwrap_or(0);
    }
    out
}

/// Whether `cwd` lies inside (or equals) `snapshot_root`, resolving symlinks on both paths.
pub fn cwd_in_repo(cwd: Option<&std::path::Path>, snapshot_root: Option<&std::path::Path>) -> bool {
    let (Some(cwd), Some(root)) = (cwd, snapshot_root) else {
        return false;
    };
    let (Ok(cwd), Ok(root)) = (cwd.canonicalize(), root.canonicalize()) else {
        return false;
    };
    cwd.starts_with(&root)
}

/// The last `max` characters of `text`.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let skip = count - max;
    format!("…{}", text.chars().skip(skip).collect::<String>())
}
