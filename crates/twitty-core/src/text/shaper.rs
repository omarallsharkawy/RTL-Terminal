//! Turning terminal rows and labels into shaped cosmic-text buffers.

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::Color as AnsiColor;
use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

use super::bidi::{
    arabic_phrase, arabic_phrase_end, is_rtl_char, trails_rtl_neutrals, LRI, LRM, PDI, RLI, RLM,
};
use super::fonts::{arabic_family, grid_family, is_arabic_script};
use super::segment::CachedSegment;
use crate::boxdraw::is_box_or_block;
use crate::color::Palette;
use crate::row::RowCursor;
use crate::style::cell_colors;
use crate::terminal::{CellData, LineData};

/// Turns terminal rows into shaped text segments. Kept apart from the GPU state so the
/// bidi/cursor logic can be tested without a window.
pub struct RowShaper<'a> {
    pub font_system: &'a mut FontSystem,
    pub palette: &'a Palette,
    /// Font size in physical pixels.
    pub font_px: f32,
    pub char_width: f32,
    pub line_height: f32,
    /// Configured font family; `None` is the generic monospace.
    pub family: Option<&'static str>,
}

impl RowShaper<'_> {
    fn shape_buffer(
        &mut self,
        text: &str,
        spans: &[(usize, Attrs<'static>)],
        shaping: Shaping,
    ) -> Buffer {
        let metrics = Metrics::new(self.font_px, self.line_height);
        let default_attrs = Attrs::new().family(grid_family(self.family));
        let mut buf = Buffer::new_empty(metrics);
        buf.set_rich_text(
            spans.iter().enumerate().map(|(i, (start, attrs))| {
                let end = spans.get(i + 1).map_or(text.len(), |(end, _)| *end);
                (&text[*start..end], attrs.clone())
            }),
            &default_attrs,
            shaping,
            None,
        );
        buf.shape_until_scroll(self.font_system, false);
        buf
    }

    /// Shape cells `[start, end)` into one buffer. Returns the buffer plus each cell's byte offset.
    fn shape_cells(
        &mut self,
        cells: &[CellData],
        start: usize,
        end: usize,
        has_rtl: bool,
        cursor: Option<RowCursor>,
    ) -> (Buffer, Vec<usize>) {
        let mut text = String::with_capacity(end - start);
        let mut spans: Vec<(usize, Attrs<'static>)> = Vec::new();
        let mut cell_bytes = Vec::with_capacity(end - start);
        let mut complex = has_rtl;
        let arabic = if has_rtl {
            arabic_family(self.font_system, self.family).map(Family::Name)
        } else {
            None
        };
        // Words (runs of non-blank cells) holding an Arabic letter are shaped in the Arabic font
        // as a whole: cosmic-text misorders the pieces of a right-to-left word whose attributes
        // change inside it, so "مرحبا!" must not switch fonts before the "!".
        let mut arabic_word = Vec::new();
        if arabic.is_some() {
            arabic_word.resize(end - start, false);
            let mut c = start;
            while c < end {
                let word_start = c;
                while c < end && !cells[c].is_word_break() {
                    c += 1;
                }
                if cells[word_start..c].iter().any(|w| is_arabic_script(w.c)) {
                    arabic_word[word_start - start..c - start].fill(true);
                }
                c += 1;
            }
        }
        // Where the Arabic phrase starts and whether it reads right to left (see `arabic_phrase`).
        let phrase = if has_rtl {
            arabic_phrase(&cells[start..end], cursor.is_some()).map(|p| start + p)
        } else {
            None
        };
        let rtl_base = phrase == Some(start);
        if has_rtl {
            let mark = if rtl_base { RLM } else { LRM };
            spans.push((0, Attrs::new().family(grid_family(self.family))));
            text.push_str(mark);
        }
        // A phrase after a left-to-right lead (a prompt, `echo `) is isolated right to left,
        // up to where its Arabic ends (`phrase_end`, exclusive).
        let phrase_isolate = phrase.filter(|&p| p > start);
        let phrase_end = phrase_isolate.map_or(end, |p| {
            start + arabic_phrase_end(&cells[start..end], p - start, cursor.is_some())
        });

        // Words (runs of non-blank cells) with Latin letters or digits and no Arabic. Neighbouring
        // ones share one isolate so "world 123" keeps its order inside right-to-left text.
        let mut isolate = Vec::new();
        if has_rtl {
            isolate.resize(end - start, (false, false));
            let mut open: Option<usize> = None;
            let mut last_end = start;
            let mut c = start;
            while c < end {
                if cells[c].is_word_break() {
                    c += 1;
                    continue;
                }
                let word_start = c;
                while c < end && !cells[c].is_word_break() {
                    c += 1;
                }
                // Latin runs never straddle the end of the Arabic phrase (isolates must nest).
                if word_start >= phrase_end && open.is_some_and(|o| o < phrase_end) {
                    if let Some(o) = open.take() {
                        isolate[o - start].0 = true;
                        isolate[last_end - 1 - start].1 = true;
                    }
                }
                let word = &cells[word_start..c];
                if word.iter().any(|w| w.c.is_ascii_alphanumeric())
                    && !word.iter().any(|w| is_rtl_char(w.c))
                {
                    open.get_or_insert(word_start);
                    last_end = c;
                } else if let Some(o) = open.take() {
                    isolate[o - start].0 = true;
                    isolate[last_end - 1 - start].1 = true;
                }
            }
            if let Some(o) = open {
                isolate[o - start].0 = true;
                isolate[last_end - 1 - start].1 = true;
            }
        }

        for (col, cell) in cells.iter().enumerate().take(end).skip(start) {
            let (opens, closes) = isolate.get(col - start).copied().unwrap_or_default();
            let span_start = text.len();
            if phrase_isolate == Some(col) {
                text.push_str(RLI);
            }
            if opens {
                text.push_str(LRI);
            }
            cell_bytes.push(text.len());
            cell.push_text(&mut text);
            if closes {
                text.push_str(PDI);
            }
            if phrase_isolate.is_some() && col + 1 == phrase_end && phrase_end < end {
                text.push_str(PDI);
            }
            if text.len() == span_start {
                continue;
            }
            complex |= !cell.c.is_ascii() || cell.has_shaped_marks();

            let (mut fg, bg) = cell_colors(self.palette, cell);
            if cursor.is_some_and(|c| c.block && c.col == col) {
                // Text under a block cursor takes the cell's background so it stays legible.
                fg = bg;
            }
            // Concealed text (`\e[8m`) is drawn fully transparent: painting it in the background
            // color would still show it over a see-through window.
            let color = if cell.flags.contains(CellFlags::HIDDEN) && fg == bg {
                cosmic_text::Color::rgba(0, 0, 0, 0)
            } else {
                fg.to_glyphon()
            };
            // Arabic letters go straight to the Arabic font instead of through font fallback.
            let family = match arabic {
                Some(f) if arabic_word[col - start] => f,
                _ => grid_family(self.family),
            };
            let mut attrs = Attrs::new().family(family).color(color);
            if cell.flags.contains(CellFlags::BOLD) {
                attrs = attrs.weight(cosmic_text::Weight::BOLD);
            }
            if cell.flags.contains(CellFlags::ITALIC) {
                attrs = attrs.style(cosmic_text::Style::Italic);
            }

            if !spans.last().is_some_and(|(_, a)| {
                a.color_opt == attrs.color_opt
                    && a.family == attrs.family
                    && a.weight == attrs.weight
                    && a.style == attrs.style
            }) {
                spans.push((span_start, attrs));
            }
        }

        // Spaces and punctuation after the last Arabic letter belong to it: keep them on its left
        // (typed spaces, "تم ✓", "مرحبا!"), with the preceding span's attributes.
        if has_rtl && trails_rtl_neutrals(&cells[start..end]) {
            text.push_str(RLM);
        }
        if phrase_isolate.is_some() && phrase_end == end {
            text.push_str(PDI);
        }

        let shaping = if complex {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };
        (self.shape_buffer(&text, &spans, shaping), cell_bytes)
    }

    /// True when every sized glyph of an LTR buffer starts at its cell's grid column.
    /// Zero-width marks (combining accents, variation selectors) are positioned relative to their
    /// base glyph by HarfBuzz and have no column advance of their own.
    fn is_grid_aligned(&self, buffer: &Buffer, cell_bytes: &[usize]) -> bool {
        buffer.layout_runs().all(|run| {
            run.glyphs
                .iter()
                .filter(|glyph| glyph.w > 0.0)
                .all(|glyph| {
                    let cell = cell_bytes
                        .partition_point(|&b| b <= glyph.start)
                        .saturating_sub(1);
                    (glyph.x - cell as f32 * self.char_width).abs() < 0.5
                })
        })
    }

    pub fn build_row(&mut self, line: &LineData, cursor: Option<RowCursor>) -> Vec<CachedSegment> {
        let cells = &line.cells;
        let cols = cells.len();
        let mut segments = Vec::new();
        let mut col = 0;
        while col < cols {
            while col < cols && (is_box_or_block(cells[col].c) || cells[col].is_blank()) {
                col += 1;
            }
            if col >= cols {
                break;
            }

            // Grow the segment, keeping short space runs so bidi context spans whole phrases.
            let start = col;
            let mut end = col;
            while end < cols && !is_box_or_block(cells[end].c) {
                if end > start && is_region_boundary(cells, end) {
                    break;
                }
                if cells[end].is_blank() {
                    let mut peek = end;
                    while peek < cols && cells[peek].is_blank() {
                        peek += 1;
                    }
                    if peek - end >= 8 || peek == cols || is_box_or_block(cells[peek].c) {
                        break;
                    }
                }
                end += 1;
            }
            while end > start && cells[end - 1].is_blank() {
                end -= 1;
            }
            // Spaces the user just typed after Arabic text stay part of it (see RLM above).
            if let Some(c) = cursor {
                if c.col > end
                    && c.col <= cols
                    && is_rtl_char(cells[end - 1].c)
                    && cells[end..c.col].iter().all(|cell| cell.is_blank())
                {
                    end = c.col;
                }
            }
            // Many TUIs hide the cursor and draw their own: an inverse-video blank right after
            // the text (pi, omp). After Arabic it belongs to the text too, so it is laid out at
            // the text's end, on its left, instead of staying at the far right of the grid.
            if let Some(block) = program_cursor_after(cells, end) {
                if is_rtl_char(cells[end - 1].c) || trails_rtl_neutrals(&cells[start..end]) {
                    end = block + 1;
                }
            }
            col = end;

            let has_rtl = cells[start..end].iter().any(|c| is_rtl_char(c.c));
            let (buffer, cell_bytes) = self.shape_cells(cells, start, end, has_rtl, cursor);

            if has_rtl || self.is_grid_aligned(&buffer, &cell_bytes) {
                segments.push(CachedSegment {
                    seg_start: start,
                    seg_end: end,
                    seg_x: start as f32 * self.char_width,
                    buffer,
                    has_rtl,
                    cell_bytes,
                });
                continue;
            }

            // A fallback glyph (icon, emoji, CJK…) has a different advance. Coalesce contiguous
            // plain ASCII runs into single segments so one emoji does not force cell-by-cell
            // HarfBuzz shaping across the entire row, locking only non-ASCII/misaligned cells.
            let mut c = start;
            while c < end {
                if cells[c].is_blank() || cells[c].is_spacer() {
                    c += 1;
                    continue;
                }
                if cells[c].c.is_ascii() && !cells[c].has_shaped_marks() {
                    let run_start = c;
                    while c < end
                        && !cells[c].is_blank()
                        && !cells[c].is_spacer()
                        && cells[c].c.is_ascii()
                        && !cells[c].has_shaped_marks()
                    {
                        c += 1;
                    }
                    let (buffer, cell_bytes) = self.shape_cells(cells, run_start, c, false, cursor);
                    if self.is_grid_aligned(&buffer, &cell_bytes) {
                        segments.push(CachedSegment {
                            seg_start: run_start,
                            seg_end: c,
                            seg_x: run_start as f32 * self.char_width,
                            buffer,
                            has_rtl: false,
                            cell_bytes,
                        });
                        continue;
                    }
                    for col_idx in run_start..c {
                        let (buffer, cell_bytes) =
                            self.shape_cells(cells, col_idx, col_idx + 1, false, cursor);
                        segments.push(CachedSegment {
                            seg_start: col_idx,
                            seg_end: col_idx + 1,
                            seg_x: col_idx as f32 * self.char_width,
                            buffer,
                            has_rtl: false,
                            cell_bytes,
                        });
                    }
                    continue;
                }
                let (buffer, cell_bytes) = self.shape_cells(cells, c, c + 1, false, cursor);
                segments.push(CachedSegment {
                    seg_start: c,
                    seg_end: c + 1,
                    seg_x: c as f32 * self.char_width,
                    buffer,
                    has_rtl: false,
                    cell_bytes,
                });
                c += 1;
            }
        }
        segments
    }
}

/// A cursor a program draws itself after text ending at `end`: a single inverse-video blank,
/// separated from the text by at most two spaces. Returns its column.
fn program_cursor_after(cells: &[CellData], end: usize) -> Option<usize> {
    let inverse_blank =
        |c: &CellData| c.is_blank() && c.flags.contains(CellFlags::INVERSE) && !c.selected;
    let block = (end..cells.len().min(end + 3))
        .find(|&c| !cells[c].is_blank() || inverse_blank(&cells[c]))?;
    let single = cells
        .get(block + 1)
        .is_none_or(|next| !next.flags.contains(CellFlags::INVERSE));
    (inverse_blank(&cells[block]) && single).then_some(block)
}

/// Background of a cell as drawn, ignoring selection and search (which must never reorder text).
fn region_bg(cell: &CellData) -> AnsiColor {
    if cell.flags.contains(CellFlags::INVERSE) {
        cell.fg
    } else {
        cell.bg
    }
}

/// True when column `col` starts a different UI region than `col - 1`: the background changes
/// and the new or the old side is padded with at least two blank cells. TUI popups, panels and
/// status bars look like that (opencode's command palette over an Arabic prompt), while a word
/// highlighted inside a sentence does not, so the sentence keeps its bidi context.
fn is_region_boundary(cells: &[CellData], col: usize) -> bool {
    let (before, after) = (region_bg(&cells[col - 1]), region_bg(&cells[col]));
    if before == after {
        return false;
    }
    let padded = |range: &mut dyn Iterator<Item = &CellData>, bg: AnsiColor| {
        range
            .take(2)
            .filter(|c| c.is_blank() && region_bg(c) == bg)
            .count()
            == 2
    };
    padded(&mut cells[col..].iter(), after) || padded(&mut cells[..col].iter().rev(), before)
}

/// Measure the advance of one monospace cell at the given pixel size.
pub fn measure_char_width(
    font_system: &mut FontSystem,
    family: Option<&'static str>,
    font_px: f32,
    line_height: f32,
) -> f32 {
    let mut buffer = Buffer::new_empty(Metrics::new(font_px, line_height));
    buffer.set_text(
        "MMMMMMMMMM",
        &Attrs::new().family(grid_family(family)),
        Shaping::Basic,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .filter_map(|run| run.glyphs.first().map(|g| g.w))
        .find(|w| *w > 0.0)
        .unwrap_or((font_px * 0.6).round())
}

/// Shape a one-line label (status line, tab title, preedit) in the grid font with full
/// shaping, so Arabic in it joins and reorders. Bidi controls, control characters and
/// non-shaping invisible marks are dropped (tabs become spaces), as in terminal cells.
#[doc(hidden)]
pub fn shape_label(
    font_system: &mut FontSystem,
    family: Option<&'static str>,
    (font_px, line_height): (f32, f32),
    text: &str,
) -> Buffer {
    let text: String = text
        .chars()
        .filter_map(|c| {
            if c == '\t' {
                Some(' ')
            } else if crate::terminal::is_ignored_mark(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    let mut buffer = Buffer::new_empty(Metrics::new(font_px, line_height));
    buffer.set_text(
        &text,
        &Attrs::new().family(grid_family(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
}
