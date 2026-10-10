use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape};
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use unicode_bidi::{bidi_class, BidiClass};

use crate::color::{Palette, Rgba};
use crate::quad::try_render_box_or_block;
use crate::terminal::{CellData, CursorState, LineData};

/// Left-to-right mark prepended to RTL segments so prompts never flip direction.
pub const LRM: &str = "\u{200E}";
/// Right-to-left mark closing an Arabic run that ends in typed spaces.
const RLM: &str = "\u{200F}";
/// Left-to-right isolate / pop directional isolate around Latin words in Arabic text.
const LRI: char = '\u{2066}';
const PDI: char = '\u{2069}';

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    pub seg_x: f32,
    pub buffer: Buffer,
    pub has_rtl: bool,
    pub cell_visual_x: Vec<(f32, f32)>,
    /// Exact glyph extent of each cell (absolute left, right) and whether it was laid out RTL.
    pub cell_edges: Vec<(f32, f32, bool)>,
}

#[derive(Clone)]
pub struct CachedRow {
    pub hash: u64,
    pub segments: Vec<CachedSegment>,
    pub geom_cells: Vec<(usize, char, AnsiColor)>,
    pub bg_cells: Vec<(usize, AnsiColor)>,
    pub underline_cells: Vec<(usize, Rgba, bool)>,
    pub strikeout_cells: Vec<(usize, Rgba)>,
}

impl CachedRow {
    pub fn visual_x_for_cell(&self, col: usize, char_width: f32, padding_left: f32) -> (f32, f32) {
        for seg in &self.segments {
            if col >= seg.seg_start && col < seg.seg_end {
                let idx = col - seg.seg_start;
                if let Some(&(x, w)) = seg.cell_visual_x.get(idx) {
                    return (x, w);
                }
            }
        }
        (padding_left + col as f32 * char_width, char_width)
    }

    /// Where the caret is drawn for logical column `col`: `(x, extends_left)`.
    ///
    /// Inside text the caret sits before the cell's glyph (its right edge for Arabic, left edge
    /// for Latin). After the last typed letter it sits on that letter's far side: left of
    /// Arabic, right of Latin. `extends_left` tells box cursors to grow leftwards so they
    /// don't cover the Arabic letter just typed.
    pub fn cursor_anchor(&self, col: usize, char_width: f32, padding_left: f32) -> (f32, bool) {
        for (i, seg) in self.segments.iter().enumerate() {
            if !seg.has_rtl {
                continue;
            }
            if col >= seg.seg_start && col < seg.seg_end {
                if let Some(&(left, right, rtl)) = seg.cell_edges.get(col - seg.seg_start) {
                    return (if rtl { right } else { left }, false);
                }
            }
            let next_start = self.segments.get(i + 1).map_or(usize::MAX, |s| s.seg_start);
            if col >= seg.seg_end && col < next_start {
                if let Some(&(left, right, rtl)) = seg.cell_edges.last() {
                    let gap = (col - seg.seg_end) as f32 * char_width;
                    return if rtl {
                        (left - gap, true)
                    } else {
                        (right + gap, false)
                    };
                }
            }
        }
        (padding_left + col as f32 * char_width, false)
    }

    pub fn cell_from_visual_x(
        &self,
        x: f32,
        char_width: f32,
        padding_left: f32,
        max_cols: usize,
    ) -> usize {
        for seg in &self.segments {
            for (idx, &(cx, cw)) in seg.cell_visual_x.iter().enumerate() {
                if x >= cx && x <= cx + cw {
                    return seg.seg_start + idx;
                }
            }
        }
        ((x - padding_left) / char_width)
            .max(0.0)
            .min(max_cols.saturating_sub(1) as f32) as usize
    }
}

pub fn hash_color<H: std::hash::Hasher>(color: &AnsiColor, hasher: &mut H) {
    use std::hash::Hash;
    match color {
        AnsiColor::Named(n) => {
            0u8.hash(hasher);
            (*n as u8).hash(hasher);
        }
        AnsiColor::Spec(rgb) => {
            1u8.hash(hasher);
            rgb.r.hash(hasher);
            rgb.g.hash(hasher);
            rgb.b.hash(hasher);
        }
        AnsiColor::Indexed(idx) => {
            2u8.hash(hasher);
            idx.hash(hasher);
        }
    }
}

#[allow(dead_code)]
pub fn hash_cells(cells: &[CellData]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        cell.zerowidth.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.is_selected.hash(&mut hasher);
    }
    hasher.finish()
}

pub fn hash_row_with_cursor(cells: &[CellData], r: usize, cursor: &CursorState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        cell.zerowidth.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.is_selected.hash(&mut hasher);
    }
    if cursor.is_visible && cursor.row == r {
        true.hash(&mut hasher);
        cursor.col.hash(&mut hasher);
        (cursor.shape as u8).hash(&mut hasher);
    } else {
        false.hash(&mut hasher);
    }
    hasher.finish()
}
/// Text a cell contributes to a shaped run: nothing for wide-char spacers, a blank for
/// hidden cells, otherwise the base character followed by its combining marks (harakat).
pub fn push_cell_text(cell: &CellData, out: &mut String) {
    if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
        return;
    }
    if cell.flags.contains(CellFlags::HIDDEN) {
        out.push(' ');
        return;
    }
    out.push(cell.c);
    out.extend(cell.zerowidth.iter());
}

/// Byte length of [`push_cell_text`] output, used to map glyphs back to grid cells.
pub fn cell_text_len(cell: &CellData) -> usize {
    if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
        0
    } else if cell.flags.contains(CellFlags::HIDDEN) {
        1
    } else {
        cell.c.len_utf8() + cell.zerowidth.iter().map(|z| z.len_utf8()).sum::<usize>()
    }
}

fn is_rtl_char(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// Cells `[start, end)` that form a Latin "word" inside Arabic text (a run of non-space
/// cells with a Latin letter or digit and no Arabic), e.g. `7abaf40`, `Ctrl+D/E/K`, `(:567-580)`.
fn ltr_word_bounds(cells: &[CellData]) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        if cells[i].c == ' ' {
            i += 1;
            continue;
        }
        let w_start = i;
        while i < cells.len() && cells[i].c != ' ' {
            i += 1;
        }
        let word = &cells[w_start..i];
        if word
            .iter()
            .any(|c| bidi_class(c.c) == BidiClass::L || c.c.is_ascii_digit())
            && !word.iter().any(|c| is_rtl_char(c.c))
        {
            words.push((w_start, i));
        }
    }
    words
}

/// Shape cells `[start, end)` of a line into one buffer.
///
/// Returns the buffer, whether it contains RTL text, whether it needed font fallback, and
/// each cell's byte range in the shaped text (used to map glyphs back to grid cells).
#[allow(clippy::too_many_arguments)]
fn shape_segment(
    line: &LineData,
    start: usize,
    end: usize,
    block_cursor_col: Option<usize>,
    metrics: Metrics,
    default_attrs: &Attrs,
    palette: &Palette,
    font_system: &mut FontSystem,
) -> (Buffer, bool, bool, Vec<(usize, usize)>) {
    let cells = &line.cells[start..end];
    let has_rtl = cells.iter().any(|c| is_rtl_char(c.c));
    let mut needs_fallback = has_rtl;
    let mut spans_data: Vec<(String, Attrs)> = Vec::new();
    let mut byte = 0;
    if has_rtl {
        spans_data.push((LRM.to_string(), default_attrs.clone()));
        byte += LRM.len();
    }

    // Inside Arabic text, isolate Latin words (LRI…PDI) so hashes like `7abaf40` keep their
    // digits and letters together instead of being torn apart by the bidi algorithm.
    let ltr_words = if has_rtl {
        ltr_word_bounds(cells)
    } else {
        Vec::new()
    };
    let mut cell_ranges = Vec::with_capacity(cells.len());

    for (i, cell) in cells.iter().enumerate() {
        let col = start + i;
        let mut text = String::new();
        if ltr_words.iter().any(|&(s, _)| s == i) {
            text.push(LRI);
        }
        let cell_start = byte + text.len();
        push_cell_text(cell, &mut text);
        let cell_end = byte + text.len();
        if ltr_words.iter().any(|&(_, e)| e == i + 1) {
            text.push(PDI);
        }
        cell_ranges.push((cell_start, cell_end));
        byte += text.len();
        if text.is_empty() {
            continue;
        }
        if !cell.c.is_ascii() || !cell.zerowidth.is_empty() {
            needs_fallback = true;
        }

        let (eff_fg, eff_bg) = if cell.flags.contains(CellFlags::INVERSE) {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };
        let mut fg = if cell.is_selected {
            Rgba::from_rgb8(245, 245, 255)
        } else {
            palette.resolve(eff_fg, false)
        };
        if cell.flags.contains(CellFlags::DIM) {
            fg.r *= 0.7;
            fg.g *= 0.7;
            fg.b *= 0.7;
        }
        if block_cursor_col == Some(col) {
            // The glyph under a block cursor takes the cell background so it stays readable.
            fg = palette.resolve(eff_bg, true);
        }
        let mut attrs = default_attrs.clone().color(fg.to_glyphon());
        if cell.flags.contains(CellFlags::BOLD) {
            attrs = attrs.weight(cosmic_text::Weight::BOLD);
        }
        if cell.flags.contains(CellFlags::ITALIC) {
            attrs = attrs.style(cosmic_text::Style::Italic);
        }

        match spans_data.last_mut() {
            Some((span, prev))
                if prev.color_opt == attrs.color_opt
                    && prev.weight == attrs.weight
                    && prev.style == attrs.style =>
            {
                span.push_str(&text)
            }
            _ => spans_data.push((text, attrs)),
        }
    }

    if has_rtl {
        // Spaces typed after Arabic would resolve to the (LTR) paragraph direction and jump to
        // the right of the word; a closing RLM keeps them inside the Arabic run, on its left.
        if let Some(last) = cells.iter().rposition(|c| c.c != ' ') {
            if last + 1 < cells.len() && is_rtl_char(cells[last].c) {
                spans_data.push((RLM.to_string(), default_attrs.clone()));
            }
        }
    }

    let mut buf = Buffer::new_empty(metrics);
    let shaping_mode = if needs_fallback {
        Shaping::Advanced
    } else {
        Shaping::Basic
    };
    buf.set_rich_text(
        spans_data.iter().map(|(s, a)| (s.as_str(), a.clone())),
        default_attrs,
        shaping_mode,
        None,
    );
    buf.shape_until_scroll(font_system, false);
    (buf, has_rtl, needs_fallback, cell_ranges)
}

/// Byte offset where each cell of `[start, end)` begins in the shaped text (plus the end).
fn cell_byte_offsets(line: &LineData, start: usize, end: usize, prefix: usize) -> Vec<usize> {
    let mut offsets = Vec::with_capacity(end - start + 1);
    let mut cur = prefix;
    offsets.push(cur);
    for cell in &line.cells[start..end] {
        cur += cell_text_len(cell);
        offsets.push(cur);
    }
    offsets
}

/// True when every glyph of an LTR buffer starts at its cell's grid column.
fn is_grid_aligned(buf: &Buffer, offsets: &[usize], char_width: f32) -> bool {
    buf.layout_runs().all(|run| {
        run.glyphs.iter().all(|g| {
            let cell = offsets.partition_point(|&b| b <= g.start).saturating_sub(1);
            (g.x - cell as f32 * char_width).abs() < 0.5
        })
    })
}

#[allow(clippy::too_many_arguments)]
pub fn shape_row(
    line: &LineData,
    r: usize,
    cursor: &CursorState,
    char_width: f32,
    line_height: f32,
    padding_left: f32,
    metrics: Metrics,
    default_attrs: &Attrs,
    palette: &Palette,
    font_system: &mut FontSystem,
    default_bg: Rgba,
) -> CachedRow {
    let cols = line.cells.len();
    let current_hash = hash_row_with_cursor(&line.cells, r, cursor);
    let block_cursor_col =
        (cursor.is_visible && cursor.row == r && cursor.shape == CursorShape::Block)
            .then_some(cursor.col);

    let mut geom_cells = Vec::new();
    let mut bg_cells = Vec::new();
    let mut underline_cells = Vec::new();
    let mut strikeout_cells = Vec::new();
    let mut geom_rendered = vec![false; cols];

    for (c, cell) in line.cells.iter().enumerate() {
        let (eff_fg, eff_bg) = if cell.flags.contains(CellFlags::INVERSE) {
            (cell.bg, cell.fg)
        } else {
            (cell.fg, cell.bg)
        };

        if cell.is_selected {
            bg_cells.push((
                c,
                AnsiColor::Spec(alacritty_terminal::vte::ansi::Rgb {
                    r: 54,
                    g: 74,
                    b: 130,
                }),
            ));
        } else {
            let bg = palette.resolve(eff_bg, true);
            if bg != default_bg {
                bg_cells.push((c, eff_bg));
            }
        }

        if cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
            let fg = palette.resolve(eff_fg, false);
            let is_double = cell.flags.contains(CellFlags::DOUBLE_UNDERLINE);
            underline_cells.push((c, fg, is_double));
        }
        if cell.flags.contains(CellFlags::STRIKEOUT) {
            let fg = palette.resolve(eff_fg, false);
            strikeout_cells.push((c, fg));
        }

        let mut test_quads = Vec::new();
        let rendered = !cell.flags.contains(CellFlags::HIDDEN)
            && try_render_box_or_block(
                cell.c,
                0.0,
                0.0,
                char_width,
                line_height,
                [0.0; 4],
                &mut test_quads,
            );
        if rendered {
            geom_rendered[c] = true;
            geom_cells.push((c, cell.c, eff_fg));
        }
    }

    let mut segments = Vec::new();
    let mut col_idx = 0;
    while col_idx < cols {
        while col_idx < cols && (geom_rendered[col_idx] || line.cells[col_idx].c == ' ') {
            col_idx += 1;
        }
        if col_idx >= cols {
            break;
        }

        let seg_start = col_idx;
        let mut seg_end = col_idx;

        // One segment flows across Arabic and Latin text alike (so there are no gaps between
        // them); it only breaks on wide space runs, box drawing, or a background change.
        while seg_end < cols && !geom_rendered[seg_end] {
            if line.cells[seg_end].bg != line.cells[seg_start].bg {
                break;
            }

            if line.cells[seg_end].c == ' ' {
                let mut space_run = 0;
                let mut peek = seg_end;
                while peek < cols && line.cells[peek].c == ' ' && !geom_rendered[peek] {
                    space_run += 1;
                    peek += 1;
                }
                if (peek < cols && geom_rendered[peek])
                    || (peek < cols && line.cells[peek].bg != line.cells[seg_start].bg)
                {
                    break;
                }

                let is_active_input_space = cursor.is_visible
                    && cursor.row == r
                    && cursor.col >= seg_end
                    && cursor.col <= peek;

                if space_run >= 3 && !is_active_input_space {
                    break;
                }
            }
            seg_end += 1;
        }

        let trim_limit = if cursor.is_visible && cursor.row == r && cursor.col >= seg_start {
            cursor.col
        } else {
            seg_start
        };

        while seg_end > trim_limit && line.cells[seg_end - 1].c == ' ' {
            seg_end -= 1;
        }
        col_idx = seg_end;
        if seg_start >= seg_end {
            continue;
        }

        let seg_x = padding_left + seg_start as f32 * char_width;
        let num_cells = seg_end - seg_start;
        let (buf, seg_has_rtl, seg_needs_fallback, cell_ranges) = shape_segment(
            line,
            seg_start,
            seg_end,
            block_cursor_col,
            metrics,
            default_attrs,
            palette,
            font_system,
        );

        if seg_has_rtl {
            let runs: Vec<_> = buf.layout_runs().collect();
            let mut cell_visual_x = Vec::with_capacity(num_cells);
            let mut cell_edges = Vec::with_capacity(num_cells);
            for (i, &(b_start, b_end)) in cell_ranges.iter().enumerate() {
                let mut min_x = f32::INFINITY;
                let mut max_x = f32::NEG_INFINITY;
                let mut rtl = false;
                for run in &runs {
                    for g in run.glyphs.iter() {
                        if g.start < b_end && g.end > b_start {
                            min_x = min_x.min(g.x);
                            max_x = max_x.max(g.x + g.w);
                            rtl |= g.level.is_rtl();
                        }
                    }
                }
                if min_x.is_infinite() {
                    let x = seg_x + i as f32 * char_width;
                    cell_visual_x.push((x, char_width));
                    cell_edges.push((x, x + char_width, false));
                } else {
                    cell_visual_x.push((seg_x + min_x, (max_x - min_x).max(char_width)));
                    cell_edges.push((seg_x + min_x, seg_x + max_x, rtl));
                }
            }
            // Arabic glyphs are usually narrower than the grid: stretch the visually last cell
            // to the segment's grid end so backgrounds and selection leave no hole.
            let grid_end = seg_x + num_cells as f32 * char_width;
            if let Some(last) = cell_visual_x
                .iter_mut()
                .max_by(|a, b| (a.0 + a.1).total_cmp(&(b.0 + b.1)))
            {
                last.1 = last.1.max(grid_end - last.0);
            }
            segments.push(CachedSegment {
                seg_start,
                seg_end,
                seg_x,
                buffer: buf,
                has_rtl: true,
                cell_visual_x,
                cell_edges,
            });
            continue;
        }

        let grid_x = |start: usize, n: usize| -> Vec<(f32, f32)> {
            (0..n)
                .map(|i| (padding_left + (start + i) as f32 * char_width, char_width))
                .collect()
        };
        let grid_edges = |start: usize, n: usize| -> Vec<(f32, f32, bool)> {
            (0..n)
                .map(|i| {
                    let x = padding_left + (start + i) as f32 * char_width;
                    (x, x + char_width, false)
                })
                .collect()
        };

        if !seg_needs_fallback
            || is_grid_aligned(
                &buf,
                &cell_byte_offsets(line, seg_start, seg_end, 0),
                char_width,
            )
        {
            segments.push(CachedSegment {
                seg_start,
                seg_end,
                seg_x,
                buffer: buf,
                has_rtl: false,
                cell_visual_x: grid_x(seg_start, num_cells),
                cell_edges: grid_edges(seg_start, num_cells),
            });
            continue;
        }

        // A fallback glyph (icon, emoji, CJK…) has a different advance than the grid.
        // Pin each non-ASCII cell to its own column and keep ASCII runs together, so the
        // rest of the line can't drift while code tokens like `7abaf40` stay one run.
        let is_plain = |cell: &CellData| cell.c.is_ascii() && cell.zerowidth.is_empty();
        let mut c = seg_start;
        while c < seg_end {
            let cell = &line.cells[c];
            if cell.c == ' ' || cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                c += 1;
                continue;
            }
            let mut run_end = c + 1;
            if is_plain(cell) {
                while run_end < seg_end && is_plain(&line.cells[run_end]) {
                    run_end += 1;
                }
                while line.cells[run_end - 1].c == ' ' {
                    run_end -= 1;
                }
            }
            let (buffer, _, _, _) = shape_segment(
                line,
                c,
                run_end,
                block_cursor_col,
                metrics,
                default_attrs,
                palette,
                font_system,
            );
            segments.push(CachedSegment {
                seg_start: c,
                seg_end: run_end,
                seg_x: padding_left + c as f32 * char_width,
                buffer,
                has_rtl: false,
                cell_visual_x: grid_x(c, run_end - c),
                cell_edges: grid_edges(c, run_end - c),
            });
            c = run_end;
        }
    }

    CachedRow {
        hash: current_hash,
        segments,
        geom_cells,
        bg_cells,
        underline_cells,
        strikeout_cells,
    }
}
