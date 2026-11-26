//! Code review panel chrome and terminal-shaped diff rows.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use glyphon::TextBounds;
use unicode_bidi::{bidi_class, BidiClass};
use unicode_width::UnicodeWidthChar;

use super::context::DrawContext;
use crate::color::Rgba;
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::metrics::CellMetrics;
use crate::render::row::CachedRow;
use crate::render::shapes::{Border, Shape};
use crate::render::style::Quad;
use crate::render::text::shaper::shape_label;
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{UiStyle, RADIUS_6};
use crate::review::diff::FileStatus;
use crate::review::layout::PanelLayout;
use crate::review::panel::{RowStyle, View};
use crate::terminal::{CellData, LineData, MAX_DRAWN_MARKS};
use crate::ui::chrome::UiLanguage;

/// A shaped label of the panel and where it goes.
pub struct ReviewText {
    pub row: Rc<CachedRow>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
}

/// The panel's diff quads, chrome shapes and text runs.
pub struct ReviewScene {
    pub quads: Vec<Quad>,
    pub shapes: Vec<Shape>,
    pub labels: Vec<UiLabel>,
    pub texts: Vec<ReviewText>,
}

type Key = (String, [u8; 3]);

/// Shaped labels of the previous frame, by text and color.
#[derive(Default)]
pub struct ReviewCache {
    font: Option<(CellMetrics, Option<&'static str>)>,
    rows: HashMap<Key, (Rc<CachedRow>, bool)>,
}

impl ReviewCache {
    fn begin_frame(&mut self, ctx: &DrawContext) {
        let font = Some((ctx.metrics, ctx.family));
        if self.font != font {
            self.rows.clear();
            self.font = font;
        }
        for (_, used) in self.rows.values_mut() {
            *used = false;
        }
    }

    fn get(&mut self, ctx: &mut DrawContext, text: &str, color: Rgba) -> Rc<CachedRow> {
        let rgb = rgb8(color);
        let key = (text.to_owned(), [rgb.r, rgb.g, rgb.b]);
        let (row, used) = self.rows.entry(key).or_insert_with(|| {
            let line = text_line(text, rgb);
            let segments = ctx.shaper().build_row(&line, None);
            let quads = ctx.row_quads(&line, &segments);
            let row = CachedRow::new(line.hash, &line, segments, quads, ctx.metrics.char_width);
            (Rc::new(row), true)
        });
        *used = true;
        row.clone()
    }

    fn end_frame(&mut self) {
        self.rows.retain(|_, (_, used)| *used);
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    /// Labels kept for the next frame.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn rgb8(c: Rgba) -> Rgb {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: to8(c.r),
        g: to8(c.g),
        b: to8(c.b),
    }
}

/// `text` as one row of cells in color `fg`: combining marks on their base cell, wide
/// characters on two cells. Control characters must already be replaced (`display_text`).
pub fn text_line(text: &str, fg: Rgb) -> LineData {
    let fg = AnsiColor::Spec(fg);
    let mut cells: Vec<CellData> = Vec::with_capacity(text.len());
    let mut has_rtl = false;
    for c in text.chars() {
        if matches!(bidi_class(c), BidiClass::R | BidiClass::AL) {
            has_rtl = true;
        }
        match c.width() {
            Some(0) | None => {
                if let Some(base) = cells.iter_mut().rev().find(|c| !c.is_spacer()) {
                    if base.zerowidth.len() < MAX_DRAWN_MARKS {
                        base.zerowidth.push(c);
                    }
                }
            }
            Some(2) => {
                cells.push(CellData {
                    c,
                    fg,
                    flags: CellFlags::WIDE_CHAR,
                    ..CellData::blank()
                });
                cells.push(CellData {
                    flags: CellFlags::WIDE_CHAR_SPACER,
                    ..CellData::blank()
                });
            }
            Some(_) => cells.push(CellData {
                c,
                fg,
                ..CellData::blank()
            }),
        }
    }
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (fg_bytes(&fg)).hash(&mut hasher);
    LineData {
        cells: cells.into(),
        has_rtl,
        hash: hasher.finish(),
    }
}

fn fg_bytes(fg: &AnsiColor) -> [u8; 3] {
    match fg {
        AnsiColor::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
        _ => [0; 3],
    }
}

pub fn build(
    ctx: &mut DrawContext,
    view: &View,
    cache: &mut ReviewCache,
    style: UiStyle,
    font: &UiFont,
    language: UiLanguage,
) -> ReviewScene {
    cache.begin_frame(ctx);
    let m = ctx.metrics;
    let layout = PanelLayout::new(
        view.rect,
        m.line_height,
        m.char_width,
        m.scale,
        view.notice.is_some(),
        view.commit_label.chars().count(),
    );
    let mut draw = ReviewDrawing {
        ctx,
        cache,
        style,
        font,
        layout,
        language,
        scene: ReviewScene {
            quads: Vec::new(),
            shapes: Vec::new(),
            labels: Vec::new(),
            texts: Vec::new(),
        },
    };
    draw.header(view);
    for (i, row) in view.rows.iter().take(layout.page_rows()).enumerate() {
        draw.row(i, row, view.gutter_cols);
    }
    if let (Some(area), Some(notice)) = (layout.footer, &view.notice) {
        draw.quad(area, style.bg2);
        let mut text = area;
        text.x += layout.pad;
        text.w = (text.w - 2.0 * layout.pad).max(0.0);
        draw.label(
            text,
            &notice.text,
            UiSize::Caption,
            UiWeight::Regular,
            UiAlign::Start,
            if notice.warn {
                style.warn
            } else {
                style.text_dim
            },
        );
    }
    draw.cache.end_frame();
    draw.scene
}

struct ReviewDrawing<'a, 'b> {
    ctx: &'a mut DrawContext<'b>,
    cache: &'a mut ReviewCache,
    style: UiStyle,
    font: &'a UiFont,
    layout: PanelLayout,
    language: UiLanguage,
    scene: ReviewScene,
}

impl ReviewDrawing<'_, '_> {
    fn header(&mut self, view: &View) {
        let s = self.style;
        let l = self.layout;
        let panel = l.panel;
        self.quad(panel, s.bg1);
        self.quad(l.header, s.bg2);
        self.quad(
            Rect::new(panel.x, panel.y, l.border.min(panel.w), panel.h),
            if view.focused { s.accent } else { s.border },
        );
        self.quad(
            Rect::new(
                l.header.x,
                l.header.bottom(),
                l.header.w,
                l.border.min((panel.bottom() - l.header.bottom()).max(0.0)),
            ),
            s.border,
        );
        let branch = view.branch.as_deref();
        let branch_w = branch.map_or(0.0, |branch| {
            let buffer = self.shape(branch, UiSize::Small, UiWeight::Semibold);
            label_width(&buffer).ceil() + s.px(27.0)
        });
        let repo_label = if view.totals.is_none() {
            self.language.text("Review changes", "راجع التغييرات")
        } else {
            &view.title
        };
        let repo_w =
            label_width(&self.shape(repo_label, UiSize::Title, UiWeight::Semibold)).ceil() + 2.0;
        let (repo, pill) = l.repository_and_branch(repo_w, branch_w);
        self.label(
            repo,
            repo_label,
            UiSize::Title,
            UiWeight::Semibold,
            UiAlign::Start,
            s.text,
        );
        if let Some(branch) = branch.filter(|_| pill.w > 0.0) {
            let pill = centered_height(pill, s.px(20.0));
            self.panel(pill, RADIUS_6, s.bg3, None);
            let icon = Rect::new(pill.x + s.px(4.0), pill.y, s.px(13.0).min(pill.w), pill.h);
            self.icon(icon, Icon::GitBranch, 12.0, s.text_dim);
            let text = Rect::new(
                icon.right() + s.px(3.0),
                pill.y,
                (pill.right() - icon.right() - s.px(7.0)).max(0.0),
                pill.h,
            );
            self.label(
                text,
                branch,
                UiSize::Small,
                UiWeight::Semibold,
                UiAlign::Start,
                s.text,
            );
        }

        let mut summary = l.summary;
        if let Some((added, removed)) = view.totals {
            let used = self.counts(summary, (added, removed), false);
            summary.x += used;
            summary.w = (summary.w - used).max(0.0);
        }
        let label = match view.summary.split_once(" / ") {
            Some((en, _)) if self.language == UiLanguage::English => en,
            Some((_, ar)) => ar,
            None => &view.summary,
        };
        self.label(
            summary,
            label,
            UiSize::Caption,
            UiWeight::Regular,
            UiAlign::Start,
            s.text_dim,
        );
        for (rect, icon) in [(l.refresh, Icon::Refresh), (l.close, Icon::Close)] {
            self.panel(rect, RADIUS_6, s.bg3, None);
            self.icon(rect, icon, 16.0, s.text);
        }
        self.panel(l.commit, RADIUS_6, s.accent, None);
        self.label(
            l.commit,
            self.language.text("Commit", "كوميت"),
            UiSize::Body,
            UiWeight::Semibold,
            UiAlign::Center,
            s.accent_text(),
        );
    }

    fn row(&mut self, i: usize, row: &crate::review::panel::ViewRow, gutter_cols: usize) {
        let s = self.style;
        let l = self.layout;
        let y = l.row_top(i);
        let h = l.line_height.min((l.body.bottom() - y).max(0.0));
        let full = Rect::new(l.body.x, y, l.body.w, h);
        if h <= 0.0 {
            return;
        }
        let inner = Rect::new(l.gutter_left(), y, (full.w - 2.0 * l.pad).max(0.0), h);
        let text_left = l.text_left(gutter_cols);
        let code = Rect::new(text_left, y, (inner.right() - text_left).max(0.0), h);
        match row.style {
            RowStyle::File { collapsed, status } => self.file(inner, row, collapsed, status),
            RowStyle::Hunk => {
                self.panel(inset(inner, 0.0, s.px(1.0)), RADIUS_6, s.bg2, None);
                self.label(
                    inset(inner, s.px(6.0), 0.0),
                    &row.text,
                    UiSize::Caption,
                    UiWeight::Regular,
                    UiAlign::Left,
                    s.text_dim,
                );
            }
            RowStyle::Context | RowStyle::Add | RowStyle::Del => {
                let tint = match row.style {
                    RowStyle::Add => Some(s.ok),
                    RowStyle::Del => Some(s.danger),
                    _ => None,
                };
                let gutter = Rect::new(
                    full.x,
                    y,
                    (text_left - s.px(3.0) - full.x).clamp(0.0, full.w),
                    h,
                );
                if let Some(color) = tint {
                    self.quad(full, s.bg1.mix(color, 0.16));
                }
                self.quad(gutter, tint.map_or(s.bg2, |color| s.bg2.mix(color, 0.19)));
                let numbers = Rect::new(
                    inner.x,
                    y,
                    (gutter.right().min(inner.right()) - inner.x).max(0.0),
                    h,
                );
                self.mono(numbers, &row.gutter, s.text_faint);
                self.mono(code, &row.text, s.text);
            }
            RowStyle::CommentInput => {
                let field = inset(code, 0.0, s.px(1.0));
                self.panel(field, RADIUS_6, s.bg2, Some(s.accent));
                self.label(
                    inset(field, s.px(6.0), 0.0),
                    &row.text,
                    UiSize::Caption,
                    UiWeight::Regular,
                    UiAlign::Start,
                    s.text,
                );
            }
            RowStyle::CommentHint => {
                self.label(
                    code,
                    &row.text,
                    UiSize::Small,
                    UiWeight::Regular,
                    UiAlign::Start,
                    s.text_dim,
                );
            }
            RowStyle::Message | RowStyle::Note => {
                self.label(
                    inner,
                    &row.text,
                    UiSize::Caption,
                    UiWeight::Regular,
                    UiAlign::Start,
                    s.text_dim,
                );
            }
        }
        if row.selected {
            self.panel(
                inset(full, s.px(2.0), 0.0),
                RADIUS_6,
                Rgba::new(0.0, 0.0, 0.0, 0.0),
                Some(s.accent),
            );
        }
    }

    fn file(
        &mut self,
        area: Rect,
        row: &crate::review::panel::ViewRow,
        collapsed: bool,
        status: FileStatus,
    ) {
        let s = self.style;
        self.panel(inset(area, 0.0, s.px(1.0)), RADIUS_6, s.bg3, None);
        let chevron = Rect::new(area.x + s.px(4.0), area.y, s.px(16.0), area.h);
        self.icon(
            chevron,
            if collapsed {
                Icon::ChevronRight
            } else {
                Icon::ChevronDown
            },
            14.0,
            s.text_dim,
        );
        let color = match status {
            FileStatus::Added => s.ok,
            FileStatus::Deleted => s.danger,
            FileStatus::Modified => s.warn,
            FileStatus::Renamed | FileStatus::Copied => s.accent,
        };
        let badge = centered_height(
            Rect::new(chevron.right() + s.px(4.0), area.y, s.px(19.0), area.h),
            s.px(16.0),
        );
        self.panel(badge, RADIUS_6, s.bg1.mix(color, 0.19), None);
        self.label(
            badge,
            &status.letter().to_string(),
            UiSize::Small,
            UiWeight::Semibold,
            UiAlign::Center,
            color.mix(s.text, 0.30),
        );

        let left = badge.right() + s.px(7.0);
        let content = Rect::new(
            left,
            area.y,
            (area.right() - left - s.px(6.0)).max(0.0),
            area.h,
        );
        let count_space = row
            .counts
            .map_or(0.0, |counts| self.counts(content, counts, true));
        let path = Rect::new(
            content.x,
            content.y,
            (content.w - count_space).max(0.0),
            content.h,
        );
        if path.w > 0.0 {
            let size = UiSize::Caption.px(s.scale);
            let buffer = shape_label(
                self.ctx.font_system,
                self.ctx.family,
                (size, (size * 1.5).ceil()),
                &row.text,
            );
            self.scene
                .labels
                .push(UiLabel::new(buffer, path, UiAlign::Start, s.text));
        }
    }

    fn counts(&mut self, rect: Rect, (added, removed): (usize, usize), right: bool) -> f32 {
        let s = self.style;
        let added = self.shape(&format!("+{added}"), UiSize::Small, UiWeight::Semibold);
        let removed = self.shape(&format!("−{removed}"), UiSize::Small, UiWeight::Semibold);
        let widths = [
            label_width(&added).ceil() + 1.0,
            label_width(&removed).ceil() + 1.0,
        ];
        let gap = s.px(6.0);
        let total = widths[0] + widths[1] + gap;
        if total + gap > rect.w {
            return 0.0;
        }
        let x = if right { rect.right() - total } else { rect.x };
        for (x, w, buffer, color) in [
            (x, widths[0], added, s.ok),
            (x + widths[0] + gap, widths[1], removed, s.danger),
        ] {
            let label = Rect::new(x, rect.y, w, rect.h);
            self.scene.labels.push(UiLabel::new(
                buffer,
                label,
                UiAlign::Left,
                color.mix(s.text, 0.22),
            ));
        }
        total + gap
    }

    fn shape(&mut self, text: &str, size: UiSize, weight: UiWeight) -> cosmic_text::Buffer {
        self.font
            .shape(self.ctx.font_system, size, weight, self.style.scale, text)
    }

    fn label(
        &mut self,
        rect: Rect,
        text: &str,
        size: UiSize,
        weight: UiWeight,
        align: UiAlign,
        color: Rgba,
    ) {
        if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
            return;
        }
        let buffer = self.font.fit(
            self.ctx.font_system,
            size,
            weight,
            self.style.scale,
            text,
            (rect.w - 1.0).max(0.0),
        );
        self.scene
            .labels
            .push(UiLabel::new(buffer, rect, align, color));
    }

    fn mono(&mut self, rect: Rect, text: &str, color: Rgba) {
        if rect.w <= 0.0 || rect.h <= 0.0 || text.is_empty() {
            return;
        }
        let row = self.cache.get(self.ctx, text, color);
        let bounds = TextBounds {
            left: rect.x.ceil() as i32,
            top: rect.y.ceil() as i32,
            right: rect.right().floor() as i32,
            bottom: rect.bottom().floor() as i32,
        };
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return;
        }
        for &(x, y, w, h, color) in &row.quads {
            let x = x + rect.x;
            let y = y + rect.y;
            let w = w.min(rect.right() - x);
            let h = h.min(rect.bottom() - y);
            if w > 0.0 && h > 0.0 {
                self.scene.quads.push((x, y, w, h, color));
            }
        }
        self.scene.texts.push(ReviewText {
            row,
            left: rect.x,
            top: rect.y,
            bounds,
        });
    }

    fn quad(&mut self, rect: Rect, color: Rgba) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.scene
                .quads
                .push((rect.x, rect.y, rect.w, rect.h, self.ctx.color(color)));
        }
    }

    fn panel(&mut self, rect: Rect, radius: f32, fill: Rgba, edge: Option<Rgba>) {
        if rect.w > 0.0 && rect.h > 0.0 {
            self.scene.shapes.push(Shape::RoundedRect {
                rect,
                radii: [self.style.px(radius); 4],
                fill: self.ctx.color(fill),
                border: edge.map(|color| Border {
                    width: self.style.hairline(),
                    color: self.ctx.color(color),
                }),
                shadow: None,
            });
        }
    }

    fn icon(&mut self, rect: Rect, icon: Icon, size: f32, color: Rgba) {
        let side = self.style.px(size).min(rect.w).min(rect.h);
        let rect = Rect::new(
            rect.x + (rect.w - side) / 2.0,
            rect.y + (rect.h - side) / 2.0,
            side,
            side,
        );
        icon.draw(rect, self.ctx.color(color), &mut self.scene.shapes);
    }
}

fn inset(rect: Rect, dx: f32, dy: f32) -> Rect {
    let dx = dx.min(rect.w / 2.0);
    let dy = dy.min(rect.h / 2.0);
    Rect::new(
        rect.x + dx,
        rect.y + dy,
        rect.w - 2.0 * dx,
        rect.h - 2.0 * dy,
    )
}

fn centered_height(rect: Rect, h: f32) -> Rect {
    let h = h.min(rect.h);
    Rect::new(rect.x, rect.y + (rect.h - h) / 2.0, rect.w, h)
}
