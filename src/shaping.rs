use alacritty_terminal::vte::ansi::Color as AnsiColor;
use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping};
use unicode_bidi::{bidi_class, BidiClass};

use crate::color::{Palette, Rgba};
use crate::quad::try_render_box_or_block;
use crate::terminal::{CellData, CursorState, LineData};

#[derive(Clone)]
pub struct CachedSegment {
    pub seg_start: usize,
    pub seg_end: usize,
    pub seg_x: f32,
    pub buffer: Buffer,
    #[allow(dead_code)]
    pub has_rtl: bool,
}

#[derive(Clone)]
pub struct CachedRow {
    pub hash: u64,
    pub segments: Vec<CachedSegment>,
    pub geom_cells: Vec<(usize, char, AnsiColor)>,
    pub bg_cells: Vec<(usize, AnsiColor)>,
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
    let current_hash = hash_cells(&line.cells);

    let mut geom_cells = Vec::new();
    let mut bg_cells = Vec::new();
    let mut geom_rendered = vec![false; cols];

    for (c, cell) in line.cells.iter().enumerate() {
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
            let bg = palette.resolve(cell.bg, true);
            if bg != default_bg {
                bg_cells.push((c, cell.bg));
            }
        }
        let mut test_quads = Vec::new();
        let rendered = try_render_box_or_block(
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
            geom_cells.push((c, cell.c, cell.fg));
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

        if seg_start < seg_end {
            let seg_x = padding_left + seg_start as f32 * char_width;

            let mut spans_data: Vec<(String, Attrs)> = Vec::new();
            let mut cur_text = String::new();
            let mut cur_attrs: Option<Attrs> = None;
            let mut seg_has_rtl = false;

            for cell in line.cells[seg_start..seg_end].iter() {
                if cell
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
                {
                    continue;
                }

                let class = bidi_class(cell.c);
                if class == BidiClass::R || class == BidiClass::AL {
                    seg_has_rtl = true;
                }

                let fg = if cell.is_selected {
                    Rgba::from_rgb8(245, 245, 255)
                } else {
                    palette.resolve(cell.fg, false)
                };
                let mut attrs = default_attrs.clone().color(fg.to_glyphon());
                if cell
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::BOLD)
                {
                    attrs = attrs.weight(cosmic_text::Weight::BOLD);
                }
                if cell
                    .flags
                    .contains(alacritty_terminal::term::cell::Flags::ITALIC)
                {
                    attrs = attrs.style(cosmic_text::Style::Italic);
                }

                let attrs_match = cur_attrs
                    .as_ref()
                    .map(|a| a.color_opt == attrs.color_opt && a.weight == attrs.weight)
                    .unwrap_or(false);

                if attrs_match {
                    cur_text.push(cell.c);
                    for &z in &cell.zerowidth {
                        cur_text.push(z);
                    }
                } else {
                    if !cur_text.is_empty() {
                        if let Some(prev) = cur_attrs.take() {
                            spans_data.push((std::mem::take(&mut cur_text), prev));
                        }
                    }
                    cur_text.push(cell.c);
                    for &z in &cell.zerowidth {
                        cur_text.push(z);
                    }
                    cur_attrs = Some(attrs);
                }
            }

            if !cur_text.is_empty() {
                if let Some(prev) = cur_attrs.take() {
                    spans_data.push((cur_text, prev));
                }
            }

            if !spans_data.is_empty() {
                if seg_has_rtl {
                    spans_data.insert(0, ("\u{200E}".to_string(), default_attrs.clone()));
                }

                let mut buf = Buffer::new_empty(metrics);
                let span_refs: Vec<(&str, Attrs)> = spans_data
                    .iter()
                    .map(|(s, a)| (s.as_str(), a.clone()))
                    .collect();

                let shaping_mode = if seg_has_rtl {
                    Shaping::Advanced
                } else {
                    Shaping::Basic
                };

                buf.set_rich_text(span_refs, default_attrs, shaping_mode, None);
                buf.shape_until_scroll(font_system, false);

                segments.push(CachedSegment {
                    seg_start,
                    seg_end,
                    seg_x,
                    buffer: buf,
                    has_rtl: seg_has_rtl,
                });
            }
        }

        col_idx = seg_end;
    }

    CachedRow {
        hash: current_hash,
        segments,
        geom_cells,
        bg_cells,
    }
}
