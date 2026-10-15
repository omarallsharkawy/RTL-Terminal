use crate::fonts::{grid_family, installed_family, new_font_system};
use alacritty_terminal::index::Side;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use glyphon::{
    Cache, ColorMode, Resolution, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer,
    Viewport,
};
use std::collections::HashMap;
use std::sync::Arc;
use unicode_bidi::{bidi_class, BidiClass};
use winit::window::Window;

use crate::color::{Palette, Rgba};
use crate::layout::{tab_rects, truncate_title, PaneId, Rect};
use crate::quad::{try_render_box_or_block, QuadRenderer};
use crate::terminal::{CellData, CursorState, LineData};

/// Line height as a multiple of the font size (tall enough for Arabic harakat).
const LINE_HEIGHT_FACTOR: f32 = 1.55;
/// Logical (DPI-independent) window padding.
const PADDING_LEFT: f32 = 8.0;
const PADDING_TOP: f32 = 6.0;
/// Left-to-right mark prepended to RTL segments so prompts never flip direction.
const LRM: &str = "\u{200E}";
/// Right-to-left mark appended after spaces typed at the end of Arabic, so they stay on its left.
const RLM: &str = "\u{200F}";
/// Left-to-right isolate / pop directional isolate: wrap Latin words and numbers inside Arabic so
/// hashes like `7abaf40` are never torn into `7` … `abaf40`.
const LRI: &str = "\u{2066}";
const PDI: &str = "\u{2069}";

type Quad = (f32, f32, f32, f32, [f32; 4]);

/// Widest a tab in the tab bar gets, in cells.
const MAX_TAB_CELLS: f32 = 28.0;

/// Everything the renderer needs to draw one pane.
pub struct PaneFrame {
    pub id: PaneId,
    /// Where the pane goes in the window (physical pixels, padding included).
    pub rect: Rect,
    pub lines: Vec<LineData>,
    pub cursor: CursorState,
    /// The pane keyboard input goes to; the others get a hollow cursor.
    pub focused: bool,
    /// One-line bar drawn over the pane's last row (search query), if any.
    pub status: Option<String>,
}

/// A tab in the tab bar.
pub struct TabLabel {
    pub title: String,
    pub active: bool,
}

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    /// X offset relative to the left padding.
    pub seg_x: f32,
    pub buffer: Buffer,
    pub has_rtl: bool,
    /// Byte offset (into the shaped text) where each cell of the segment starts.
    pub cell_bytes: Vec<usize>,
}

impl CachedSegment {
    /// Visual `(left, right)` x of each cell (relative to the left padding), taken from the glyphs
    /// that hold it so backgrounds follow reordered RTL text. A glyph shared by several cells
    /// (ligature, cluster) is split between them; cells without a glyph are `None`, spacers are
    /// empty.
    pub fn cell_extents(&self, cells: &[CellData]) -> Vec<Option<(f32, f32)>> {
        let mut out: Vec<Option<(f32, f32)>> = vec![None; self.seg_end - self.seg_start];
        let mut real = Vec::new();
        let mut real_bytes = Vec::new();
        for (i, cell) in cells[self.seg_start..self.seg_end].iter().enumerate() {
            if cell.is_spacer() {
                continue;
            }
            real.push(i);
            real_bytes.push(self.cell_bytes[i]);
        }
        if real.is_empty() {
            return out;
        }
        // Zero-width glyphs (marks, isolates) are reordered far from their cell, so they only
        // place cells that have no sized glyph, as an empty extent.
        for sized in [true, false] {
            for run in self.buffer.layout_runs() {
                for g in run.glyphs.iter().filter(|g| (g.w > 0.0) == sized) {
                    let i0 = real_bytes
                        .partition_point(|&b| b <= g.start)
                        .saturating_sub(1);
                    let i1 = real_bytes
                        .partition_point(|&b| b < g.end)
                        .saturating_sub(1)
                        .max(i0);
                    let n = (i1 - i0 + 1) as f32;
                    let part = g.w / n;
                    for k in 0..=(i1 - i0) {
                        let left = if g.level.is_rtl() {
                            g.x + g.w - (k + 1) as f32 * part
                        } else {
                            g.x + k as f32 * part
                        };
                        let (l, r) = (self.seg_x + left, self.seg_x + left + part);
                        let slot = &mut out[real[i0 + k]];
                        match slot {
                            None => *slot = Some((l, r)),
                            Some((a, b)) if sized => *slot = Some((a.min(l), b.max(r))),
                            Some(_) => {}
                        }
                    }
                }
            }
        }
        out
    }
}

#[derive(Clone, Default)]
pub struct CachedRow {
    pub hash: u64,
    /// Visual extents of cells in RTL segments (see `visual_cell_extents`), for hit testing.
    pub visual: Vec<Option<(f32, f32)>>,
    /// Cells whose character is right-to-left (their left and right halves are swapped).
    pub rtl_cells: Vec<bool>,
    /// Cells holding a plain space (see `CellData::is_blank`).
    pub blank_cells: Vec<bool>,
    pub segments: Vec<CachedSegment>,
    /// Backgrounds, box drawing and text decorations, relative to the row origin.
    pub quads: Vec<Quad>,
}

/// Cursor details that change how a row is drawn (the glyph under a block cursor is recolored).
#[derive(Clone, Copy, PartialEq, Hash)]
pub struct RowCursor {
    pub col: usize,
    pub block: bool,
}

impl CachedRow {
    /// The cell drawn under `x` (relative to the row origin) and which side of it, so mouse
    /// selection and mouse reports follow what is on screen even where Arabic is reordered.
    pub fn hit_test(&self, x: f32, char_width: f32, cols: usize) -> (usize, Side) {
        let is_rtl = |c: usize| self.rtl_cells.get(c).copied().unwrap_or(false);
        let sized = || {
            self.visual
                .iter()
                .enumerate()
                .filter_map(|(c, e)| e.filter(|(l, r)| r > l).map(|e| (c, e)))
        };
        let in_rtl_segment = |x: f32| {
            self.segments.iter().any(|s| {
                s.has_rtl
                    && x >= s.seg_start as f32 * char_width
                    && x < s.seg_end as f32 * char_width
            })
        };
        let hit = sized().find(|(_, (l, r))| x >= *l && x < *r).or_else(|| {
            // Inside an RTL segment but between glyphs: take the nearest drawn cell.
            in_rtl_segment(x).then(|| {
                sized().min_by(|(_, a), (_, b)| {
                    let d = |(l, r): (f32, f32)| (l - x).abs().min((r - x).abs());
                    d(*a).total_cmp(&d(*b))
                })
            })?
        });
        if let Some((col, (l, r))) = hit {
            let left_half = x < (l + r) / 2.0;
            let side = if left_half != is_rtl(col) {
                Side::Left
            } else {
                Side::Right
            };
            return (col, side);
        }
        let rel = (x / char_width).max(0.0);
        let col = (rel as usize).min(cols.saturating_sub(1));
        let side = if rel.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        (col, side)
    }

    /// Where the cursor goes (relative to the left padding), and whether it should extend to the
    /// left of that point. After Arabic text the caret sits on the text's left, where the next
    /// Arabic letter will appear; after Latin text it sits on the right.
    pub fn cursor_anchor(&self, col: usize, char_width: f32) -> (f32, bool) {
        let row = self;
        let grid_x = col as f32 * char_width;
        let glyph_of = |seg: &CachedSegment, c: usize| {
            let target = seg.cell_bytes[c - seg.seg_start];
            seg.buffer.layout_runs().find_map(|run| {
                run.glyphs
                    .iter()
                    .find(|g| target >= g.start && target < g.end)
                    .map(|g| (seg.seg_x + g.x, seg.seg_x + g.x + g.w, g.level.is_rtl()))
            })
        };
        for (i, seg) in row.segments.iter().enumerate() {
            if !seg.has_rtl {
                continue;
            }
            if col >= seg.seg_start && col < seg.seg_end {
                // Inside the text: the insertion point is the cell's leading edge.
                return match glyph_of(seg, col) {
                    Some((_, right, true)) => (right, true),
                    Some((left, _, false)) => (left, false),
                    None => (grid_x, false),
                };
            }
            let next_start = row.segments.get(i + 1).map_or(usize::MAX, |s| s.seg_start);
            // Only spaces may separate the caret from the text it follows; past a border or
            // other content it stays on its own cell.
            let only_spaces = || (seg.seg_end..col).all(|c| row.blank_cells.get(c) == Some(&true));
            if col >= seg.seg_end && col < next_start && only_spaces() {
                let gap = (col - seg.seg_end) as f32 * char_width;
                return match glyph_of(seg, seg.seg_end - 1) {
                    Some((left, _, true)) => (left - gap, true),
                    Some((_, right, false)) => (right + gap, false),
                    None => (grid_x, false),
                };
            }
        }
        (grid_x, false)
    }
}

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
            let mut attrs = Attrs::new()
                .family(grid_family(self.family))
                .color(fg.to_glyphon());
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

pub struct Renderer {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub quad_renderer: QuadRenderer,
    pub font_system: FontSystem,
    /// Configured terminal font family (leaked once per change; `None` = monospace).
    pub font_family: Option<&'static str>,
    pub swash_cache: SwashCache,
    pub text_atlas: TextAtlas,
    pub text_renderer: TextRenderer,
    pub viewport: Viewport,
    pub palette: Palette,
    /// Font size in logical points (what the user zooms and what is saved).
    pub font_size: f32,
    /// Window scale factor (1.0 = 96 DPI).
    pub scale: f32,
    /// Cell metrics in physical pixels.
    pub char_width: f32,
    pub line_height: f32,
    pub padding_left: f32,
    pub padding_top: f32,
    pub is_srgb: bool,
    pub focused: bool,
    /// Window background opacity (1.0 = solid).
    pub opacity: f32,
    /// Configured cursor style ("block", "beam"/"bar", "underline").
    pub cursor_style: String,
    /// Blink phase: while false the cursor is not drawn (and its block glyph is not recolored).
    pub cursor_blink_on: bool,
    /// Shaped rows of every pane, so a pane that did not change is not reshaped.
    pub pane_caches: HashMap<PaneId, Vec<CachedRow>>,
    /// Input-method composition drawn at the cursor.
    pub ime: crate::ime::PreeditState,
}

fn hash_row(cells: &[CellData], cursor: Option<RowCursor>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for cell in cells {
        cell.c.hash(&mut hasher);
        cell.zerowidth.hash(&mut hasher);
        hash_color(&cell.fg, &mut hasher);
        hash_color(&cell.bg, &mut hasher);
        cell.flags.bits().hash(&mut hasher);
        cell.selected.hash(&mut hasher);
        cell.search_match.hash(&mut hasher);
        cell.link_hover.hash(&mut hasher);
    }
    cursor.hash(&mut hasher);
    hasher.finish()
}

fn hash_color<H: std::hash::Hasher>(color: &alacritty_terminal::vte::ansi::Color, hasher: &mut H) {
    use alacritty_terminal::vte::ansi::Color;
    use std::hash::Hash;
    match color {
        Color::Named(n) => (0u8, *n as u8).hash(hasher),
        Color::Spec(rgb) => (1u8, rgb.r, rgb.g, rgb.b).hash(hasher),
        Color::Indexed(idx) => (2u8, *idx).hash(hasher),
    }
}

fn is_rtl_char(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// True when the last strong character is Arabic/Hebrew and only neutrals (spaces, punctuation,
/// symbols) follow it. Digits are excluded: numbers are isolated and keep their own side.
fn trails_rtl_neutrals(cells: &[CellData]) -> bool {
    let mut trailing = 0;
    for cell in cells.iter().rev() {
        match bidi_class(cell.c) {
            BidiClass::R | BidiClass::AL => return trailing > 0,
            BidiClass::L | BidiClass::EN | BidiClass::AN => return false,
            _ => trailing += 1,
        }
    }
    false
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a,
    }
}

/// Resolve a cell's effective (foreground, background) honoring INVERSE, DIM and HIDDEN.
pub fn cell_colors(palette: &Palette, cell: &CellData) -> (Rgba, Rgba) {
    let mut fg = palette.resolve(cell.fg, false);
    let mut bg = palette.resolve(cell.bg, true);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(CellFlags::DIM) {
        fg = mix(fg, bg, 0.34);
    }
    if cell.search_match > 0 {
        bg = if cell.search_match > 1 {
            palette.search_focus
        } else {
            palette.search_match
        };
        fg = palette.background;
    }
    if cell.selected {
        bg = palette.selection;
    }
    // Last, so selecting or searching never reveals concealed text (passwords, `\e[8m`).
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}

/// Width of the cursor on `col`: two cells on a double-width character, so a block cursor
/// covers all of the glyph it recolors.
pub fn cursor_cell_width(line: &LineData, col: usize, cw: f32) -> f32 {
    match line.cells.get(col) {
        Some(cell) if cell.flags.contains(CellFlags::WIDE_CHAR) => cw * 2.0,
        _ => cw,
    }
}

/// Box drawing or block element of `cell` in its background color, for drawing on top of a
/// block cursor at `rect` (x, y, w, h). Text under the cursor gets the same recolor when shaped;
/// without this, geometry would vanish beneath the cursor.
pub fn geometry_under_block_cursor(
    palette: &Palette,
    is_srgb: bool,
    cell: &CellData,
    (x, y, w, h): (f32, f32, f32, f32),
) -> Vec<Quad> {
    let mut quads = Vec::new();
    if !cell.flags.contains(CellFlags::HIDDEN) {
        let (_, bg) = cell_colors(palette, cell);
        let color = if is_srgb {
            bg.to_linear()
        } else {
            bg.to_array()
        };
        try_render_box_or_block(cell.c, x, y, w, h, color, &mut quads);
    }
    quads
}

/// Where each cell of a row is drawn, relative to the row origin: `Some((left, right))` for
/// cells inside RTL segments (they follow their glyphs), `None` for cells on the grid.
pub fn visual_cell_extents(
    line: &LineData,
    segments: &[CachedSegment],
    cw: f32,
) -> Vec<Option<(f32, f32)>> {
    let mut visual: Vec<Option<(f32, f32)>> = vec![None; line.cells.len()];
    for seg in segments.iter().filter(|s| s.has_rtl) {
        let mut extents = seg.cell_extents(&line.cells);
        // Arabic glyphs are usually narrower than the cells they occupy: stretch the visually
        // outermost cells to the segment's grid edges, so colored backgrounds (selections, TUI
        // highlights) don't leave unpainted holes at the end of the text.
        let (grid_l, grid_r) = (seg.seg_start as f32 * cw, seg.seg_end as f32 * cw);
        let sized = |e: &Option<(f32, f32)>| e.filter(|(l, r)| r > l);
        if let Some(i) = (0..extents.len())
            .filter(|&i| sized(&extents[i]).is_some())
            .max_by(|&a, &b| extents[a].unwrap().1.total_cmp(&extents[b].unwrap().1))
        {
            if let Some((_, r)) = extents[i].as_mut() {
                *r = r.max(grid_r);
            }
        }
        if let Some(i) = (0..extents.len())
            .filter(|&i| sized(&extents[i]).is_some())
            .min_by(|&a, &b| extents[a].unwrap().0.total_cmp(&extents[b].unwrap().0))
        {
            if let Some((l, _)) = extents[i].as_mut() {
                *l = l.min(grid_l);
            }
        }
        for (i, ext) in extents.into_iter().enumerate() {
            let c = seg.seg_start + i;
            visual[c] = if line.cells[c].is_spacer() {
                let x = c as f32 * cw;
                Some((x, x))
            } else {
                ext
            };
        }
    }
    visual
}

/// Backgrounds, box geometry and decorations of a row. Cells inside RTL segments follow the
/// glyphs that hold them; everything else sits on the grid. `metrics` is (cell width, line
/// height, font px).
pub fn build_row_quads(
    palette: &Palette,
    is_srgb: bool,
    metrics: (f32, f32, f32),
    line: &LineData,
    segments: &[CachedSegment],
) -> Vec<Quad> {
    let (cw, lh, font_px) = metrics;
    let target = |rgba: Rgba| {
        if is_srgb {
            rgba.to_linear()
        } else {
            rgba.to_array()
        }
    };
    let thickness = (font_px / 14.0).round().max(1.0);
    let underline_y = ((lh + font_px) * 0.5).round().min(lh - thickness);
    let strike_y = (lh * 0.5).round();

    let visual = visual_cell_extents(line, segments, cw);

    let mut quads = Vec::new();
    for (c, cell) in line.cells.iter().enumerate() {
        let (fg, bg) = cell_colors(palette, cell);
        let (x, w) = visual[c].map_or((c as f32 * cw, cw), |(l, r)| (l, r - l));
        if bg != palette.background && w > 0.0 {
            quads.push((x, 0.0, w, lh, target(bg)));
        }
        let fg_color = target(fg);
        if !cell.flags.contains(CellFlags::HIDDEN) {
            try_render_box_or_block(cell.c, x, 0.0, w, lh, fg_color, &mut quads);
        }
        if w <= 0.0 {
            continue;
        }
        if cell.flags.contains(CellFlags::DOUBLE_UNDERLINE) {
            quads.push((x, underline_y - thickness * 2.0, w, thickness, fg_color));
            quads.push((x, underline_y, w, thickness, fg_color));
        } else if cell.link_hover || cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
            quads.push((x, underline_y, w, thickness, fg_color));
        }
        if cell.flags.contains(CellFlags::STRIKEOUT) {
            quads.push((x, strike_y, w, thickness, fg_color));
        }
    }
    quads
}

/// How glyphon treats text colors. `Accurate` linearizes them for an sRGB surface; on the
/// non-sRGB surface that is preferred, colors must go out as-is (`Web`), like the quads, or text
/// is drawn darker than its palette color.
pub fn text_color_mode(is_srgb: bool) -> ColorMode {
    if is_srgb {
        ColorMode::Accurate
    } else {
        ColorMode::Web
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

impl Renderer {
    pub async fn new(window: Arc<Window>, initial_font_size: f32) -> anyhow::Result<Self> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone())?;
        let mut adapter = None;
        // Prefer a real GPU; fall back to a software adapter (e.g. WARP on Windows VMs / RDP).
        for force_fallback_adapter in [false, true] {
            if let Ok(a) = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter,
                    apply_limit_buckets: false,
                })
                .await
            {
                adapter = Some(a);
                break;
            }
        }
        let adapter = adapter.ok_or_else(|| anyhow::anyhow!("No compatible GPU adapter found"))?;

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("twitty device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
                memory_hints: Default::default(),
            })
            .await?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(surface_caps.formats[0]);
        let is_srgb = format.is_srgb();

        let present_mode = if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Immediate)
        {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::AutoVsync
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode,
            // A pre/post-multiplied surface lets the compositor show what is behind the window.
            alpha_mode: [
                wgpu::CompositeAlphaMode::PreMultiplied,
                wgpu::CompositeAlphaMode::PostMultiplied,
            ]
            .into_iter()
            .find(|m| surface_caps.alpha_modes.contains(m))
            .unwrap_or(surface_caps.alpha_modes[0]),
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
            color_space: Default::default(),
        };
        surface.configure(&device, &config);

        let quad_renderer = QuadRenderer::new(&device, format);
        quad_renderer.update_screen_size(&queue, width as f32, height as f32);

        let font_system = new_font_system();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let mut viewport = Viewport::new(&device, &cache);
        viewport.update(&queue, Resolution { width, height });

        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, text_color_mode(is_srgb));
        let text_renderer = TextRenderer::new(
            &mut text_atlas,
            &device,
            wgpu::MultisampleState::default(),
            None,
        );

        let scale = window.scale_factor() as f32;
        let mut renderer = Self {
            window,
            surface,
            device,
            queue,
            config,
            quad_renderer,
            font_system,
            font_family: None,
            swash_cache,
            text_atlas,
            text_renderer,
            viewport,
            palette: Palette::default(),
            font_size: initial_font_size,
            scale,
            char_width: 0.0,
            line_height: 0.0,
            padding_left: 0.0,
            padding_top: 0.0,
            is_srgb,
            focused: true,
            opacity: 1.0,
            cursor_style: "block".into(),
            cursor_blink_on: true,
            pane_caches: HashMap::new(),
            ime: Default::default(),
        };
        renderer.update_metrics();
        Ok(renderer)
    }

    pub(crate) fn font_px(&self) -> f32 {
        self.font_size * self.scale
    }

    /// Recompute cell metrics from the font size and scale factor.
    fn update_metrics(&mut self) {
        self.font_size = self.font_size.clamp(8.0, 48.0);
        let font_px = self.font_px();
        self.line_height = (font_px * LINE_HEIGHT_FACTOR).round();
        self.char_width = measure_char_width(
            &mut self.font_system,
            self.font_family,
            font_px,
            self.line_height,
        );
        self.padding_left = (PADDING_LEFT * self.scale).round();
        self.padding_top = (PADDING_TOP * self.scale).round();
        self.pane_caches.clear();
    }

    /// Use `name` for the terminal grid. An uninstalled family falls back to monospace.
    pub fn set_font_family(&mut self, name: Option<&str>) {
        let name = name.map(str::trim).filter(|n| !n.is_empty());
        self.font_family = match name {
            Some(n) => match installed_family(&self.font_system, n) {
                Some(installed) => Some(Box::leak(installed.into_boxed_str())),
                None => {
                    eprintln!("twitty: font_family {n:?} is not installed, using monospace");
                    None
                }
            },
            None => None,
        };
        self.update_metrics();
    }

    pub fn set_font_size(&mut self, new_size: f32) {
        self.font_size = new_size;
        self.update_metrics();
    }

    pub fn set_scale_factor(&mut self, scale: f32) {
        self.scale = scale.max(0.25);
        self.update_metrics();
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    /// Forget the cached rows of a closed pane.
    pub fn forget_pane(&mut self, id: PaneId) {
        self.pane_caches.remove(&id);
    }

    /// Background quad and shaped text for a pane's status line, positioned over its last row.
    fn status_line_parts(
        &mut self,
        rect: Rect,
        rows: usize,
        text: &str,
    ) -> Option<(Quad, Buffer, f32)> {
        let y = rect.y + self.padding_top + rows.checked_sub(1)? as f32 * self.line_height;
        let bar = mix(self.palette.background, self.palette.foreground, 0.14);
        let quad = (
            rect.x,
            y,
            rect.w,
            self.line_height,
            self.to_target_color(bar),
        );
        let metrics = Metrics::new(self.font_px(), self.line_height);
        let mut buffer = Buffer::new_empty(metrics);
        buffer.set_text(
            text,
            &Attrs::new().family(grid_family(self.font_family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);
        Some((quad, buffer, y))
    }

    pub fn set_opacity(&mut self, opacity: f32) {
        self.opacity = opacity.clamp(0.2, 1.0);
    }

    /// The configured style replaces the default block; shapes an app asks for (vim insert
    /// mode's beam, for example) are kept.
    fn effective_cursor_shape(&self, shape: CursorShape) -> CursorShape {
        if shape != CursorShape::Block {
            return shape;
        }
        match self.cursor_style.as_str() {
            "beam" | "bar" => CursorShape::Beam,
            "underline" => CursorShape::Underline,
            _ => CursorShape::Block,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
            self.viewport
                .update(&self.queue, Resolution { width, height });
            self.quad_renderer
                .update_screen_size(&self.queue, width as f32, height as f32);
            self.pane_caches.clear();
        }
    }

    /// Width of the divider between panes.
    pub fn divider_gap(&self) -> f32 {
        self.scale.round().max(1.0)
    }

    /// Height of the tab bar (shown only with two or more tabs).
    pub fn tab_bar_height(&self, tab_count: usize) -> f32 {
        if tab_count >= 2 {
            self.line_height
        } else {
            0.0
        }
    }

    /// The window area below the tab bar, where panes are laid out.
    pub fn content_rect(&self, tab_count: usize) -> Rect {
        let bar = self.tab_bar_height(tab_count);
        Rect::new(
            0.0,
            bar,
            self.config.width as f32,
            (self.config.height as f32 - bar).max(1.0),
        )
    }

    /// Grid size (cols, rows) of a pane occupying `rect`.
    pub fn compute_grid_size(&self, rect: Rect) -> (usize, usize) {
        let avail_w = (rect.w - self.padding_left * 2.0).max(10.0);
        let avail_h = (rect.h - self.padding_top * 2.0).max(10.0);
        let cols = (avail_w / self.char_width).floor() as usize;
        let rows = (avail_h / self.line_height).floor() as usize;
        (cols.max(10), rows.max(4))
    }

    /// Map a physical pixel position to a (col, row) cell of the pane in `rect`, clamped to
    /// its grid.
    pub fn pixel_to_cell(&self, rect: Rect, x: f64, y: f64) -> (usize, usize) {
        let (cols, rows) = self.compute_grid_size(rect);
        let col = ((x - (rect.x + self.padding_left) as f64) / self.char_width as f64).max(0.0);
        let row = ((y - (rect.y + self.padding_top) as f64) / self.line_height as f64).max(0.0);
        ((col as usize).min(cols - 1), (row as usize).min(rows - 1))
    }

    /// Map a pixel to the cell drawn there and its side. Unlike `pixel_to_cell`, this follows
    /// reordered Arabic glyphs, so selections and mouse reports match what is on screen.
    pub fn pixel_to_visual_cell(
        &self,
        pane: PaneId,
        rect: Rect,
        x: f64,
        y: f64,
    ) -> (usize, usize, Side) {
        let (col, row) = self.pixel_to_cell(rect, x, y);
        let (cols, _) = self.compute_grid_size(rect);
        let rel_x = (x - (rect.x + self.padding_left) as f64) as f32;
        match self.pane_caches.get(&pane).and_then(|rows| rows.get(row)) {
            Some(cached) => {
                let (col, side) = cached.hit_test(rel_x, self.char_width, cols);
                (col, row, side)
            }
            None => (col, row, self.pixel_side(rect, x)),
        }
    }

    /// Which half of its cell the pixel falls in (selections include a cell only when the
    /// pointer passes its middle).
    pub fn pixel_side(&self, rect: Rect, x: f64) -> Side {
        let rel = (x - (rect.x + self.padding_left) as f64) / self.char_width as f64;
        if rel.rem_euclid(1.0) < 0.5 {
            Side::Left
        } else {
            Side::Right
        }
    }

    fn shaper(&mut self) -> RowShaper<'_> {
        RowShaper {
            font_px: self.font_px(),
            font_system: &mut self.font_system,
            palette: &self.palette,
            char_width: self.char_width,
            line_height: self.line_height,
            family: self.font_family,
        }
    }

    pub(crate) fn to_target_color(&self, rgba: Rgba) -> [f32; 4] {
        if self.is_srgb {
            rgba.to_linear()
        } else {
            rgba.to_array()
        }
    }

    fn build_row_quads(&self, line: &LineData, segments: &[CachedSegment]) -> Vec<Quad> {
        build_row_quads(
            &self.palette,
            self.is_srgb,
            (self.char_width, self.line_height, self.font_px()),
            line,
            segments,
        )
    }

    fn cursor_quads(
        &self,
        (x, y, w): (f32, f32, f32),
        shape: CursorShape,
        focused: bool,
        out: &mut Vec<Quad>,
    ) {
        let h = self.line_height;
        let color = self.to_target_color(self.palette.cursor);
        let t = (self.scale * 1.5).round().max(1.0);
        let hollow = |out: &mut Vec<Quad>| {
            out.push((x, y, w, t, color));
            out.push((x, y + h - t, w, t, color));
            out.push((x, y, t, h, color));
            out.push((x + w - t, y, t, h, color));
        };
        match shape {
            CursorShape::Hidden => {}
            _ if !focused => hollow(out),
            CursorShape::Block => out.push((x, y, w, h, color)),
            CursorShape::HollowBlock => hollow(out),
            CursorShape::Beam => out.push((x, y, t, h, color)),
            CursorShape::Underline => out.push((x, y + h - t * 2.0, w, t * 2.0, color)),
        }
    }

    /// Draw the tab bar (with two or more tabs), every pane and the dividers between them.
    pub fn render(
        &mut self,
        tabs: &[TabLabel],
        panes: &[PaneFrame],
        dividers: &[Rect],
    ) -> anyhow::Result<()> {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                // Swapchain no longer matches the window (resize, display change, GPU reset).
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return Ok(());
            }
            _ => return Ok(()),
        };
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let default_bg_color = self.to_target_color(self.palette.background);
        let mut quads: Vec<Quad> = Vec::new();
        // The focused pane's cursor is separate: a preedit replaces it.
        let mut cursor_quads: Vec<Quad> = Vec::new();
        let mut other_cursor_quads: Vec<Quad> = Vec::new();
        let mut ime_caret: Option<(f32, f32, bool)> = None;
        let mut ime_bounds: Option<TextBounds> = None;
        let rect_bounds = |r: Rect| TextBounds {
            left: r.x as i32,
            top: r.y as i32,
            right: r.right() as i32,
            bottom: r.bottom() as i32,
        };

        let tab_geometry = if tabs.len() >= 2 {
            let max_tab = MAX_TAB_CELLS * self.char_width;
            tab_rects(
                tabs.len(),
                self.config.width as f32,
                self.line_height,
                max_tab,
            )
        } else {
            Vec::new()
        };
        if !tab_geometry.is_empty() {
            let bar = mix(self.palette.background, self.palette.foreground, 0.08);
            quads.push((
                0.0,
                0.0,
                self.config.width as f32,
                self.line_height,
                self.to_target_color(bar),
            ));
            let accent = (self.scale * 2.0).round().max(1.0);
            for (_, rect) in tabs.iter().zip(&tab_geometry).filter(|(t, _)| t.active) {
                quads.push((rect.x, rect.y, rect.w, rect.h, default_bg_color));
                quads.push((
                    rect.x,
                    rect.y,
                    rect.w,
                    accent,
                    self.to_target_color(self.palette.cursor),
                ));
            }
        }

        // Row caches are taken out of the map while drawing so shaping can borrow `self`.
        let mut caches: Vec<Vec<CachedRow>> = Vec::with_capacity(panes.len());
        for pane in panes {
            let rect = pane.rect;
            let (origin_x, origin_y) = (rect.x + self.padding_left, rect.y + self.padding_top);
            let mut rows = self.pane_caches.remove(&pane.id).unwrap_or_default();
            if rows.len() != pane.lines.len() {
                rows.resize(pane.lines.len(), CachedRow::default());
            }
            let shape = self.effective_cursor_shape(pane.cursor.shape);
            let cursor_focused = self.focused && pane.focused;

            for (r, line) in pane.lines.iter().enumerate() {
                let y = origin_y + r as f32 * self.line_height;
                // The cursor row stays marked during the blink's off phase: shaping depends on it
                // (an Arabic line being typed keeps its LTR base), only the block recolor turns off.
                let row_cursor =
                    (pane.cursor.is_visible && pane.cursor.row == r).then_some(RowCursor {
                        col: pane.cursor.col,
                        block: cursor_focused
                            && self.cursor_blink_on
                            && shape == CursorShape::Block,
                    });

                let hash = hash_row(&line.cells, row_cursor);
                if rows[r].hash != hash {
                    let segments = self.shaper().build_row(line, row_cursor);
                    let row_quads = self.build_row_quads(line, &segments);
                    rows[r] = CachedRow {
                        hash,
                        visual: visual_cell_extents(line, &segments, self.char_width),
                        rtl_cells: line.cells.iter().map(|c| is_rtl_char(c.c)).collect(),
                        blank_cells: line.cells.iter().map(CellData::is_blank).collect(),
                        segments,
                        quads: row_quads,
                    };
                }

                let cached = &rows[r];
                quads.extend(
                    cached
                        .quads
                        .iter()
                        .map(|&(x, qy, w, h, c)| (origin_x + x, y + qy, w, h, c)),
                );

                if pane.focused && pane.cursor.row == r {
                    let (x, ext) = cached.cursor_anchor(pane.cursor.col, self.char_width);
                    ime_caret = Some((origin_x + x, y, ext));
                    ime_bounds = Some(rect_bounds(rect));
                }

                // Only the focused pane's cursor blinks.
                if let Some(rc) = row_cursor.filter(|_| !pane.focused || self.cursor_blink_on) {
                    let (x, extends_left) = cached.cursor_anchor(rc.col, self.char_width);
                    let mut x = origin_x + x;
                    if extends_left {
                        x -= match shape {
                            CursorShape::Beam => (self.scale * 1.5).round().max(1.0),
                            _ => self.char_width,
                        };
                    }
                    let out = if pane.focused {
                        &mut cursor_quads
                    } else {
                        &mut other_cursor_quads
                    };
                    let w = cursor_cell_width(line, rc.col, self.char_width);
                    let x = x.max(0.0);
                    self.cursor_quads((x, y, w), shape, cursor_focused, out);
                    if let Some(cell) = line.cells.get(rc.col).filter(|_| rc.block) {
                        let rect = (x, y, w, self.line_height);
                        out.extend(geometry_under_block_cursor(
                            &self.palette,
                            self.is_srgb,
                            cell,
                            rect,
                        ));
                    }
                }
            }
            caches.push(rows);
        }

        let preedit_box = self.prepare_preedit(ime_caret, &mut quads, &mut cursor_quads);
        // The cursor sits above cell backgrounds but below text (its glyph was recolored).
        quads.extend(other_cursor_quads);
        quads.extend(cursor_quads);

        let mut statuses: Vec<Option<(Quad, Buffer, f32)>> = Vec::with_capacity(panes.len());
        for pane in panes {
            let status = match &pane.status {
                Some(text) => self.status_line_parts(pane.rect, pane.lines.len(), text),
                None => None,
            };
            if let Some((quad, _, _)) = &status {
                quads.push(*quad);
            }
            statuses.push(status);
        }

        let divider_color =
            self.to_target_color(mix(self.palette.background, self.palette.foreground, 0.28));
        quads.extend(dividers.iter().map(|d| (d.x, d.y, d.w, d.h, divider_color)));
        self.quad_renderer
            .set_rects(&self.device, &self.queue, &quads);

        // Tab titles are shaped with full shaping so Arabic titles render.
        let mut tab_texts: Vec<(Buffer, Rect, glyphon::Color)> = Vec::new();
        for (rect, tab) in tab_geometry.iter().zip(tabs) {
            let cols = ((rect.w / self.char_width).floor() as usize).saturating_sub(2);
            let title = truncate_title(&tab.title, cols);
            let mut buffer = Buffer::new_empty(Metrics::new(self.font_px(), self.line_height));
            buffer.set_text(
                &title,
                &Attrs::new().family(grid_family(self.font_family)),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut self.font_system, false);
            let color = if tab.active {
                self.palette.foreground
            } else {
                mix(self.palette.foreground, self.palette.background, 0.2)
            };
            tab_texts.push((buffer, *rect, color.to_glyphon()));
        }

        let mut text_areas: Vec<TextArea> = Vec::new();
        for (buffer, rect, color) in &tab_texts {
            text_areas.push(TextArea {
                buffer,
                left: rect.x + self.char_width,
                top: rect.y,
                scale: 1.0,
                bounds: rect_bounds(*rect),
                default_color: *color,
                custom_glyphs: &[],
            });
        }
        for ((pane, rows), status) in panes.iter().zip(&caches).zip(&statuses) {
            let bounds = rect_bounds(pane.rect);
            for (r, cached_row) in rows.iter().enumerate() {
                if status.is_some() && r + 1 == rows.len() {
                    break;
                }
                let y = pane.rect.y + self.padding_top + r as f32 * self.line_height;
                // The row under the preedit is clipped around it, so its text does not show
                // through the preedit's background.
                let (parts, n) = match preedit_box {
                    Some((left, _, width)) if pane.focused && pane.cursor.row == r => {
                        (crate::ime::bounds_outside(bounds, left, left + width), 2)
                    }
                    _ => ([bounds, bounds], 1),
                };
                for seg in &cached_row.segments {
                    for &bounds in &parts[..n] {
                        text_areas.push(TextArea {
                            buffer: &seg.buffer,
                            left: pane.rect.x + self.padding_left + seg.seg_x,
                            top: y,
                            scale: 1.0,
                            bounds,
                            default_color: self.palette.foreground.to_glyphon(),
                            custom_glyphs: &[],
                        });
                    }
                }
            }
            if let Some((_, buffer, y)) = status {
                text_areas.push(TextArea {
                    buffer,
                    left: pane.rect.x + self.padding_left,
                    top: *y,
                    scale: 1.0,
                    bounds,
                    default_color: self.palette.foreground.to_glyphon(),
                    custom_glyphs: &[],
                });
            }
        }
        if let Some((left, top, _)) = preedit_box {
            let bounds = ime_bounds.unwrap_or_else(|| {
                rect_bounds(Rect::new(
                    0.0,
                    0.0,
                    self.config.width as f32,
                    self.config.height as f32,
                ))
            });
            text_areas.extend(self.ime.text_area(
                (left, top),
                bounds,
                self.palette.foreground.to_glyphon(),
            ));
        }

        let prepared = self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.text_atlas,
            &self.viewport,
            text_areas,
            &mut self.swash_cache,
        );
        for (pane, rows) in panes.iter().zip(caches) {
            self.pane_caches.insert(pane.id, rows);
        }
        prepared.map_err(|e| anyhow::anyhow!("Text prepare error: {:?}", e))?;

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("twitty render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("twitty render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            // Premultiplied, so transparency keeps the theme's tint.
                            r: (default_bg_color[0] * self.opacity) as f64,
                            g: (default_bg_color[1] * self.opacity) as f64,
                            b: (default_bg_color[2] * self.opacity) as f64,
                            a: self.opacity as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.quad_renderer.render(&mut pass);
            self.text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .map_err(|e| anyhow::anyhow!("Text render error: {:?}", e))?;
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_texture);

        self.text_atlas.trim();

        Ok(())
    }
}
