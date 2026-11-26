//! Proportional, single-line labels for the chrome. The terminal and its row shaper keep
//! their existing font configuration.

use cosmic_text::{fontdb, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight, Wrap};
use glyphon::TextBounds;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

use super::fit::fit_text;
use super::fonts::{installed_family, is_arabic_script};
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiSize {
    Small,
    Caption,
    Body,
    Title,
}

impl UiSize {
    pub fn px(self, scale: f32) -> f32 {
        let logical = match self {
            Self::Small => 11.0,
            Self::Caption => 12.0,
            Self::Body => 13.0,
            Self::Title => 15.0,
        };
        logical * scale
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UiWeight {
    #[default]
    Regular,
    Semibold,
}

const LATIN_UI: &str = "Twitty Latin UI";
const ARABIC_UI: &str = "Twitty Arabic UI";
const UI_FONTS: &[&[u8]] = &[
    include_bytes!("../../../../assets/fonts/TwittyLatinUI-Regular.ttf"),
    include_bytes!("../../../../assets/fonts/TwittyLatinUI-SemiBold.ttf"),
    include_bytes!("../../../../assets/fonts/TwittyArabicUI-Regular.ttf"),
    include_bytes!("../../../../assets/fonts/TwittyArabicUI-SemiBold.ttf"),
];

fn load_ui_fonts(fonts: &mut FontSystem) {
    if fonts
        .db()
        .query(&fontdb::Query {
            families: &[Family::Name(LATIN_UI)],
            ..Default::default()
        })
        .is_none()
    {
        let db = fonts.db_mut();
        for &font in UI_FONTS {
            db.load_font_source(fontdb::Source::Binary(Arc::new(font)));
        }
    }
}

#[derive(Default)]
pub struct UiFont {
    family: Option<String>,
    covers_arabic: bool,
}

impl UiFont {
    /// Load the UI faces and resolve a configured override once at window startup.
    pub fn new(fonts: &mut FontSystem, configured: Option<&str>) -> Self {
        load_ui_fonts(fonts);
        let configured = configured.map(str::trim).filter(|s| !s.is_empty());
        let family = configured.and_then(|name| {
            let installed = installed_family(fonts, name);
            if installed.is_none() {
                twitty_core::warnings::warn(format!(
                    "twitty: ui_font_family {name:?} is not installed, using bundled UI fonts"
                ));
            }
            installed
        });
        let mut ui = Self {
            family: Some(family.unwrap_or_else(|| LATIN_UI.to_owned())),
            covers_arabic: false,
        };
        let query = fontdb::Query {
            families: &[ui.family()],
            ..Default::default()
        };
        ui.covers_arabic = fonts.db().query(&query).is_some_and(|id| {
            fonts
                .get_font(id, Weight::NORMAL)
                .is_some_and(|font| font.as_swash().charmap().map('ب') != 0)
        });
        ui
    }

    fn family(&self) -> Family<'_> {
        self.family
            .as_deref()
            .map_or(Family::Name(LATIN_UI), Family::Name)
    }

    /// Shape one line at the selected size. Arabic ranges use the bundled Arabic sans if the
    /// primary face does not cover Arabic. Bidi resolution still sees the entire line.
    pub fn shape(
        &self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
    ) -> Buffer {
        if self.family.is_none() {
            load_ui_fonts(fonts);
        }
        let mut text: String = text
            .chars()
            .filter(|&c| !twitty_core::terminal::is_bidi_control(c))
            .collect();
        text = text.replace(['\r', '\n', '\u{2028}', '\u{2029}'], " ");
        let px = size.px(scale);
        let mut buffer = Buffer::new_empty(Metrics::new(px, (px * 1.5).ceil()));
        buffer.set_wrap(Wrap::None);
        let attrs = Attrs::new().family(self.family()).weight(match weight {
            UiWeight::Regular => Weight::NORMAL,
            UiWeight::Semibold => Weight::SEMIBOLD,
        });
        if !self.covers_arabic && text.chars().any(is_arabic_script) {
            let arabic = attrs.clone().family(Family::Name(ARABIC_UI));
            let mut spans = Vec::new();
            let mut start = 0;
            let mut rtl = false;
            for (i, grapheme) in text.grapheme_indices(true) {
                // Keep neutral punctuation with the preceding Arabic span. In particular an
                // ellipsis must share its shaping run to land at the visual end of an RTL title.
                let is_arabic = grapheme.chars().any(is_arabic_script)
                    || (rtl && grapheme.chars().all(|c| !c.is_alphanumeric()));
                if is_arabic != rtl {
                    if i > start {
                        spans.push((
                            &text[start..i],
                            if rtl { arabic.clone() } else { attrs.clone() },
                        ));
                    }
                    rtl = is_arabic;
                    start = i;
                }
            }
            spans.push((&text[start..], if rtl { arabic } else { attrs.clone() }));
            buffer.set_rich_text(spans, &attrs, Shaping::Advanced, None);
        } else {
            buffer.set_text(&text, &attrs, Shaping::Advanced, None);
        }
        buffer.shape_until_scroll(fonts, false);
        buffer
    }

    #[doc(hidden)]
    pub fn fit_words(
        &self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
        width: f32,
    ) -> Buffer {
        let mut shape = |text: &str| self.shape(fonts, size, weight, scale, text);
        let full = shape(text);
        if label_width(&full) <= width {
            return full;
        }
        let fitted = fit_text(text, |candidate| {
            candidate != text && label_width(&shape(candidate)) <= width
        });
        shape(&fitted)
    }

    /// Shape and shorten by whole graphemes to fit available physical pixels.
    pub fn fit(
        &self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
        width: f32,
    ) -> Buffer {
        let mut shape = |text: &str| self.shape(fonts, size, weight, scale, text);
        let full = shape(text);
        if label_width(&full) <= width {
            return full;
        }
        let cuts: Vec<_> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        let mut best = shape("…");
        if label_width(&best) > width {
            return shape("");
        }
        let (mut low, mut high) = (0, cuts.len().saturating_sub(1));
        while low < high {
            let keep = (low + high).div_ceil(2);
            let candidate = shape(&format!("{}…", text[..cuts[keep]].trim_end()));
            if label_width(&candidate) <= width {
                best = candidate;
                low = keep;
            } else {
                high = keep - 1;
            }
        }
        best
    }
}

/// Fit `query` into `width` physical pixels, keeping the tail prefixed with `…` when it overflows
/// so what the user is typing stays visible.
pub fn fit_query_tail(
    font: &UiFont,
    font_system: &mut FontSystem,
    scale: f32,
    query: &str,
    width: f32,
) -> (String, Buffer) {
    let mut shape =
        |text: &str| font.shape(font_system, UiSize::Title, UiWeight::Regular, scale, text);
    let full = shape(query);
    if label_width(&full) <= width {
        return (query.to_owned(), full);
    }
    let cuts: Vec<_> = query.grapheme_indices(true).map(|(i, _)| i).collect();
    let ellipsis = shape("…");
    if label_width(&ellipsis) > width {
        return (String::new(), shape(""));
    }
    let mut best_text = "…".to_owned();
    let mut best = ellipsis;
    let (mut low, mut high) = (0, cuts.len().saturating_sub(1));
    while low < high {
        let keep = (low + high).div_ceil(2);
        let start = cuts[cuts.len() - keep];
        let text = format!("…{}", &query[start..]);
        let candidate = shape(&text);
        if label_width(&candidate) <= width {
            best_text = text;
            best = candidate;
            low = keep;
        } else {
            high = keep - 1;
        }
    }
    (best_text, best)
}

pub fn label_width(buffer: &Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}

#[derive(Clone, Copy)]
pub enum UiAlign {
    Start,
    Left,
    Right,
    Center,
}

/// A shaped label aligned within a physical rectangle, clipped to that rectangle.
pub struct UiLabel {
    pub buffer: Arc<Buffer>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}

impl UiLabel {
    pub fn new(buffer: impl Into<Arc<Buffer>>, rect: Rect, align: UiAlign, color: Rgba) -> Self {
        let buffer = buffer.into();
        let room = (rect.w - label_width(&buffer)).max(0.0);
        let rtl = buffer.layout_runs().next().is_some_and(|run| run.rtl);
        let x = match align {
            UiAlign::Start if rtl => room,
            UiAlign::Start | UiAlign::Left => 0.0,
            UiAlign::Right => room,
            UiAlign::Center => room / 2.0,
        };
        let y = (rect.h - buffer.metrics().line_height) / 2.0;
        let left = rect.x.ceil() as i32;
        let top = rect.y.ceil() as i32;
        Self {
            buffer,
            left: (rect.x + x).round(),
            top: (rect.y + y).round(),
            bounds: TextBounds {
                left,
                top,
                right: (rect.right().floor() as i32).max(left),
                bottom: (rect.bottom().floor() as i32).max(top),
            },
            color: color.to_glyphon(),
        }
    }
}
