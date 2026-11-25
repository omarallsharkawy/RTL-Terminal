//! The diff flattened into rows of one line height each (file headers, notes, hunk headers,
//! diff lines, the comment box), and the scroll and selection math over them.

use super::diff::{Diff, LineKind, OneLine};

/// A diff line: file, hunk and line indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineRef {
    pub file: usize,
    pub hunk: usize,
    pub line: usize,
}

/// What a file row says instead of hunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    OneLine(OneLine),
    /// The file mode changed.
    Mode,
    /// No content change (an empty file, a pure rename).
    Empty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A file header (path and counts); collapsible.
    File(usize),
    Note(usize, Note),
    Hunk(usize, usize),
    Line(LineRef),
    /// `\ No newline at end of file` after a line.
    NoNewline(LineRef),
    /// The comment box under a line: the text being typed, then its key hint.
    CommentInput,
    CommentHint,
    /// More files than the panel lists.
    Omitted,
    /// The diff was cut at its byte or line bound.
    Truncated,
}

impl Row {
    /// Rows the selection can rest on.
    pub fn selectable(&self) -> bool {
        matches!(self, Row::File(_) | Row::Line(_))
    }

    pub fn file(&self) -> Option<usize> {
        match *self {
            Row::File(f) | Row::Note(f, _) | Row::Hunk(f, _) => Some(f),
            Row::Line(l) | Row::NoNewline(l) => Some(l.file),
            _ => None,
        }
    }
}

/// Every row of `diff`: `collapsed(file)` hides a file's body, `comment` puts the comment box
/// under that line.
pub fn build_rows(
    diff: &Diff,
    collapsed: impl Fn(usize) -> bool,
    comment: Option<LineRef>,
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(diff.stored_lines + diff.files.len() * 3 + 2);
    for (f, file) in diff.files.iter().enumerate() {
        rows.push(Row::File(f));
        if collapsed(f) {
            continue;
        }
        if file.mode_change().is_some() {
            rows.push(Row::Note(f, Note::Mode));
        }
        if let Some(why) = file.one_line {
            rows.push(Row::Note(f, Note::OneLine(why)));
            continue;
        }
        if file.hunks.is_empty() && file.mode_change().is_none() {
            rows.push(Row::Note(f, Note::Empty));
        }
        for (h, hunk) in file.hunks.iter().enumerate() {
            rows.push(Row::Hunk(f, h));
            for (l, line) in hunk.lines.iter().enumerate() {
                let r = LineRef {
                    file: f,
                    hunk: h,
                    line: l,
                };
                rows.push(Row::Line(r));
                if line.no_newline {
                    rows.push(Row::NoNewline(r));
                }
                if comment == Some(r) {
                    rows.push(Row::CommentInput);
                    rows.push(Row::CommentHint);
                }
            }
        }
    }
    if diff.omitted_files > 0 {
        rows.push(Row::Omitted);
    }
    if diff.truncated || diff.lines_capped {
        rows.push(Row::Truncated);
    }
    rows
}

/// The line number a comment refers to: the new one, or the old one for a deleted line.
pub fn comment_line(diff: &Diff, r: LineRef) -> Option<(u32, LineKind)> {
    let line = diff
        .files
        .get(r.file)?
        .hunks
        .get(r.hunk)?
        .lines
        .get(r.line)?;
    let number = match line.kind {
        LineKind::Del => line.old?,
        _ => line.new?,
    };
    Some((number, line.kind))
}

/// The top row index kept in `0..=len - page`.
pub fn clamp_top(top: usize, len: usize, page: usize) -> usize {
    top.min(len.saturating_sub(page.max(1)))
}

/// The top row that shows `selected` with the least scrolling from `top`.
pub fn reveal(top: usize, selected: usize, page: usize) -> usize {
    let page = page.max(1);
    if selected < top {
        selected
    } else if selected >= top + page {
        selected + 1 - page
    } else {
        top
    }
}

/// The selectable row `steps` selectable rows away from `from` (negative: up), stopping at the
/// first or last one. With no selection, starts at the first selectable row.
pub fn step(rows: &[Row], from: Option<usize>, steps: isize) -> Option<usize> {
    let Some(mut at) = from.filter(|&i| i < rows.len()) else {
        return rows.iter().position(Row::selectable);
    };
    let mut left = steps.unsigned_abs();
    while left > 0 {
        let next = if steps < 0 {
            rows[..at].iter().rposition(Row::selectable)
        } else {
            rows[at + 1..]
                .iter()
                .position(Row::selectable)
                .map(|i| at + 1 + i)
        };
        match next {
            Some(n) => at = n,
            None => break,
        }
        left -= 1;
    }
    Some(at)
}

/// The selectable row nearest to row `index` (at or below it first, then above).
pub fn nearest(rows: &[Row], index: usize) -> Option<usize> {
    let index = index.min(rows.len().checked_sub(1)?);
    rows[index..]
        .iter()
        .position(Row::selectable)
        .map(|i| index + i)
        .or_else(|| rows[..index].iter().rposition(Row::selectable))
}

/// The next (or previous) file header from `from`.
pub fn next_file(rows: &[Row], from: Option<usize>, forward: bool) -> Option<usize> {
    let is_file = |r: &Row| matches!(r, Row::File(_));
    match from.filter(|&i| i < rows.len()) {
        None => rows.iter().position(is_file),
        Some(at) if forward => rows[at + 1..]
            .iter()
            .position(is_file)
            .map(|i| at + 1 + i)
            .or(Some(at)),
        Some(at) => {
            // From inside a file, "previous" first goes to that file's own header.
            rows[..at].iter().rposition(is_file).or(Some(at))
        }
    }
}

/// The row under `y`, given the first visible row `top` drawn at `body_top`.
pub fn row_at(top: usize, len: usize, body_top: f32, line_height: f32, y: f64) -> Option<usize> {
    if line_height <= 0.0 || y < body_top as f64 {
        return None;
    }
    let index = top + ((y - body_top as f64) / line_height as f64).floor() as usize;
    (index < len).then_some(index)
}
