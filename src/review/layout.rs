//! Panel geometry: where the panel sits next to the panes, its header (title, summary, buttons),
//! body rows and footer, and what the pointer is over. Shared by drawing
//! (`render::scene::review`) and the app's hit testing so both always agree.

use super::rows::row_at;
use crate::layout::Rect;

/// Share of the window width the panel takes.
pub const PANEL_FRACTION: f32 = 0.45;
/// Narrowest panel, in logical pixels (when the window allows it).
pub const MIN_PANEL_PX: f32 = 360.0;
/// Width always left to the panes, in logical pixels.
pub const MIN_TERMINAL_PX: f32 = 160.0;
/// Below this width (logical pixels) the panel is not shown at all.
pub const MIN_USABLE_PX: f32 = 200.0;

/// Split the pane area `content` into the panes' part and the panel on its right: 45 % of the
/// window, at least 360 px, leaving the panes at least 160 px. `None` when there is no room.
pub fn split_content(content: Rect, window_width: f32, scale: f32) -> (Rect, Option<Rect>) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let want = (window_width * PANEL_FRACTION)
        .max(MIN_PANEL_PX * scale)
        .round();
    let room = (content.w - MIN_TERMINAL_PX * scale).floor();
    let width = want.min(room);
    if !width.is_finite() || width < MIN_USABLE_PX * scale {
        return (content, None);
    }
    (
        Rect::new(content.x, content.y, content.w - width, content.h),
        Some(Rect::new(
            content.right() - width,
            content.y,
            width,
            content.h,
        )),
    )
}

/// What lies under the pointer in the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelHit {
    Outside,
    /// Inside, on nothing clickable.
    Inside,
    Close,
    Refresh,
    Commit,
    /// Body row `index` (into all rows); `gutter` when on its line numbers.
    Row {
        index: usize,
        gutter: bool,
    },
}

/// The panel's parts, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelLayout {
    pub panel: Rect,
    /// Width of the line between the panes and the panel (and under the header).
    pub border: f32,
    /// Horizontal padding inside the panel.
    pub pad: f32,
    pub line_height: f32,
    pub char_width: f32,
    pub header: Rect,
    /// First header row's text area: repository and branch.
    pub title: Rect,
    /// Second header row's text area: file count and `+N −M`.
    pub summary: Rect,
    pub refresh: Rect,
    pub close: Rect,
    pub commit: Rect,
    /// Scrolling rows.
    pub body: Rect,
    /// One line for a notice under the rows, when there is one.
    pub footer: Option<Rect>,
}

impl PanelLayout {
    /// `commit_cols` is the commit button label's width in cells.
    pub fn new(
        panel: Rect,
        line_height: f32,
        char_width: f32,
        scale: f32,
        footer: bool,
        commit_cols: usize,
    ) -> Self {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let (lh, cw) = (line_height.max(1.0), char_width.max(1.0));
        let header_row = (26.0 * scale).max(lh);
        let border = scale.round().max(1.0);
        let pad = (8.0 * scale).round();
        let pad_v = (4.0 * scale).round();
        let left = panel.x + border;
        let inner_left = left + pad;
        let inner_right = (panel.right() - pad).max(inner_left);
        let header_h = (2.0 * header_row + 2.0 * pad_v).min(panel.h);
        let header = Rect::new(left, panel.y, (panel.w - border).max(0.0), header_h);
        let title_y = (panel.y + pad_v).min(header.bottom());
        let summary_y = (title_y + header_row).min(header.bottom());
        let title_h = header_row.min(header.bottom() - title_y);
        let summary_h = header_row.min(header.bottom() - summary_y);
        let gap = (4.0 * scale).min((inner_right - inner_left) / 8.0);
        let button = (24.0 * scale).min((inner_right - inner_left - gap).max(0.0) / 2.0);
        let button_h = title_h.min(24.0 * scale);
        let button_y = title_y + (title_h - button_h) / 2.0;
        let close = Rect::new(inner_right - button, button_y, button, button_h);
        let refresh = Rect::new(close.x - button - gap, button_y, button, button_h);
        let commit_w =
            ((commit_cols as f32 + 2.0) * 7.0 * scale).min((inner_right - inner_left) * 0.46);
        let commit_h = summary_h.min(24.0 * scale);
        let commit = Rect::new(
            inner_right - commit_w,
            summary_y + (summary_h - commit_h) / 2.0,
            commit_w,
            commit_h,
        );
        let title = Rect::new(
            inner_left,
            title_y,
            (refresh.x - pad - inner_left).max(0.0),
            title_h,
        );
        let summary = Rect::new(
            inner_left,
            summary_y,
            (commit.x - pad - inner_left).max(0.0),
            summary_h,
        );
        let body_top = (header.bottom() + border).min(panel.bottom());
        let footer_h = if footer { lh + 2.0 * pad_v } else { 0.0 };
        let body_h = (panel.bottom() - body_top - footer_h).max(0.0);
        let body = Rect::new(left, body_top, header.w, body_h);
        let footer = footer.then(|| {
            Rect::new(
                left,
                body.bottom(),
                header.w,
                (panel.bottom() - body.bottom()).max(0.0),
            )
        });
        Self {
            panel,
            border,
            pad,
            line_height: lh,
            char_width: cw,
            header,
            title,
            summary,
            refresh,
            close,
            commit,
            body,
            footer,
        }
    }

    pub fn repository_and_branch(&self, repo_width: f32, branch_width: f32) -> (Rect, Rect) {
        let gap = self.pad / 2.0;
        let branch_width = branch_width.min(self.title.w * 0.55);
        if branch_width <= self.pad * 2.0 {
            return (
                self.title,
                Rect::new(self.title.right(), self.title.y, 0.0, self.title.h),
            );
        }
        let repo_w = repo_width.min((self.title.w - branch_width - gap).max(0.0));
        (
            Rect::new(self.title.x, self.title.y, repo_w, self.title.h),
            Rect::new(
                self.title.x + repo_w + gap,
                self.title.y,
                branch_width,
                self.title.h,
            ),
        )
    }

    /// Whole rows the body shows (at least one).
    pub fn page_rows(&self) -> usize {
        ((self.body.h / self.line_height).floor() as usize).max(1)
    }

    /// Left edge of the line-number gutter.
    pub fn gutter_left(&self) -> f32 {
        self.body.x + self.pad
    }

    /// Width of the gutter of `gutter_cols` cells.
    pub fn gutter_width(&self, gutter_cols: usize) -> f32 {
        gutter_cols as f32 * self.char_width
    }

    /// Left edge of a row's text after a gutter of `gutter_cols` cells and one blank cell.
    pub fn text_left(&self, gutter_cols: usize) -> f32 {
        self.gutter_left() + (gutter_cols as f32 + 1.0) * self.char_width
    }

    /// Top of the `visible`-th visible body row.
    pub fn row_top(&self, visible: usize) -> f32 {
        self.body.y + visible as f32 * self.line_height
    }

    /// What `(x, y)` is over, with rows `top..len` scrolled into the body.
    pub fn hit(&self, x: f64, y: f64, top: usize, len: usize, gutter_cols: usize) -> PanelHit {
        if !self.panel.contains(x, y) {
            return PanelHit::Outside;
        }
        if self.close.contains(x, y) {
            return PanelHit::Close;
        }
        if self.refresh.contains(x, y) {
            return PanelHit::Refresh;
        }
        if self.commit.contains(x, y) {
            return PanelHit::Commit;
        }
        if self.body.contains(x, y) {
            let shown = top + self.page_rows();
            if let Some(index) = row_at(top, len.min(shown), self.body.y, self.line_height, y) {
                let gutter_end = self.gutter_left() + self.gutter_width(gutter_cols);
                return PanelHit::Row {
                    index,
                    gutter: x < gutter_end as f64,
                };
            }
        }
        PanelHit::Inside
    }
}
