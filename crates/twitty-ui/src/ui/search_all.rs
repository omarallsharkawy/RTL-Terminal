// Adapted from cmux-gtk cmux/src/ui/all_surfaces_search.rs (AGPL-3.0-or-later,
// Copyright (c) 2026 Douglas Soares de Andrade). Changes: rewritten as a window-free palette
// mode with Arabic folding, bounded time-sliced scans, fair shares and line-exact jumps.

//! Search every terminal at once (`search.all`): the palette's search mode.
//!
//! As in cmux-gtk's all-surfaces search, each terminal's text is read and the lines holding
//! the query are listed with the tab they belong to, and choosing one jumps to that pane; the
//! scan runs a moment after the last key press (debounced). Here the lines come from the
//! scrollback as well as the screen, the match uses the palette's folding (case, tashkeel,
//! alef/yeh/teh marbuta forms), and each result remembers its exact line so the jump scrolls to
//! it and highlights it with the scrollback search.
//!
//! Bounds: at most [`MAX_LINES_PER_PANE`] rows of each pane are read (newest first) and
//! [`MAX_HITS`] results kept in all, shared fairly between the panes that match. Queries
//! shorter than [`MIN_QUERY_CHARS`] (after folding) do not search. Reading every row of 20
//! full panes takes longer than a frame, so the app runs a [`SearchJob`] in [`SLICE`]s; a
//! pane is resumed by absolute line, so output arriving in between moves nothing.
//!
//! Window-free: the app hands in terminals, drawing is `render::scene::palette`.

use std::time::{Duration, Instant};

use memchr::memmem::Finder;

use crate::layout::PaneId;
use crate::shell_integration::clean_text;
use crate::terminal::{Terminal, TextChar, TextLinePos};
use crate::ui::notifications::is_rtl;
use crate::ui::palette::fold_into;
use crate::ui::palette::PaletteItem;

/// Most results listed, over all panes.
pub const MAX_HITS: usize = 200;

/// Most rows read from each pane, newest first (screen plus scrollback).
pub const MAX_LINES_PER_PANE: usize = 10_000;

/// Shortest query (in folded characters) that searches.
pub const MIN_QUERY_CHARS: usize = 2;

/// How long typing must pause before the panes are searched.
pub const DEBOUNCE: Duration = Duration::from_millis(120);

/// Widest matching line shown, in columns (longer ones are cut around the match with `…`).
pub const SNIPPET_COLS: usize = 56;

/// Longest tab/pane tag shown beside a line, in characters.
const TAG_CHARS: usize = 28;

/// A query ready to match folded lines.
#[derive(Clone, Debug)]
pub struct Query {
    folded: String,
    finder: Finder<'static>,
}

impl Query {
    /// The trimmed, folded query; `None` when it is shorter than [`MIN_QUERY_CHARS`].
    pub fn new(text: &str) -> Option<Self> {
        let mut folded = String::new();
        for c in text.trim().chars() {
            push_folded(c, &mut folded);
        }
        if folded.chars().count() < MIN_QUERY_CHARS {
            return None;
        }
        let finder = Finder::new(folded.as_bytes()).into_owned();
        Some(Self { folded, finder })
    }

    pub fn folded(&self) -> &str {
        &self.folded
    }
}

/// `c` folded for matching: Arabic presentation forms (sent by vim's `arabicshape`) as their
/// plain letters, then the palette's folding.
fn push_folded(c: char, out: &mut String) {
    if is_presentation_form(c) {
        unicode_normalization::char::decompose_compatible(c, |d| fold_into(d, out));
    } else {
        fold_into(c, out);
    }
}

fn is_presentation_form(c: char) -> bool {
    matches!(c, '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}')
}

/// One matching line of one pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineHit {
    /// The logical line, to find it again.
    pub pos: TextLinePos,
    /// Row (within the logical line) and column of the match's first cell.
    pub row: usize,
    pub column: usize,
    /// The matched cells' text as stored (no marks): what the scrollback search highlights.
    pub needle: String,
    /// The line, cut around the match.
    pub snippet: String,
}

/// The pane a scan belongs to, as the results show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane: PaneId,
    /// Index of its tab.
    pub tab: usize,
    pub tab_title: String,
    /// Title of the pane itself, given when its tab is split.
    pub pane_title: Option<String>,
}

/// One result row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub pane: PaneId,
    pub tab: usize,
    pub line: LineHit,
    /// Tab number and title (and the pane's, in a split tab).
    pub tag: String,
}

/// The first match of `query` on each line of `term`, newest line first: at most `max_hits`
/// of them, from the last `max_rows` rows. All at once; the app goes through [`SearchJob`].
pub fn scan_terminal(
    term: &mut Terminal,
    query: &Query,
    max_hits: usize,
    max_rows: usize,
) -> Vec<LineHit> {
    let mut scan = PaneScan::new(max_rows);
    scan.run(term, query, max_hits, None);
    scan.hits
}

/// Lines read between two looks at the clock.
const LINES_PER_CLOCK_CHECK: usize = 64;

/// How far the scan of one pane got.
#[derive(Debug)]
struct PaneScan {
    hits: Vec<LineHit>,
    /// Where to go on (`None` with `started`: done).
    next: Option<TextLinePos>,
    started: bool,
    rows_left: usize,
    done: bool,
}

impl PaneScan {
    fn new(max_rows: usize) -> Self {
        Self {
            hits: Vec::new(),
            next: None,
            started: false,
            rows_left: max_rows,
            done: max_rows == 0,
        }
    }

    /// Read lines until the pane is done or `deadline` passed (checked every few lines).
    fn run(
        &mut self,
        term: &mut Terminal,
        query: &Query,
        max_hits: usize,
        deadline: Option<Instant>,
    ) {
        if self.done || (self.started && self.next.is_none()) || self.hits.len() >= max_hits {
            self.done = true;
            return;
        }
        let from = self.next;
        self.started = true;
        let needle_len = query.folded.len();
        let hits = &mut self.hits;
        let mut folded = String::new();
        let mut lines = 0;
        let mut out_of_time = false;
        let scan = term.for_each_text_line(from, self.rows_left, |pos, chars| {
            folded.clear();
            for t in chars {
                push_folded(t.c, &mut folded);
            }
            if let Some(at) = query.finder.find(folded.as_bytes()) {
                // Most lines do not match. Map folded bytes back to cells only on a hit.
                folded.clear();
                let (mut base, mut start, mut end) = (0, 0, 0);
                for (i, t) in chars.iter().enumerate() {
                    if !t.mark {
                        base = i;
                    }
                    let before = folded.len();
                    push_folded(t.c, &mut folded);
                    if before <= at && at < folded.len() {
                        start = base;
                    }
                    if folded.len() >= at + needle_len {
                        end = base + 1;
                        break;
                    }
                }
                while chars.get(end).is_some_and(|t| t.mark) {
                    end += 1;
                }
                let needle = chars[start..end]
                    .iter()
                    .filter(|t| !t.mark)
                    .map(|t| t.c)
                    .collect();
                hits.push(LineHit {
                    pos,
                    row: chars[start].row.into(),
                    column: chars[start].col.into(),
                    needle,
                    snippet: snippet(chars, start, end, SNIPPET_COLS),
                });
                if hits.len() >= max_hits {
                    return false;
                }
            }
            lines += 1;
            if lines % LINES_PER_CLOCK_CHECK == 0 && deadline.is_some_and(|d| Instant::now() >= d) {
                out_of_time = true;
                return false;
            }
            true
        });
        self.rows_left = self.rows_left.saturating_sub(scan.rows);
        self.next = scan.next;
        self.done = !out_of_time || scan.next.is_none() || self.rows_left == 0;
    }
}

/// A search of every pane, run a slice at a time so a long scan never holds up the window:
/// panes in the order given (tabs, then panes), each from its newest line.
pub struct SearchJob {
    query: Query,
    panes: Vec<(PaneInfo, PaneScan)>,
    /// The pane being read.
    current: usize,
}

impl SearchJob {
    pub fn new(query: Query, panes: Vec<PaneInfo>) -> Self {
        let panes = panes
            .into_iter()
            .map(|info| (info, PaneScan::new(MAX_LINES_PER_PANE)))
            .collect();
        Self {
            query,
            panes,
            current: 0,
        }
    }

    /// The pane to read next, `None` once every pane is done.
    pub fn current_pane(&self) -> Option<PaneId> {
        self.panes.get(self.current).map(|(info, _)| info.pane)
    }

    /// Read the current pane (`None`: it was closed) until it is done or `deadline` passed.
    pub fn step(&mut self, term: Option<&mut Terminal>, deadline: Option<Instant>) {
        let Some((_, scan)) = self.panes.get_mut(self.current) else {
            return;
        };
        match term {
            Some(term) => scan.run(term, &self.query, MAX_HITS, deadline),
            None => {
                scan.hits.clear();
                scan.done = true;
            }
        }
        if scan.done {
            self.current += 1;
        }
    }

    pub fn is_done(&self) -> bool {
        self.current >= self.panes.len()
    }

    pub fn query(&self) -> &Query {
        &self.query
    }

    /// The results so far (see [`assemble`]).
    pub fn hits(&self) -> Vec<SearchHit> {
        assemble(
            self.panes
                .iter()
                .map(|(info, scan)| (info.clone(), scan.hits.clone()))
                .collect(),
        )
    }

    /// Run the job with `term_of` until it is done or `deadline` passed.
    pub fn run_until(
        &mut self,
        mut term_of: impl FnMut(PaneId, &mut dyn FnMut(Option<&mut Terminal>)),
        deadline: Option<Instant>,
    ) {
        while let Some(id) = self.current_pane() {
            term_of(id, &mut |term| self.step(term, deadline));
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
        }
    }
}

/// How many results each pane keeps when they found `counts` and `total` fit: every pane an
/// equal share, and what a pane does not use goes to the others.
pub fn fair_shares(counts: &[usize], total: usize) -> Vec<usize> {
    let mut shares = vec![0; counts.len()];
    let mut left = total;
    let mut open: Vec<usize> = (0..counts.len()).filter(|&i| counts[i] > 0).collect();
    while left > 0 && !open.is_empty() {
        let each = (left / open.len()).max(1);
        let mut still = Vec::new();
        for &i in &open {
            if left == 0 {
                break;
            }
            let take = each.min(counts[i] - shares[i]).min(left);
            shares[i] += take;
            left -= take;
            if shares[i] < counts[i] {
                still.push(i);
            }
        }
        open = still;
    }
    shares
}

/// The results of every pane, in the order given (tabs, then panes), each pane's newest
/// first, at most [`MAX_HITS`] in all.
pub fn assemble(per_pane: Vec<(PaneInfo, Vec<LineHit>)>) -> Vec<SearchHit> {
    let counts: Vec<usize> = per_pane.iter().map(|(_, hits)| hits.len()).collect();
    let shares = fair_shares(&counts, MAX_HITS);
    let mut out = Vec::new();
    for ((info, hits), share) in per_pane.into_iter().zip(shares) {
        let tag = tag(&info);
        out.extend(hits.into_iter().take(share).map(|line| SearchHit {
            pane: info.pane,
            tab: info.tab,
            line,
            tag: tag.clone(),
        }));
    }
    out
}

/// Search every pane at once (a [`SearchJob`] run to the end).
pub fn search_all<'a>(
    panes: impl IntoIterator<Item = (PaneInfo, &'a mut Terminal)>,
    query: &Query,
) -> Vec<SearchHit> {
    let (infos, mut terms): (Vec<PaneInfo>, Vec<&mut Terminal>) = panes.into_iter().unzip();
    let ids: Vec<PaneId> = infos.iter().map(|i| i.pane).collect();
    let mut job = SearchJob::new(query.clone(), infos);
    job.run_until(
        |id, step| {
            let i = ids.iter().position(|&p| p == id);
            step(i.map(|i| &mut *terms[i]))
        },
        None,
    );
    job.hits()
}

/// `2 build` or `2 build › vim`: the tab's number and title, and the pane's in a split tab.
fn tag(info: &PaneInfo) -> String {
    let tab_title = clean_text(&info.tab_title, 256);
    let pane_title = info
        .pane_title
        .as_deref()
        .map(|t| clean_text(t, 256))
        .filter(|t| !t.is_empty() && *t != tab_title);
    let text = match pane_title {
        Some(pane) => format!("{} {tab_title} › {pane}", info.tab + 1),
        None => format!("{} {tab_title}", info.tab + 1),
    };
    if text.chars().count() > TAG_CHARS {
        let cut: String = text.chars().take(TAG_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        text
    }
}

/// The text of `chars` (a line from [`Terminal::for_each_text_line`]) cut to at most
/// `max_cols` columns around the match `start..end` (indices into `chars`), with `…` where
/// it was cut. Some text before the match is kept, the rest of the room goes after it. Wide
/// characters count two columns, combining marks none; blanks at either end are dropped.
pub fn snippet(chars: &[TextChar], start: usize, end: usize, max_cols: usize) -> String {
    // Cells: (first char, end char, columns).
    let mut cells: Vec<(usize, usize, usize)> = Vec::new();
    for (i, t) in chars.iter().enumerate() {
        match cells.last_mut() {
            Some(cell) if t.mark => cell.1 = i + 1,
            _ => cells.push((i, i + 1, if t.wide { 2 } else { 1 })),
        }
    }
    let blank = |&(from, to, _): &(usize, usize, usize)| {
        chars[from..to].iter().all(|t| t.c.is_whitespace())
    };
    let first = cells.iter().position(|c| !blank(c)).unwrap_or(cells.len());
    let last = cells
        .iter()
        .rposition(|c| !blank(c))
        .map_or(first, |i| i + 1);
    let cells = &cells[first..last.max(first)];
    let n = cells.len();
    let mut prefix = vec![0; n + 1];
    for (i, cell) in cells.iter().enumerate() {
        prefix[i + 1] = prefix[i] + cell.2;
    }
    let width = |a: usize, b: usize| prefix[b] - prefix[a];
    let text = |a: usize, b: usize| -> String {
        let mut out = String::new();
        for &(from, to, _) in &cells[a..b] {
            for t in &chars[from..to] {
                if matches!(t.c, '\u{061C}' | '\u{200E}' | '\u{200F}') {
                    continue;
                }
                if is_presentation_form(t.c) {
                    unicode_normalization::char::decompose_compatible(t.c, |d| out.push(d));
                } else {
                    out.push(t.c);
                }
            }
        }
        out
    };
    if width(0, n) <= max_cols {
        return text(0, n);
    }
    let match_start = cells.iter().position(|c| start < c.1).unwrap_or(n);
    let match_end = cells
        .iter()
        .position(|c| end <= c.1)
        .map_or(n, |i| i + 1)
        .max(match_start);
    // Room left for text once both sides may carry a `…`.
    let room = max_cols.saturating_sub(2).max(1);
    let (mut a, mut b) = (match_start.min(n), match_end.min(n));
    // A match wider than the room keeps its start.
    while b > a + 1 && width(a, b) > room {
        b -= 1;
    }
    let before = room.saturating_sub(width(a, b)) / 3;
    while a > 0 && width(a - 1, match_start.min(n)) <= before {
        a -= 1;
    }
    while b < n && width(a, b + 1) <= room {
        b += 1;
    }
    while a > 0 && width(a - 1, b) <= room {
        a -= 1;
    }
    // Cut on one side only: its `…` was the only one needed.
    let room = room + 1;
    if a == 0 {
        while b < n && width(a, b + 1) <= room {
            b += 1;
        }
    }
    if b == n {
        while a > 0 && width(a - 1, b) <= room {
            a -= 1;
        }
    }
    let body = text(a, b);
    let mut out = String::new();
    if a > 0 {
        out.push('…');
        out.push_str(body.trim_start());
    } else {
        out.push_str(&body);
    }
    if b < n {
        out.truncate(out.trim_end().len());
        out.push('…');
    }
    out
}

/// A result as a palette row: the line on the side its direction reads from (Arabic on the
/// right, like the commands' Arabic titles) and the tab on the other.
pub fn palette_item(hit: &SearchHit) -> PaletteItem {
    let (title_en, title_ar) = if is_rtl(&hit.line.snippet) {
        (hit.tag.clone(), hit.line.snippet.clone())
    } else {
        (hit.line.snippet.clone(), hit.tag.clone())
    };
    PaletteItem {
        title_en,
        title_ar,
        shortcut: None,
        dimmed: false,
    }
}

fn note(title_en: &str, title_ar: &str) -> PaletteItem {
    PaletteItem {
        title_en: title_en.into(),
        title_ar: title_ar.into(),
        shortcut: None,
        dimmed: true,
    }
}

/// Shown in the query line while it is empty.
pub fn placeholder() -> &'static str {
    crate::ui::chrome::t("Search all terminals…")
}

/// How long one slice of a scan may take before the window gets to handle input and draw.
pub const SLICE: Duration = Duration::from_millis(8);

/// The palette's search mode: the results so far, the scan in progress, and the one waiting
/// for typing to pause.
#[derive(Default)]
pub struct SearchAll {
    hits: Vec<SearchHit>,
    /// The folded query `hits` answer, once a scan finished.
    searched: Option<String>,
    /// When the next scan is due to start (the query changed).
    due: Option<Instant>,
    job: Option<SearchJob>,
}

impl SearchAll {
    /// The query is now `query` (typed at `now`): schedule a scan, or drop the results when it
    /// is too short to search. A scan in progress for another query is dropped.
    pub fn query_changed(&mut self, query: &str, now: Instant) {
        let q = Query::new(query);
        let folded = q.as_ref().map(Query::folded);
        if self.job.as_ref().map(|j| j.query().folded()) != folded {
            self.job = None;
        }
        match folded {
            None => {
                self.hits.clear();
                self.searched = None;
                self.due = None;
            }
            Some(f) if self.searched.as_deref() == Some(f) || self.job.is_some() => self.due = None,
            Some(_) => self.due = Some(now + DEBOUNCE),
        }
    }

    /// When the waiting scan should start.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// A scan is waiting or in progress.
    pub fn is_pending(&self) -> bool {
        self.due.is_some() || self.job.is_some()
    }

    /// Start scanning `panes` for `query` (the app calls this when the scan is due).
    pub fn start(&mut self, query: Query, panes: Vec<PaneInfo>) {
        self.due = None;
        self.job = Some(SearchJob::new(query, panes));
    }

    /// The scan in progress.
    pub fn job_mut(&mut self) -> Option<&mut SearchJob> {
        self.job.as_mut()
    }

    /// Take the scan's results so far, and end it when it is done.
    pub fn update(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        self.hits = job.hits();
        if job.is_done() {
            self.searched = Some(job.query().folded.clone());
            self.job = None;
        }
    }

    /// The scan for `query` found `hits` (a scan run all at once).
    pub fn finish(&mut self, query: &Query, hits: Vec<SearchHit>) {
        self.hits = hits;
        self.searched = Some(query.folded.clone());
        self.due = None;
        self.job = None;
    }

    pub fn hits(&self) -> &[SearchHit] {
        &self.hits
    }

    /// Forget the results of panes that were closed.
    pub fn retain_open(&mut self, is_open: impl Fn(PaneId) -> bool) {
        self.hits.retain(|h| is_open(h.pane));
    }

    /// The rows to list for `query`: the results, or a hint when there are none.
    pub fn rows(&self, query: &str) -> Vec<PaletteItem> {
        if Query::new(query).is_none() {
            return vec![note(
                "Type at least 2 characters to search every terminal",
                "اكتب حرفين على الأقل",
            )];
        }
        if !self.hits.is_empty() {
            return self.hits.iter().map(palette_item).collect();
        }
        if self.is_pending() || self.searched.is_none() {
            vec![note("Searching…", "بيدوّر…")]
        } else {
            vec![note("No matches", "مفيش نتايج")]
        }
    }
}

/// The result to jump to: none once its pane is closed.
pub fn jump_target(hit: &SearchHit, is_open: impl Fn(PaneId) -> bool) -> Option<&SearchHit> {
    is_open(hit.pane).then_some(hit)
}
