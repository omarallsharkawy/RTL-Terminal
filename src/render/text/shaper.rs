//! Turning terminal rows and labels into shaped cosmic-text buffers.

use alacritty_terminal::term::cell::Flags as CellFlags;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};

use super::bidi::{is_rtl_char, trails_rtl_neutrals, LRI, LRM, PDI, RLM};
use super::fonts::grid_family;
use super::segment::CachedSegment;
use crate::color::Palette;
use crate::render::row::RowCursor;
use crate::render::scene::boxdraw::try_render_box_or_block;
use crate::render::style::cell_colors;
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
    fn shape_buffer(&mut self, spans: &[(String, Attrs<'static>)], shaping: Shaping) -> Buffer {
        let metrics = Metrics::new(self.font_px, self.line_height);
        let default_attrs = Attrs::new().family(grid_family(self.family));
        let mut buf = Buffer::new_empty(metrics);
        buf.set_rich_text(
            spans.iter().map(|(s, a)| (s.as_str(), a.clone())),
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
        let mut spans: Vec<(String, Attrs<'static>)> = Vec::new();
        let mut cell_bytes = Vec::with_capacity(end - start);
        let mut byte = 0;
        let mut complex = has_rtl;
        // Finished output that starts with Arabic reads right to left as a whole; the line being
        // typed (cursor on this row) keeps an LTR base so editing never flips it.
        let rtl_base = has_rtl && cursor.is_none() && is_rtl_char(cells[start].c);
        if has_rtl {
            let mark = if rtl_base { RLM } else { LRM };
            spans.push((
                mark.to_string(),
                Attrs::new().family(grid_family(self.family)),
            ));
            byte += mark.len();
        }

        // Words (runs of non-blank cells) with Latin letters or digits and no Arabic. Neighbouring
        // ones share one isolate so "world 123" keeps its order inside right-to-left text.
        let mut isolate = vec![(false, false); end - start];
        if has_rtl {
            let mut open: Option<usize> = None;
            let mut last_end = start;
            let mut c = start;
            while c < end {
                if cells[c].is_blank() {
                    c += 1;
                    continue;
                }
                let word_start = c;
                while c < end && !cells[c].is_blank() {
                    c += 1;
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
            let (opens, closes) = isolate[col - start];
            let mut text = String::new();
            if opens {
                text.push_str(LRI);
                byte += LRI.len();
            }
            cell_bytes.push(byte);
            byte += cell.text_len();
            cell.push_text(&mut text);
            if closes {
                text.push_str(PDI);
                byte += PDI.len();
            }
            if text.is_empty() {
                continue;
            }
            complex |= !cell.c.is_ascii() || !cell.zerowidth.is_empty();

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
            let mut attrs = Attrs::new().family(grid_family(self.family)).color(color);
            if cell.flags.contains(CellFlags::BOLD) {
                attrs = attrs.weight(cosmic_text::Weight::BOLD);
            }
            if cell.flags.contains(CellFlags::ITALIC) {
                attrs = attrs.style(cosmic_text::Style::Italic);
            }

            match spans.last_mut() {
                Some((existing, a))
                    if a.color_opt == attrs.color_opt
                        && a.weight == attrs.weight
                        && a.style == attrs.style =>
                {
                    existing.push_str(&text)
                }
                _ => spans.push((text, attrs)),
            }
        }

        // Spaces and punctuation after the last Arabic letter belong to it: keep them on its left
        // (typed spaces, "تم ✓", "مرحبا!"), not at the far right of the LTR paragraph.
        if has_rtl && trails_rtl_neutrals(&cells[start..end]) {
            if let Some((text, _)) = spans.last_mut() {
                text.push_str(RLM);
            }
        }

        let shaping = if complex {
            Shaping::Advanced
        } else {
            Shaping::Basic
        };
        (self.shape_buffer(&spans, shaping), cell_bytes)
    }

    /// True when every glyph of an LTR buffer starts exactly at its cell's grid column.
    fn is_grid_aligned(&self, buffer: &Buffer, cell_bytes: &[usize]) -> bool {
        buffer.layout_runs().all(|run| {
            run.glyphs.iter().all(|glyph| {
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
        let geom: Vec<bool> = cells
            .iter()
            .map(|cell| {
                let mut scratch = Vec::new();
                try_render_box_or_block(cell.c, 0.0, 0.0, 1.0, 1.0, [0.0; 4], &mut scratch)
            })
            .collect();

        let mut segments = Vec::new();
        let mut col = 0;
        while col < cols {
            while col < cols && (geom[col] || cells[col].is_blank()) {
                col += 1;
            }
            if col >= cols {
                break;
            }

            // Grow the segment, keeping short space runs so bidi context spans whole phrases.
            let start = col;
            let mut end = col;
            while end < cols && !geom[end] {
                if cells[end].is_blank() {
                    let mut peek = end;
                    while peek < cols && cells[peek].is_blank() {
                        peek += 1;
                    }
                    if peek - end >= 8 || peek == cols || geom[peek] {
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

            // A fallback glyph (icon, emoji, CJK…) has a different advance: lock every cell to the grid.
            for c in start..end {
                if cells[c].is_blank() || cells[c].is_spacer() {
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
            }
        }
        segments
    }
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
/// shaping, so Arabic in it joins and reorders.
pub fn shape_label(
    font_system: &mut FontSystem,
    family: Option<&'static str>,
    (font_px, line_height): (f32, f32),
    text: &str,
) -> Buffer {
    let mut buffer = Buffer::new_empty(Metrics::new(font_px, line_height));
    buffer.set_text(
        text,
        &Attrs::new().family(grid_family(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
}
