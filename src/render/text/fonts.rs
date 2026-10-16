//! Font setup: the embedded Arabic fallback and the configurable terminal font family.

use cosmic_text::{fontdb, Family, FontSystem};

/// Noto Naskh Arabic (SIL OFL 1.1, see `assets/fonts/OFL.txt`). Embedded so Arabic renders
/// on machines that have no Arabic font installed.
const NOTO_NASKH_ARABIC: &[u8] =
    include_bytes!("../../../assets/fonts/NotoNaskhArabic-Regular.ttf");

/// A `FontSystem` with the system fonts plus the embedded Arabic font. System fonts win
/// where they cover a script; the embedded one is only the safety net.
pub fn new_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system
        .db_mut()
        .load_font_data(NOTO_NASKH_ARABIC.to_vec());
    font_system
}

/// The installed spelling of a font family named `name` (case-insensitive), if any. Shaping
/// matches family names exactly, so the configured name must be replaced with this one.
pub fn installed_family(font_system: &FontSystem, name: &str) -> Option<String> {
    font_system.db().faces().find_map(|f| {
        f.families
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(n, _)| n.clone())
    })
}

/// The family used for terminal cells: the configured name, or the generic monospace.
pub fn grid_family(name: Option<&'static str>) -> Family<'static> {
    name.map_or(Family::Monospace, Family::Name)
}

/// Family Arabic letters are shaped with when the grid font has none: the font cosmic-text's
/// fallback would find, named up front. Fallback tries fonts one by one for every word, which made
/// an Arabic row ~50x slower to shape than a Latin one. `None` when the grid font covers Arabic
/// itself (then it is used, as before).
pub fn arabic_family(font_system: &mut FontSystem, grid: Option<&'static str>) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<Option<&'static str>, Option<&'static str>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(found) = cache.lock().ok().and_then(|c| c.get(&grid).copied()) {
        return found;
    }
    let covers_arabic = |font_system: &mut FontSystem, family: Family| {
        let query = fontdb::Query {
            families: &[family],
            ..Default::default()
        };
        let Some(id) = font_system.db().query(&query) else {
            return false;
        };
        font_system
            .get_font(id, fontdb::Weight::NORMAL)
            .is_some_and(|f| f.as_swash().charmap().map('\u{0628}') != 0)
    };
    let found = if covers_arabic(font_system, grid_family(grid)) {
        None
    } else {
        ["Noto Sans Arabic", "Noto Naskh Arabic"]
            .into_iter()
            .find(|name| covers_arabic(font_system, Family::Name(name)))
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(grid, found);
    }
    found
}

/// Letters of the Arabic script (Arabic, Supplement, Extended-A, presentation forms).
pub(crate) fn is_arabic_script(c: char) -> bool {
    matches!(c,
        '\u{0600}'..='\u{06FF}'
        | '\u{0750}'..='\u{077F}'
        | '\u{0870}'..='\u{08FF}'
        | '\u{FB50}'..='\u{FDFF}'
        | '\u{FE70}'..='\u{FEFF}')
}
