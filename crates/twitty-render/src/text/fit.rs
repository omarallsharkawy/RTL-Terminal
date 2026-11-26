//! Width fitting shared by workspace cards and window chrome.

use unicode_segmentation::UnicodeSegmentation;

/// Prefer a whole-word prefix. If the first word is too wide, keep whole graphemes instead.
pub fn fit_text(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    if fits(text) {
        return text.to_owned();
    }
    if !fits("…") {
        // At extreme widths a narrow first grapheme may still fit.
        return text
            .graphemes(true)
            .next()
            .filter(|grapheme| fits(grapheme))
            .unwrap_or("")
            .to_owned();
    }

    let word_cuts = text
        .match_indices(char::is_whitespace)
        .filter(|(i, _)| *i > 0 && !text[..*i].ends_with(char::is_whitespace))
        .map(|(i, _)| i);
    let grapheme_cuts = text.grapheme_indices(true).skip(1).map(|(i, _)| i);
    for cuts in [word_cuts.collect::<Vec<_>>(), grapheme_cuts.collect()] {
        let n = cuts.partition_point(|&cut| fits(&format!("{}…", text[..cut].trim_end())));
        if n > 0 {
            return format!("{}…", text[..cuts[n - 1]].trim_end());
        }
    }
    "…".to_owned()
}
