//! Pure, window-free geometry and hit-testing for command block cards and their
//! floating top-right action toolbars. Shared by rendering (`render::scene::blocks`) and pointer
//! input (`app::mouse`), so drawn cards and button clicks never drift apart.

use crate::blocks::{BlockDecor, BlockStatus};
use crate::layout::{PaneId, Rect};
use crate::render::metrics::CellMetrics;
use crate::render::ui_style::{RADIUS_6, SPACE_4};
use crate::ui::chrome::UiLanguage;

/// Action button in a block card's floating top-right toolbar.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BlockButton {
    ExplainError,
    CopyCommand,
    CopyOutput,
    AskAgent,
    Rerun,
}

/// Fixed-capacity `Copy` list of toolbar buttons (4 standard buttons, plus `ExplainError` on
/// failed blocks with a non-zero exit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarButtons {
    items: [(BlockButton, Rect, bool); 5],
    len: usize,
}

impl ToolbarButtons {
    pub fn as_slice(&self) -> &[(BlockButton, Rect, bool)] {
        &self.items[..self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [(BlockButton, Rect, bool)] {
        &mut self.items[..self.len]
    }

    pub fn iter(&self) -> std::slice::Iter<'_, (BlockButton, Rect, bool)> {
        self.as_slice().iter()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl std::ops::Deref for ToolbarButtons {
    type Target = [(BlockButton, Rect, bool)];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl std::ops::DerefMut for ToolbarButtons {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl IntoIterator for ToolbarButtons {
    type Item = (BlockButton, Rect, bool);
    type IntoIter = std::iter::Take<std::array::IntoIter<(BlockButton, Rect, bool), 5>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

impl<'a> IntoIterator for &'a ToolbarButtons {
    type Item = &'a (BlockButton, Rect, bool);
    type IntoIter = std::slice::Iter<'a, (BlockButton, Rect, bool)>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a> IntoIterator for &'a mut ToolbarButtons {
    type Item = &'a mut (BlockButton, Rect, bool);
    type IntoIter = std::slice::IterMut<'a, (BlockButton, Rect, bool)>;

    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

/// Which block card (and optionally which toolbar button) the pointer hovers in one pane.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct BlockHover {
    pub pane: PaneId,
    pub block: u64,
    pub button: Option<BlockButton>,
}

/// Geometry of a block card's floating top-right toolbar, in physical pixels.
///
/// The icon buttons are anchored to the toolbar's right edge so their hit rectangles depend
/// only on `pane_rect` and `metrics.scale` (never on font metrics). When a dim duration/exit label
/// fits to their left, the pill expands leftward to include `meta`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardToolbarLayout {
    /// Pill background rectangle (also used to clip terminal row text behind the toolbar).
    pub rect: Rect,
    /// Dim duration / exit code label area to the left of the buttons, when present and room fits.
    pub meta: Option<Rect>,
    /// Action buttons left to right: `(button, rect, enabled)`.
    pub buttons: ToolbarButtons,
}

impl CardToolbarLayout {
    pub fn with_language(mut self, language: UiLanguage, card: Rect) -> Self {
        self.rect = language.mirror(self.rect, card);
        self.meta = self.meta.map(|rect| language.mirror(rect, card));
        for (_, rect, _) in &mut self.buttons {
            *rect = language.mirror(*rect, card);
        }
        self
    }

    /// The enabled button under `(x, y)`, if any.
    pub fn hit_button(&self, x: f64, y: f64) -> Option<BlockButton> {
        self.buttons
            .iter()
            .find_map(|&(btn, rect, enabled)| (enabled && rect.contains(x, y)).then_some(btn))
    }
}

/// Geometry of one visible command block card, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardLayout {
    pub id: u64,
    pub status: BlockStatus,
    pub selected: bool,
    pub hovered: bool,
    /// Rounded background rectangle (strictly inside `pane_rect`; never shifts the terminal grid).
    pub card: Rect,
    /// Corner radii `[top_left, top_right, bottom_right, bottom_left]`; 0 on clipped viewport edges.
    pub radii: [f32; 4],
    /// Thin left status strip aligned to `card`'s leading edge, clear of column 0.
    pub strip: Rect,
    /// First visible viewport row covered by this block.
    pub first_visible_row: usize,
    /// Last visible viewport row covered by this block (inclusive).
    pub last_visible_row: usize,
    /// Block starts above the viewport (partially scrolled off top).
    pub clipped_top: bool,
    /// Block continues below the viewport (partially scrolled off bottom).
    pub clipped_bottom: bool,
    /// Floating top-right toolbar, present when the block is hovered.
    pub toolbar: Option<CardToolbarLayout>,
}

/// Visible viewport row span `[first, last]` (inclusive) for a block, or `None` if off screen.
pub fn visible_rows(decor: &BlockDecor, rows: usize) -> Option<(usize, usize)> {
    if rows == 0 {
        return None;
    }
    let first = decor.first_row.max(0);
    let last = decor.last_row.min(rows as i64 - 1);
    (first <= last && last >= 0).then_some((first as usize, last as usize))
}

/// Lay out visible command block cards for one pane.
///
/// Only blocks whose rows intersect `0..rows` and whose status is not [`BlockStatus::Prompt`]
/// become cards. The terminal grid is never moved or resized: the 4 px inner padding effect comes
/// purely from the card background extending into the pane padding and unobstructed margins.
pub fn layout_cards(
    pane_rect: Rect,
    metrics: CellMetrics,
    rows: usize,
    blocks: &[BlockDecor],
    hovered_block: Option<u64>,
    hidden_row: Option<usize>,
    mut meta_width: impl FnMut(&str) -> f32,
) -> Vec<CardLayout> {
    if rows == 0 || pane_rect.w <= 0.0 || pane_rect.h <= 0.0 || blocks.is_empty() {
        return Vec::new();
    }

    let scale = metrics.scale;
    let pad = (SPACE_4 * scale).round();
    let margin_x = pad;
    let margin_y = pad;
    let half_gap = (pad * 0.5).round();
    let origin_y = pane_rect.y + metrics.padding_top;

    // Horizontal card bounds: 4 px outward from column 0 / grid right, clamped inside pane_rect.
    let card_x = (pane_rect.x + metrics.padding_left - pad).max(pane_rect.x + margin_x);
    let card_right =
        (pane_rect.right() - metrics.padding_left + pad).min(pane_rect.right() - margin_x);
    let card_w = (card_right - card_x).max(0.0);
    if card_w <= 0.0 {
        return Vec::new();
    }
    let strip_w = (2.0 * scale).round().max(1.0).min(card_w);
    let radius = (RADIUS_6 * scale).round();

    // Collect only visible real command blocks (skip pending waiting prompts).
    let visible: Vec<(&BlockDecor, usize, usize)> = blocks
        .iter()
        .filter(|d| d.status != BlockStatus::Prompt)
        .filter_map(|d| visible_rows(d, rows).map(|(f, l)| (d, f, l)))
        .collect();

    let mut out = Vec::with_capacity(visible.len());
    for (idx, &(decor, first, last)) in visible.iter().enumerate() {
        let clipped_top = decor.first_row < 0;
        let clipped_bottom = decor.last_row >= rows as i64;
        let adjacent_above = idx > 0 && visible[idx - 1].0.last_row + 1 == decor.first_row;
        let adjacent_below =
            idx + 1 < visible.len() && visible[idx + 1].0.first_row == decor.last_row + 1;

        let rows_top = origin_y + first as f32 * metrics.line_height;
        let rows_bottom = origin_y + (last + 1) as f32 * metrics.line_height;

        let card_top = if clipped_top {
            pane_rect.y
        } else if adjacent_above {
            rows_top + half_gap
        } else {
            (rows_top - pad).max(pane_rect.y + margin_y)
        };
        let card_bottom = if clipped_bottom {
            pane_rect.bottom()
        } else if adjacent_below {
            rows_bottom - half_gap
        } else {
            (rows_bottom + pad).min(pane_rect.bottom() - margin_y)
        };
        let card_h = (card_bottom - card_top).max(0.0);
        if card_h <= 0.0 {
            continue;
        }

        let card = Rect::new(card_x, card_top, card_w, card_h);
        let top_r = if clipped_top { 0.0 } else { radius };
        let bot_r = if clipped_bottom { 0.0 } else { radius };
        let radii = [top_r, top_r, bot_r, bot_r];
        let strip = Rect::new(card_x, card_top, strip_w, card_h);

        let hovered = hovered_block == Some(decor.id) && decor.id != 0;
        let show_toolbar = hovered && hidden_row != Some(first);
        let toolbar = show_toolbar
            .then(|| {
                layout_toolbar(
                    card,
                    strip_w,
                    rows_top,
                    metrics,
                    decor.has_command,
                    matches!(decor.status, BlockStatus::Failed(_)),
                    decor.meta.as_deref().map(&mut meta_width),
                )
            })
            .flatten();

        out.push(CardLayout {
            id: decor.id,
            status: decor.status,
            selected: decor.selected,
            hovered,
            card,
            radii,
            strip,
            first_visible_row: first,
            last_visible_row: last,
            clipped_top,
            clipped_bottom,
            toolbar,
        });
    }
    out
}

fn layout_toolbar(
    card: Rect,
    strip_w: f32,
    first_row_y: f32,
    metrics: CellMetrics,
    has_command: bool,
    failed: bool,
    meta_w: Option<f32>,
) -> Option<CardToolbarLayout> {
    let scale = metrics.scale;
    let pad = (SPACE_4 * scale).round();
    let inner_pad = pad;
    let btn_w = (20.0 * scale).round();
    let toolbar_h = (24.0 * scale).round().min(card.h);
    let btn_h = btn_w.min(toolbar_h);
    if toolbar_h < (10.0 * scale) {
        return None;
    }

    let min_left = card.x + strip_w + pad;
    let right = card.right() - pad;
    let btn_count = if failed { 5 } else { 4 };
    let buttons_w = btn_count as f32 * btn_w + 2.0 * inner_pad;
    if right - buttons_w < min_left {
        return None;
    }

    // Center on the first visible row, clamped inside the card.
    let ideal_y = first_row_y + (metrics.line_height - toolbar_h) * 0.5;
    let y = ideal_y
        .max(card.y + (1.0 * scale).round())
        .min((card.bottom() - toolbar_h).max(card.y));
    let btn_y = y + (toolbar_h - btn_h) * 0.5;

    let x_rerun = right - inner_pad - btn_w;
    let x_agent = x_rerun - btn_w;
    let x_out = x_agent - btn_w;
    let x_cmd = x_out - btn_w;
    let x_explain = x_cmd - btn_w;
    let leftmost_btn = if failed { x_explain } else { x_cmd };

    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let buttons = if failed {
        ToolbarButtons {
            items: [
                (
                    BlockButton::ExplainError,
                    Rect::new(x_explain, btn_y, btn_w, btn_h),
                    true,
                ),
                (
                    BlockButton::CopyCommand,
                    Rect::new(x_cmd, btn_y, btn_w, btn_h),
                    has_command,
                ),
                (
                    BlockButton::CopyOutput,
                    Rect::new(x_out, btn_y, btn_w, btn_h),
                    true,
                ),
                (
                    BlockButton::AskAgent,
                    Rect::new(x_agent, btn_y, btn_w, btn_h),
                    true,
                ),
                (
                    BlockButton::Rerun,
                    Rect::new(x_rerun, btn_y, btn_w, btn_h),
                    has_command,
                ),
            ],
            len: 5,
        }
    } else {
        ToolbarButtons {
            items: [
                (
                    BlockButton::CopyCommand,
                    Rect::new(x_cmd, btn_y, btn_w, btn_h),
                    has_command,
                ),
                (
                    BlockButton::CopyOutput,
                    Rect::new(x_out, btn_y, btn_w, btn_h),
                    true,
                ),
                (
                    BlockButton::AskAgent,
                    Rect::new(x_agent, btn_y, btn_w, btn_h),
                    true,
                ),
                (
                    BlockButton::Rerun,
                    Rect::new(x_rerun, btn_y, btn_w, btn_h),
                    has_command,
                ),
                (BlockButton::ExplainError, zero, false),
            ],
            len: 4,
        }
    };

    let label_pad = pad;
    let (meta, left) = match meta_w.filter(|&w| w > 0.0) {
        Some(w) => {
            let avail = (leftmost_btn - label_pad - min_left - label_pad).max(0.0);
            if avail >= (24.0 * scale) {
                let lw = w.ceil().min(avail);
                let mx = leftmost_btn - label_pad - lw;
                (Some(Rect::new(mx, y, lw, toolbar_h)), mx - label_pad)
            } else {
                (None, leftmost_btn - inner_pad)
            }
        }
        None => (None, leftmost_btn - inner_pad),
    };

    Some(CardToolbarLayout {
        rect: Rect::new(left, y, right - left, toolbar_h),
        meta,
        buttons,
    })
}

/// Hit-test `(x, y)` against the command block cards of one pane.
///
/// Returns `(block_id, button)` when `(x, y)` lies inside a visible block card; `button` is
/// `Some` when it hits an enabled icon button in that card's floating toolbar.
pub fn hit_test_cards(
    pane_rect: Rect,
    metrics: CellMetrics,
    rows: usize,
    blocks: &[BlockDecor],
    hidden_row: Option<usize>,
    (x, y): (f64, f64),
    language: UiLanguage,
) -> Option<(u64, Option<BlockButton>)> {
    // First find which card rectangle the pointer lies inside (so its toolbar is laid out).
    let base = layout_cards(pane_rect, metrics, rows, blocks, None, hidden_row, |_| 0.0);
    let hovered = base.iter().find(|c| c.card.contains(x, y)).map(|c| c.id);

    // Re-run with `hovered` active so its floating toolbar button rects are populated.
    let cards = if hovered.is_some() {
        layout_cards(
            pane_rect,
            metrics,
            rows,
            blocks,
            hovered,
            hidden_row,
            |_| 0.0,
        )
    } else {
        base
    };

    for card in &cards {
        if let Some(tb) = card.toolbar {
            if let Some(button) = tb.with_language(language, card.card).hit_button(x, y) {
                return Some((card.id, Some(button)));
            }
        }
    }
    hovered.map(|id| (id, None))
}
