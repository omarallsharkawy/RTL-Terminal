//! Reuse shaped chrome and palette labels while their fonts, size and available width agree.

use std::collections::HashMap;
use std::sync::Arc;

use cosmic_text::{Buffer, FontSystem};

use super::ui::{fit_query_tail, UiFont, UiSize, UiWeight};

const MAX_LABELS: usize = 1024;
const MAX_TEXT_BYTES: usize = 1024;

/// Owned alongside one UI font and font system. Replace it when the font changes.
#[derive(Default)]
pub struct LabelCache {
    by_style: HashMap<Style, HashMap<String, (Arc<Buffer>, u64)>>,
    count: usize,
    tick: u64,
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
    /// Width quantized to quarter-pixels so tiny float noise does not cause cache misses.
    End(u32),
    Words(u32),
    Tail(u32),
}

fn quantize_width(width: f32) -> u32 {
    if width.is_finite() {
        (width.max(0.0) * 4.0).round().min(u32::MAX as f32) as u32
    } else {
        0
    }
}

fn dequantize_width(q: u32) -> f32 {
    q as f32 * 0.25
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

    /// Borrow the underlying [`UiFont`] (e.g. for helpers that shape directly).
    pub fn font(&self) -> &'a UiFont {
        self.font
    }

    #[doc(hidden)]
    pub fn shape(
        &mut self,
        fonts: &mut FontSystem,
        size: UiSize,
        weight: UiWeight,
        scale: f32,
        text: &str,
    ) -> Arc<Buffer> {
        self.get(fonts, size, weight, scale, text, Fit::Unbounded)
    }

    #[doc(hidden)]
    pub fn fit(
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
            Fit::End(quantize_width(width)),
        )
    }

    #[doc(hidden)]
    pub fn fit_words(
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
            Fit::Words(quantize_width(width)),
        )
    }

    #[doc(hidden)]
    pub fn fit_tail(
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
            Fit::Tail(quantize_width(width)),
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
                    font.fit(fonts, size, weight, scale, text, dequantize_width(width))
                }
                Fit::Words(width) => {
                    font.fit_words(fonts, size, weight, scale, text, dequantize_width(width))
                }
                Fit::Tail(width) => {
                    fit_query_tail(font, fonts, scale, text, dequantize_width(width)).1
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
            scale: quantize_width(scale),
            fit,
        };
        cache.tick = cache.tick.wrapping_add(1);
        let now = cache.tick;
        if let Some((buffer, last_used)) = cache
            .by_style
            .get_mut(&style)
            .and_then(|texts| texts.get_mut(text))
        {
            *last_used = now;
            return Arc::clone(buffer);
        }
        if cache.count >= MAX_LABELS {
            evict_oldest_half(cache);
        }
        let buffer = build();
        cache
            .by_style
            .entry(style)
            .or_default()
            .insert(text.to_owned(), (Arc::clone(&buffer), now));
        cache.count += 1;
        buffer
    }
}

fn evict_oldest_half(cache: &mut LabelCache) {
    let mut ages: Vec<u64> = cache
        .by_style
        .values()
        .flat_map(|texts| texts.values().map(|(_, used)| *used))
        .collect();
    if ages.is_empty() {
        cache.count = 0;
        return;
    }
    let mid = ages.len() / 2;
    let (_, &mut cutoff, _) = ages.select_nth_unstable(mid);
    let mut kept = 0usize;
    cache.by_style.retain(|_, texts| {
        texts.retain(|_, (_, used)| *used > cutoff);
        kept += texts.len();
        !texts.is_empty()
    });
    cache.count = kept;
}
