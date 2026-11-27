//! One frame on the CPU: every quad back to front and every text run, built from the panes
//! without touching the GPU, so the whole layout can be tested headless.

use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::Buffer;
use glyphon::TextBounds;

use super::blocks::{self, Badge, BlockPaint};
use super::chrome;
use super::context::DrawContext;
use super::cursor::{
    cursor_cell_width, cursor_left, cursor_quads, effective_cursor_shape,
    geometry_under_block_cursor,
};
use super::memory::{memory_overlay, MemoryView};
use super::motion::Slide;
use super::palette::{palette_overlay, PaletteIcons, PaletteView};
use super::preedit::{bounds_outside, ImeArea, PreeditState};
use super::settings::{settings_overlay, SettingsView};
use super::welcome::{welcome_overlay, WelcomeView};
use super::window_chrome::{self, ChromeView};
use crate::{PaneFrame, TabLabel};
use twitty_core::layout::{PaneId, Rect};
use twitty_render::metrics::CellMetrics;
use twitty_render::row::cache::RowCache;
use twitty_render::row::RowCursor;
use twitty_render::shapes::ShapeLayers;
use twitty_render::style::Quad;
use twitty_render::text::labels::UiLabels;
use twitty_render::text::ui::{UiFont, UiLabel};
use twitty_render::ui_style::UiStyle;
use twitty_ui::blocks::cards::BlockHover;
use twitty_ui::ui::chrome::UiLanguage;
use twitty_ui::ui::motion::ChromeMotion;

use twitty_core::row::hash_row;
pub use twitty_render::scene::TextRun;

/// Renderer state a frame depends on besides what it is drawn with.
pub struct FrameOptions<'a> {
    /// Whether the window has keyboard focus (unfocused cursors are hollow).
    pub window_focused: bool,
    /// Configured cursor style ("block", "beam"/"bar", "underline").
    pub cursor_style: &'a str,
    /// Blink phase: while false the focused cursor is not drawn.
    pub cursor_blink_on: bool,
    /// Surface size in physical pixels.
    pub surface: (f32, f32),
}

/// Everything a frame draws. Row text stays in the `RowCache` and the preedit text in its
/// `PreeditState`; `text_runs` collects them with the labels shaped here.
pub struct Frame {
    /// Back to front: tab bar, per pane the selected block's tint, its rows (backgrounds,
    /// geometry, decorations) and block marks and badges, then the preedit box,
    /// unfocused panes' cursors, the focused cursor (or preedit caret), status bars, dividers.
    pub quads: Vec<Quad>,
    pub shapes: ShapeLayers,
    chrome_labels: Vec<UiLabel>,
    review_labels: Vec<UiLabel>,
    agent_panel_labels: Vec<UiLabel>,
    sidebar_rect: Option<Rect>,
    review_rect: Option<Rect>,
    agent_panel_rect: Option<Rect>,
    pane_clips: Vec<(PaneId, TextBounds)>,
    /// Window clear color (the theme background) in surface space.
    pub background: [f32; 4],
    /// The candidate popup area, when it moved since it was last reported.
    pub ime_area: Option<ImeArea>,
    tab_titles: Vec<(Buffer, Rect, glyphon::Color)>,
    sidebar_labels: Vec<super::sidebar::Label>,
    /// Labels of the code review panel.
    review_texts: Vec<super::review::ReviewText>,
    /// Monospace terminal-shaped rows of the Agent side panel.
    agent_panel_texts: Vec<super::agent::AgentText>,
    /// Per pane: the status line text and its top y.
    statuses: Vec<Option<(Buffer, f32)>>,
    /// Per pane: the badges of its command blocks (legacy mode).
    badges: Vec<Vec<Badge>>,
    /// Floating block card toolbar / badge rectangles whose covered row text must be clipped.
    block_clips: Vec<(PaneId, Rect)>,
    /// Dim duration/exit labels inside block card toolbars.
    block_labels: Vec<UiLabel>,
    /// The preedit box (left, y, width), when one is visible.
    preedit_box: Option<(f32, f32, f32)>,
    /// Clip of the pane the preedit is drawn in.
    ime_bounds: Option<TextBounds>,
    metrics: CellMetrics,
    foreground: glyphon::Color,
    surface: (f32, f32),
    /// Command composer shaped lines, chrome labels, and clip rects (drawn below the palette).
    composer_texts: Vec<super::composer::ComposerText>,
    composer_labels: Vec<UiLabel>,
    composer_clips: Vec<Rect>,
    /// The text of every layer drawn over the panes, bottom to top, in the order the layers
    /// were added. All text is drawn after all shapes, so each layer first hides the text under
    /// its area, then adds its own.
    layers: Vec<TextLayer>,
}

/// One overlay's text, in [`Frame::layers`].
enum TextLayer {
    Review,
    AgentPanel,
    Composer,
    /// Labels over `holes`, which hide the text drawn before them.
    Plain {
        holes: Vec<Rect>,
        labels: Vec<UiLabel>,
    },
}

/// The parts of `bounds` outside `hole`: up to four strips (above, below, left, right).
pub fn bounds_around(bounds: TextBounds, hole: Rect) -> Vec<TextBounds> {
    let (hl, ht) = (hole.x.floor() as i32, hole.y.floor() as i32);
    let (hr, hb) = (hole.right().ceil() as i32, hole.bottom().ceil() as i32);
    if hr <= bounds.left || hl >= bounds.right || hb <= bounds.top || ht >= bounds.bottom {
        return vec![bounds];
    }
    let mid_top = bounds.top.max(ht);
    let mid_bottom = bounds.bottom.min(hb);
    [
        TextBounds {
            bottom: ht,
            ..bounds
        },
        TextBounds { top: hb, ..bounds },
        TextBounds {
            top: mid_top,
            bottom: mid_bottom,
            right: hl,
            ..bounds
        },
        TextBounds {
            top: mid_top,
            bottom: mid_bottom,
            left: hr,
            ..bounds
        },
    ]
    .into_iter()
    .filter(|b| {
        b.left.max(bounds.left) < b.right.min(bounds.right)
            && b.top.max(bounds.top) < b.bottom.min(bounds.bottom)
    })
    .map(|b| TextBounds {
        left: b.left.max(bounds.left),
        top: b.top.max(bounds.top),
        right: b.right.min(bounds.right),
        bottom: b.bottom.min(bounds.bottom),
    })
    .collect()
}

fn clip_runs(runs: Vec<TextRun<'_>>, hole: Rect) -> Vec<TextRun<'_>> {
    runs.into_iter()
        .flat_map(|run| {
            bounds_around(run.bounds, hole)
                .into_iter()
                .map(move |bounds| TextRun { bounds, ..run })
        })
        .collect()
}

fn rect_bounds(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x as i32,
        top: r.y as i32,
        right: r.right() as i32,
        bottom: r.bottom() as i32,
    }
}

/// Lay out the tab bar (with two or more tabs), every pane and the dividers between them.
/// Rows that changed are reshaped into `cache`; the preedit is shaped into `ime`.
/// Uses the legacy block markers when panes carry `blocks` (`block_cards = false`).
pub fn build_frame(
    ctx: &mut DrawContext,
    opts: &FrameOptions,
    cache: &mut RowCache,
    ime: &mut PreeditState,
    tabs: &[TabLabel],
    panes: &[PaneFrame],
    dividers: &[Rect],
) -> Frame {
    build_frame_base(ctx, opts, cache, ime, tabs, panes, dividers, true)
}

/// Like [`build_frame`], rendering command blocks as rounded SDF cards with floating hover
/// toolbars (`block_cards = true`).
#[allow(clippy::too_many_arguments)]
pub fn build_frame_with_cards(
    ctx: &mut DrawContext,
    opts: &FrameOptions,
    cache: &mut RowCache,
    ime: &mut PreeditState,
    tabs: &[TabLabel],
    panes: &[PaneFrame],
    dividers: &[Rect],
    style: UiStyle,
    font: &UiFont,
    block_hover: Option<BlockHover>,
    language: UiLanguage,
) -> Frame {
    let mut frame = build_frame_base(ctx, opts, cache, ime, tabs, panes, dividers, false);
    frame.add_block_cards(ctx, style, font, panes, block_hover, language);
    frame
}

#[allow(clippy::too_many_arguments)]
pub fn build_frame_base(
    ctx: &mut DrawContext,
    opts: &FrameOptions,
    cache: &mut RowCache,
    ime: &mut PreeditState,
    tabs: &[TabLabel],
    panes: &[PaneFrame],
    dividers: &[Rect],
    legacy_blocks: bool,
) -> Frame {
    let m = ctx.metrics;
    let (width, _) = opts.surface;
    let background = ctx.color(ctx.palette.background);
    let cursor_color = ctx.color(ctx.palette.cursor);
    let mut quads: Vec<Quad> = Vec::new();
    // The focused pane's cursor is separate: a preedit replaces it.
    let mut focused_cursor: Vec<Quad> = Vec::new();
    let mut other_cursors: Vec<Quad> = Vec::new();
    let mut ime_caret = None;
    let mut ime_bounds: Option<TextBounds> = None;
    let mut badges = Vec::with_capacity(panes.len());

    let tab_geometry = chrome::tab_geometry(ctx, tabs.len(), width);
    chrome::tab_bar_quads(ctx, tabs, &tab_geometry, width, &mut quads);

    for pane in panes {
        let rect = pane.rect;
        let (origin_x, origin_y) = (rect.x + m.padding_left, rect.y + m.padding_top);
        let mut previous = cache.take(pane.id);
        let mut rows = Vec::with_capacity(pane.lines.len());
        let shape = effective_cursor_shape(opts.cursor_style, pane.cursor.shape);
        let cursor_focused = opts.window_focused && pane.focused;
        if legacy_blocks {
            blocks::tint_quads(ctx, pane, &mut quads);
        }

        for (r, line) in pane.lines.iter().enumerate() {
            let y = origin_y + r as f32 * m.line_height;
            // The cursor row stays marked during the blink's off phase: shaping depends on it
            // (an Arabic line being typed keeps its LTR base), only the block recolor turns off.
            let row_cursor =
                (pane.cursor.is_visible && pane.cursor.row == r).then_some(RowCursor {
                    col: pane.cursor.col,
                    block: cursor_focused && opts.cursor_blink_on && shape == CursorShape::Block,
                });

            let hash = hash_row(line, row_cursor);
            let row = previous
                .take(r, hash)
                .unwrap_or_else(|| ctx.build_row(hash, line, row_cursor));
            rows.push(row);

            let cached = &rows[r];
            quads.extend(
                cached
                    .quads
                    .iter()
                    .map(|&(x, qy, w, h, c)| (origin_x + x, y + qy, w, h, c)),
            );

            if pane.focused && pane.cursor.row == r {
                let (x, ext) = cached.cursor_anchor(pane.cursor.col, m.char_width);
                ime_caret = Some((origin_x + x, y, ext, rect));
                ime_bounds = Some(rect_bounds(rect));
            }

            // Only the focused pane's cursor blinks.
            if let Some(rc) = row_cursor.filter(|_| !pane.focused || opts.cursor_blink_on) {
                let (x, extends_left) = cached.cursor_anchor(rc.col, m.char_width);
                let x = cursor_left(origin_x + x, extends_left, shape, &m);
                let out = if pane.focused {
                    &mut focused_cursor
                } else {
                    &mut other_cursors
                };
                let w = cursor_cell_width(line, rc.col, m.char_width);
                cursor_quads((x, y, w), shape, cursor_focused, &m, cursor_color, out);
                if let Some(cell) = line.cells.get(rc.col).filter(|_| rc.block) {
                    let rect = (x, y, w, m.line_height);
                    out.extend(geometry_under_block_cursor(
                        ctx.palette,
                        ctx.is_srgb,
                        cell,
                        rect,
                    ));
                }
            }
        }
        cache.put(pane.id, rows);
        if legacy_blocks {
            blocks::mark_quads(ctx, pane, &mut quads);
            let status_row = pane.status.as_ref().and(pane.lines.len().checked_sub(1));
            badges.push(blocks::badges(ctx, pane, status_row, &mut quads));
        } else {
            badges.push(Vec::new());
        }
    }

    let (preedit_box, ime_area) = ime.layout(ctx, ime_caret, &mut quads, &mut focused_cursor);
    // The cursor sits above cell backgrounds but below text (its glyph was recolored).
    quads.extend(other_cursors);
    quads.extend(focused_cursor);

    let mut statuses = Vec::with_capacity(panes.len());
    for pane in panes {
        let status = match &pane.status {
            Some(text) => chrome::status_line(ctx, pane.rect, pane.lines.len(), text),
            None => None,
        };
        statuses.push(status.map(|(quad, buffer, y)| {
            quads.push(quad);
            (buffer, y)
        }));
    }

    quads.extend(chrome::divider_quads(ctx, dividers));

    let tab_titles = chrome::tab_titles(ctx, tabs, &tab_geometry);

    Frame {
        quads,
        shapes: ShapeLayers::default(),
        chrome_labels: Vec::new(),
        review_labels: Vec::new(),
        agent_panel_labels: Vec::new(),
        sidebar_rect: None,
        review_rect: None,
        agent_panel_rect: None,
        pane_clips: Vec::new(),
        background,
        ime_area,
        tab_titles,
        sidebar_labels: Vec::new(),
        review_texts: Vec::new(),
        agent_panel_texts: Vec::new(),
        statuses,
        badges,
        block_clips: Vec::new(),
        block_labels: Vec::new(),
        preedit_box,
        ime_bounds,
        metrics: m,
        foreground: ctx.palette.foreground.to_glyphon(),
        surface: opts.surface,
        composer_texts: Vec::new(),
        composer_labels: Vec::new(),
        composer_clips: Vec::new(),
        layers: Vec::new(),
    }
}

impl Frame {
    /// Draw command block cards into `shapes.underlay` and floating toolbars into `shapes.chrome`.
    pub fn add_block_cards(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        panes: &[PaneFrame],
        block_hover: Option<BlockHover>,
        language: UiLanguage,
    ) {
        let paint = BlockPaint {
            hover: block_hover,
            fades: None,
        };
        self.add_block_paint(ctx, style, font, panes, &paint, language);
    }

    pub fn add_block_paint(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        panes: &[PaneFrame],
        paint: &BlockPaint<'_>,
        language: UiLanguage,
    ) {
        for pane in panes {
            let status_row = pane.status.as_ref().and(pane.lines.len().checked_sub(1));
            let scene = blocks::draw_cards(
                ctx,
                style,
                font,
                pane,
                status_row,
                paint,
                &mut self.shapes,
                language,
            );
            self.block_clips.extend(scene.clips);
            self.block_labels.extend(scene.labels);
        }
    }

    /// Draw the toolbar and pane headers. The `PaneFrame` rectangles already describe grids.
    pub fn add_chrome<'a>(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        view: &ChromeView,
    ) {
        self.add_chrome_motion(ctx, style, font.into(), view, None);
    }

    pub fn add_chrome_motion(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: UiLabels<'_>,
        view: &ChromeView,
        motion: Option<&ChromeMotion<'_>>,
    ) {
        self.chrome_labels =
            window_chrome::draw(ctx, style, font, view, &mut self.shapes.chrome, motion);
        self.pane_clips.extend(view.panes.iter().filter_map(|pane| {
            let edge = if pane.flash > 0.0 {
                style.px(2.0 + 2.0 * pane.flash.clamp(0.0, 1.0)).ceil()
            } else if pane.attention {
                style.hairline()
            } else {
                return None;
            };
            let grid = pane.layout.grid;
            let top_inset = (edge - pane.layout.header.h).max(0.0);
            let clip = Rect::new(
                grid.x + edge,
                grid.y + top_inset,
                (grid.w - 2.0 * edge).max(0.0),
                (grid.h - edge - top_inset).max(0.0),
            );
            Some((pane.id, rect_bounds(clip)))
        }));
    }

    /// Draw the docked command composer above terminal content and below overlays.
    pub fn add_composer(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        view: &super::composer::ComposerView,
    ) {
        self.add_composer_motion(ctx, style, font, view, 1.0, true);
    }

    pub fn add_composer_motion(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        view: &super::composer::ComposerView,
        opacity: f32,
        open: bool,
    ) {
        if opacity <= 0.0 {
            return;
        }
        let mut scene = super::composer::build(ctx, style, font, view);
        let slide = Slide::new(opacity, [0.0, style.px(10.0)]);
        if opacity < 1.0 {
            scene.shapes.iter_mut().for_each(|shape| slide.shape(shape));
            scene.labels.iter_mut().for_each(|label| slide.label(label));
            scene
                .clips
                .iter_mut()
                .for_each(|rect| *rect = slide.rect(*rect));
            for text in &mut scene.texts {
                slide.text_position(&mut text.left, &mut text.top, &mut text.bounds);
                for seg in &mut text.row.segments {
                    slide.buffer(&mut seg.buffer, ctx.font_system);
                }
            }
        }
        self.shapes.chrome.extend(scene.shapes);
        self.composer_texts = scene.texts;
        self.composer_labels = scene.labels;
        self.composer_clips = scene.clips;
        self.layers.push(TextLayer::Composer);
        if open && view.focused {
            if let Some((x, y, w, h)) = scene.ime_area {
                self.ime_area = Some((x + slide.x as i32, y + slide.y as i32, w, h));
            }
        }
    }

    /// Draw the failed-block AI explanation / fix popover above terminal content and below overlays.
    pub fn add_block_ai(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        view: &super::ai::BlockAiView,
    ) {
        let (rect, labels) = super::ai::block_ai_overlay(
            ctx,
            style,
            font,
            view,
            self.surface,
            &mut self.shapes.chrome,
        );
        self.push_layer(rect, labels);
    }

    /// Add a layer of `labels` that hides the text drawn before it under `hole`.
    fn push_layer(&mut self, hole: Option<Rect>, labels: Vec<UiLabel>) {
        self.layers.push(TextLayer::Plain {
            holes: hole.into_iter().collect(),
            labels,
        });
    }

    /// Draw an open right-click menu above everything else.
    pub fn add_context_menu(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        menu: &twitty_ui::ui::context_menu::ContextMenu,
        layout: &twitty_ui::ui::context_menu::MenuLayout,
        language: twitty_ui::ui::chrome::UiLanguage,
    ) {
        let labels = super::context_menu::draw(
            ctx,
            style,
            font,
            menu,
            layout,
            language,
            &mut self.shapes.overlay,
        );
        self.push_layer(Some(layout.rect), labels);
    }

    /// Draw short notices at the top center of the window.
    pub fn add_toasts(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        toasts: &[twitty_ui::ui::toast::Toast],
        language: twitty_ui::ui::chrome::UiLanguage,
    ) {
        let (labels, rects) = super::context_menu::draw_toasts(
            ctx,
            style,
            font,
            toasts,
            self.surface,
            language,
            &mut self.shapes.overlay,
        );
        self.layers.push(TextLayer::Plain {
            holes: rects,
            labels,
        });
    }

    /// Draw a hover tooltip near pointer `at`, above everything else.
    pub fn add_tooltip(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        text: &str,
        at: (f32, f32),
    ) {
        let (label, rect) = super::context_menu::draw_tooltip(
            ctx,
            style,
            font,
            text,
            at,
            self.surface,
            &mut self.shapes.overlay,
        );
        self.push_layer(Some(rect), vec![label]);
    }

    /// Draw the one-time AI privacy consent modal above everything else.
    pub fn add_ai_consent(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        view: &super::ai::ConsentOverlayView,
    ) {
        let (rect, labels) = super::ai::consent_overlay(
            ctx,
            style,
            font,
            view,
            self.surface,
            &mut self.shapes.overlay,
        );
        self.push_layer(rect, labels);
    }

    /// Draw the command palette above everything else.
    pub fn add_palette(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: &UiFont,
        view: &PaletteView,
    ) {
        self.add_palette_motion(ctx, style, font, view, &PaletteIcons::default(), 1.0);
    }

    /// Draw at a sampled transition opacity, including the translated text clip.
    pub fn add_palette_motion<'a>(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        view: &PaletteView,
        icons: &PaletteIcons,
        opacity: f32,
    ) {
        if opacity <= 0.0 {
            return;
        }
        let start = self.shapes.overlay.len();
        let (outer, mut texts) = palette_overlay(
            ctx,
            style,
            font.into(),
            view,
            icons,
            self.surface,
            &mut self.shapes.overlay,
        );
        let slide = Slide::new(opacity, [0.0, -style.px(10.0)]);
        if opacity < 1.0 {
            self.shapes.overlay[start..]
                .iter_mut()
                .for_each(|shape| slide.shape(shape));
            texts.iter_mut().for_each(|label| slide.label(label));
        }
        self.push_layer(outer.map(|rect| slide.rect(rect)), texts);
    }

    /// Draw the settings panel above everything else, clipping underlying text behind it.
    pub fn add_settings<'a>(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        view: &SettingsView,
    ) {
        let (outer, texts) = settings_overlay(
            ctx,
            style,
            font,
            view,
            self.surface,
            &mut self.shapes.overlay,
        );
        self.push_layer(outer, texts);
    }

    /// Draw the first-run welcome wizard above everything else, clipping underlying text behind it.
    pub fn add_welcome<'a>(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        view: &WelcomeView,
    ) {
        let (outer, texts) = welcome_overlay(
            ctx,
            style,
            font,
            view,
            self.surface,
            &mut self.shapes.overlay,
        );
        self.push_layer(outer, texts);
    }

    /// Draw the Project Memory overlay above everything else, clipping underlying text behind it.
    pub fn add_memory<'a>(
        &mut self,
        ctx: &mut DrawContext,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        view: &MemoryView,
    ) {
        let (outer, texts) = memory_overlay(
            ctx,
            style,
            font,
            view,
            self.surface,
            &mut self.shapes.overlay,
        );
        self.push_layer(outer, texts);
    }

    /// Optional chrome is attached explicitly so offscreen/golden callers keep their layout.
    pub fn add_sidebar(
        &mut self,
        ctx: &mut DrawContext,
        view: &twitty_ui::sidebar::cards::View,
        cache: &mut super::sidebar::LabelCache,
        style: UiStyle,
        font: &UiFont,
    ) {
        let scene = super::sidebar::build(ctx, view, cache, style, font);
        self.quads.extend(scene.quads);
        self.shapes.chrome.extend(scene.shapes);
        self.sidebar_rect = Some(view.area);
        self.sidebar_labels = scene.labels;
    }

    /// The code review panel, beside the panes (they are laid out to leave it room).
    pub fn add_review<'a>(
        &mut self,
        ctx: &mut DrawContext,
        view: &twitty_ui::review::panel::View,
        cache: &mut super::review::ReviewCache,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        language: UiLanguage,
    ) {
        self.add_review_motion(ctx, view, cache, style, font, language, 1.0);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_review_motion<'a>(
        &mut self,
        ctx: &mut DrawContext,
        view: &twitty_ui::review::panel::View,
        cache: &mut super::review::ReviewCache,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        language: UiLanguage,
        opacity: f32,
    ) {
        if opacity <= 0.0 {
            return;
        }
        let mut scene = super::review::build(ctx, view, cache, style, font, language);
        let slide = Slide::new(opacity, [style.px(14.0), 0.0]);
        if opacity < 1.0 {
            // Put the moving panel above the expanded panes and their chrome on exit.
            self.shapes
                .chrome
                .extend(scene.quads.into_iter().map(|(x, y, w, h, fill)| {
                    let mut shape =
                        twitty_render::shapes::Shape::rounded(Rect::new(x, y, w, h), 0.0, fill);
                    slide.shape(&mut shape);
                    shape
                }));
            scene.shapes.iter_mut().for_each(|shape| slide.shape(shape));
            scene.labels.iter_mut().for_each(|label| slide.label(label));
            for text in &mut scene.texts {
                slide.text_position(&mut text.left, &mut text.top, &mut text.bounds);
                for seg in &mut std::rc::Rc::make_mut(&mut text.row).segments {
                    slide.buffer(&mut seg.buffer, ctx.font_system);
                }
            }
        } else {
            self.quads.extend(scene.quads);
        }
        self.shapes.chrome.extend(scene.shapes);
        self.review_labels = scene.labels;
        self.review_rect = Some(slide.rect(view.rect));
        self.review_texts = scene.texts;
        self.layers.push(TextLayer::Review);
    }

    /// The Agent side panel, docked beside the panes (clipping underlying terminal text).
    pub fn add_agent_panel<'a>(
        &mut self,
        ctx: &mut DrawContext,
        view: &twitty_ui::ui::agent_panel::AgentPanelView,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
    ) {
        self.add_agent_panel_motion(ctx, view, style, font, 1.0);
    }

    pub fn add_agent_panel_motion<'a>(
        &mut self,
        ctx: &mut DrawContext,
        view: &twitty_ui::ui::agent_panel::AgentPanelView,
        style: UiStyle,
        font: impl Into<UiLabels<'a>>,
        opacity: f32,
    ) {
        if opacity <= 0.0 {
            return;
        }
        let mut scene = super::agent::build(ctx, view, style, font);
        let dir = if view.mirrored() { -1.0 } else { 1.0 };
        let slide = Slide::new(opacity, [dir * style.px(14.0), 0.0]);
        if opacity < 1.0 {
            self.shapes
                .chrome
                .extend(scene.quads.into_iter().map(|(x, y, w, h, fill)| {
                    let mut shape =
                        twitty_render::shapes::Shape::rounded(Rect::new(x, y, w, h), 0.0, fill);
                    slide.shape(&mut shape);
                    shape
                }));
            scene.shapes.iter_mut().for_each(|shape| slide.shape(shape));
            scene.labels.iter_mut().for_each(|label| slide.label(label));
            for text in &mut scene.texts {
                slide.text_position(&mut text.left, &mut text.top, &mut text.bounds);
                for seg in &mut std::rc::Rc::make_mut(&mut text.row).segments {
                    slide.buffer(&mut seg.buffer, ctx.font_system);
                }
            }
        } else {
            self.quads.extend(scene.quads);
        }
        self.shapes.chrome.extend(scene.shapes);
        self.agent_panel_labels = scene.labels;
        self.agent_panel_rect = Some(slide.rect(view.rect));
        self.agent_panel_texts = scene.texts;
        self.layers.push(TextLayer::AgentPanel);
    }

    pub fn add_attention(&mut self, ctx: &DrawContext, rects: &[Rect]) {
        self.quads
            .extend(super::sidebar::attention_quads(ctx, rects));
    }

    /// Every text run, in drawing order: tab titles, per-pane rows (the last one hidden under
    /// a status line) and status line, preedit, sidebar labels, the review panel, then the
    /// command palette.
    pub fn text_runs<'a>(
        &'a self,
        panes: &[PaneFrame],
        cache: &'a RowCache,
        ime: &'a PreeditState,
    ) -> Vec<TextRun<'a>> {
        let m = &self.metrics;
        let fg = self.foreground;
        let mut runs = Vec::new();
        for (buffer, rect, color) in &self.tab_titles {
            runs.push(TextRun {
                buffer,
                left: rect.x + m.char_width,
                top: rect.y,
                bounds: rect_bounds(*rect),
                color: *color,
            });
        }
        let pane_bounds = |pane: &PaneFrame| {
            self.pane_clips
                .iter()
                .find_map(|&(id, bounds)| (id == pane.id).then_some(bounds))
                .unwrap_or_else(|| rect_bounds(pane.rect))
        };
        for ((pane, status), badges) in panes.iter().zip(&self.statuses).zip(&self.badges) {
            let bounds = pane_bounds(pane);
            let card_holes: Vec<Rect> = self
                .block_clips
                .iter()
                .filter_map(|&(id, rect)| (id == pane.id).then_some(rect))
                .collect();
            let rows = pane.lines.len();
            for r in 0..rows {
                if status.is_some() && r + 1 == rows {
                    break;
                }
                let Some(cached_row) = cache.row(pane.id, r) else {
                    continue;
                };
                let y = pane.rect.y + m.padding_top + r as f32 * m.line_height;
                // The row under the preedit is clipped around it, so its text does not show
                // through the preedit's background.
                let badge = badges.iter().find(|b| b.row == r);
                let (parts, n) = match (self.preedit_box, badge) {
                    (Some((left, _, width)), _) if pane.focused && pane.cursor.row == r => {
                        (bounds_outside(bounds, left, left + width), 2)
                    }
                    // A block badge covers the end of its row.
                    (_, Some(b)) => (bounds_outside(bounds, b.left, b.right), 1),
                    _ => ([bounds, bounds], 1),
                };
                let row_bounds: Vec<TextBounds> = if card_holes.is_empty() {
                    parts[..n].to_vec()
                } else {
                    let row_top = y.floor() as i32;
                    let row_bottom = (y + m.line_height).ceil() as i32;
                    let mut clipped: Vec<TextBounds> = parts[..n]
                        .iter()
                        .map(|&b| TextBounds {
                            top: b.top.max(row_top),
                            bottom: b.bottom.min(row_bottom),
                            ..b
                        })
                        .filter(|b| b.left < b.right && b.top < b.bottom)
                        .collect();
                    for &hole in &card_holes {
                        if hole.bottom() <= y || hole.y >= y + m.line_height {
                            continue;
                        }
                        clipped = clipped
                            .into_iter()
                            .flat_map(|b| bounds_around(b, hole))
                            .collect();
                    }
                    clipped
                };
                for seg in &cached_row.segments {
                    for &bounds in &row_bounds {
                        runs.push(TextRun {
                            buffer: &seg.buffer,
                            left: pane.rect.x + m.padding_left + seg.seg_x,
                            top: y,
                            bounds,
                            color: fg,
                        });
                    }
                }
            }
            for badge in badges {
                runs.push(TextRun {
                    buffer: &badge.buffer,
                    left: badge.text_left,
                    top: badge.top,
                    bounds,
                    color: badge.color,
                });
            }
            if let Some((buffer, y)) = status {
                runs.push(TextRun {
                    buffer,
                    left: pane.rect.x + m.padding_left,
                    top: *y,
                    bounds,
                    color: fg,
                });
            }
        }
        if let Some((left, top, _)) = self.preedit_box {
            let (width, height) = self.surface;
            let bounds = panes
                .iter()
                .find(|pane| pane.focused)
                .map(pane_bounds)
                .or(self.ime_bounds)
                .unwrap_or_else(|| rect_bounds(Rect::new(0.0, 0.0, width, height)));
            if let Some(buffer) = ime.buffer() {
                let mut preedit_runs = vec![TextRun {
                    buffer,
                    left,
                    top,
                    bounds,
                    color: fg,
                }];
                if let Some(pane) = panes.iter().find(|pane| pane.focused) {
                    for &(_, hole) in self.block_clips.iter().filter(|(id, _)| *id == pane.id) {
                        preedit_runs = clip_runs(preedit_runs, hole);
                    }
                }
                runs.extend(preedit_runs);
            }
        }
        // Glyphon draws after these surfaces. Keep pane labels and preedit behind them.
        if let Some(hole) = self.sidebar_rect {
            runs = clip_runs(runs, hole);
        }
        for label in &self.sidebar_labels {
            runs.push(TextRun {
                buffer: &label.buffer,
                left: label.left,
                top: label.top,
                bounds: label.bounds,
                color: label.color,
            });
        }
        runs.extend(self.block_labels.iter().map(ui_run));
        runs.extend(self.chrome_labels.iter().map(ui_run));
        // Terminal-shaped rows of a panel, positioned like the panel drew them.
        macro_rules! push_rows {
            ($texts:expr) => {
                for text in $texts {
                    for seg in &text.row.segments {
                        runs.push(TextRun {
                            buffer: &seg.buffer,
                            left: text.left + seg.seg_x,
                            top: text.top,
                            bounds: text.bounds,
                            color: fg,
                        });
                    }
                }
            };
        }
        for layer in &self.layers {
            match layer {
                TextLayer::Review => {
                    if let Some(hole) = self.review_rect {
                        runs = clip_runs(runs, hole);
                    }
                    push_rows!(&self.review_texts);
                    runs.extend(self.review_labels.iter().map(ui_run));
                }
                TextLayer::AgentPanel => {
                    if let Some(hole) = self.agent_panel_rect {
                        runs = clip_runs(runs, hole);
                    }
                    push_rows!(&self.agent_panel_texts);
                    runs.extend(self.agent_panel_labels.iter().map(ui_run));
                }
                TextLayer::Composer => {
                    for &hole in &self.composer_clips {
                        runs = clip_runs(runs, hole);
                    }
                    push_rows!(&self.composer_texts);
                    runs.extend(self.composer_labels.iter().map(ui_run));
                }
                TextLayer::Plain { holes, labels } => {
                    for &hole in holes {
                        runs = clip_runs(runs, hole);
                    }
                    runs.extend(labels.iter().map(ui_run));
                }
            }
        }
        runs
    }
}

fn ui_run(text: &UiLabel) -> TextRun<'_> {
    TextRun {
        buffer: &text.buffer,
        left: text.left,
        top: text.top,
        bounds: text.bounds,
        color: text.color,
    }
}
