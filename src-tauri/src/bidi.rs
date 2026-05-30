use serde::Serialize;
use unicode_width::UnicodeWidthChar;

use crate::terminal::CellStyle;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisualCell {
    pub ch: String,
    pub logical_index: usize,
    pub width: usize,
    pub style: CellStyle,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalLine {
    pub row: usize,
    pub base_direction: Direction,
    pub cells: Vec<VisualCell>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Rtl,
    Ltr,
}

pub fn visual_line(row: usize, logical: &[(char, CellStyle)], force_rtl: bool) -> TerminalLine {
    let text: String = logical.iter().map(|(ch, _)| *ch).collect();
    let base_direction = if force_rtl || first_strong_is_rtl(&text) {
        Direction::Rtl
    } else {
        Direction::Ltr
    };

    let cells = logical
        .iter()
        .enumerate()
        .map(|(logical_index, (ch, style))| VisualCell {
            ch: ch.to_string(),
            logical_index,
            width: ch.width().unwrap_or(1).max(1).min(2),
            style: style.clone(),
        })
        .collect();

    TerminalLine { row, base_direction, cells }
}

fn first_strong_is_rtl(text: &str) -> bool {
    for ch in text.chars() {
        if matches!(ch as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF) {
            return true;
        }
        if ch.is_ascii_alphabetic() {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_logical_text_for_canvas_shaping() {
        let style = CellStyle::default();
        let logical: Vec<_> = "سلام RTL".chars().map(|ch| (ch, style.clone())).collect();
        let line = visual_line(0, &logical, false);
        let text: String = line.cells.iter().map(|cell| cell.ch.as_str()).collect();
        assert_eq!(text, "سلام RTL");
        assert!(matches!(line.base_direction, Direction::Rtl));
    }
}
