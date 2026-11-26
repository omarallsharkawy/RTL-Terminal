//! Reuse shaped chrome and palette labels while their fonts, size and available width agree.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{Buffer, FontSystem};

use super::ui::{fit_query_tail, UiFont, UiSize, UiWeight};

const MAX_LABELS: usize = 512;
const MAX_TEXT_BYTES: usize = 1024;

/// Owned alongside one UI font and font system. Replace it when the font changes.
#[derive(Default)]
pub struct LabelCache {
    by_style: HashMap<Style, HashMap<String, Arc<Buffer>>>,
    count: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Style {
    size: UiSize,
    weight: UiWeight,
    scale: u32,
    fit: Fit,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Fit {
    Unbounded,
    End(u32),
    Words(u32),
    Tail(u32),
}

/// Borrow the font and optionally its persistent cache. Window-free callers can pass a font
/// directly for one-off frames.
pub struct UiLabels<'a> {
    font: &'a UiFont,
    cache: Option<&'a mut LabelCache>,
}

impl<'a> From<&'a UiFont> for UiLabels<'a> {
    fn from(font: &'a UiFont) -> Self {
        Self { font, cache: None }
    }
}

impl<'a> UiLabels<'a> {
    pub fn cached(font: &'a UiFont, cache: &'a mut LabelCache) -> Self {
        Self {
            font,
            cache: Some(cache),
        }
    }

    pub(crate) fn shape(
        &mut self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
    ) -> Arc<Buffer> {
        self.get(fonts, size, weight, scale, text, Fit::Unbounded)
    }

    pub(crate) fn fit(
        &mut self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
        width: f32,
    ) -> Arc<Buffer> {
        self.get(fonts, size, weight, scale, text, Fit::End(width.to_bits()))
    }

    pub(crate) fn fit_words(
        &mut self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
        width: f32,
    ) -> Arc<Buffer> {
        self.get(
            fonts,
            size,
            weight,
            scale,
            text,
            Fit::Words(width.to_bits()),
        )
    }

    pub(crate) fn fit_tail(
        &mut self,
        fonts: &mut FontSystem,
        scale: f32,
        text: &str,
        width: f32,
    ) -> Arc<Buffer> {
        self.get(
            fonts,
            UiSize::Title,
            UiWeight::Regular,
            scale,
            text,
            Fit::Tail(width.to_bits()),
        )
    }

    fn get(
        &mut self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
        fit: Fit,
    ) -> Arc<Buffer> {
        let font = self.font;
        let mut build = || {
            Arc::new(match fit {
                Fit::Unbounded => font.shape(fonts, size, weight, scale, text),
                Fit::End(width) => {
                    font.fit(fonts, size, weight, scale, text, f32::from_bits(width))
                }
                Fit::Words(width) => {
                    font.fit_words(fonts, size, weight, scale, text, f32::from_bits(width))
                }
                Fit::Tail(width) => {
                    fit_query_tail(font, fonts, scale, text, f32::from_bits(width)).1
                }
            })
        };
        let Some(cache) = self
            .cache
            .as_deref_mut()
            .filter(|_| text.len() <= MAX_TEXT_BYTES)
        else {
            return build();
        };
        let style = Style {
            size,
            weight,
            scale: scale.to_bits(),
            fit,
        };
        if let Some(buffer) = cache.by_style.get(&style).and_then(|texts| texts.get(text)) {
            return Arc::clone(buffer);
        }
        if cache.count == MAX_LABELS {
            cache.by_style.clear();
            cache.count = 0;
        }
        let buffer = build();
        cache
            .by_style
            .entry(style)
            .or_default()
            .insert(text.to_owned(), Arc::clone(&buffer));
        cache.count += 1;
        buffer
    }
}
