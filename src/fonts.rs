//! Font setup: the embedded Arabic fallback and the configurable terminal font family.

use cosmic_text::{Family, FontSystem};

/// Noto Naskh Arabic (SIL OFL 1.1, see `assets/fonts/OFL.txt`). Embedded so Arabic renders
/// on machines that have no Arabic font installed.
const NOTO_NASKH_ARABIC: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");

/// A `FontSystem` with the system fonts plus the embedded Arabic font. System fonts win
/// where they cover a script; the embedded one is only the safety net.
pub fn new_font_system() -> FontSystem {
    let mut font_system = FontSystem::new();
    font_system
        .db_mut()
        .load_font_data(NOTO_NASKH_ARABIC.to_vec());
    font_system
}

/// Whether a font family with this name (case-insensitive) is installed.
pub fn family_installed(font_system: &FontSystem, name: &str) -> bool {
    font_system
        .db()
        .faces()
        .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
}

/// The family used for terminal cells: the configured name, or the generic monospace.
pub fn grid_family(name: Option<&'static str>) -> Family<'static> {
    name.map_or(Family::Monospace, Family::Name)
}
