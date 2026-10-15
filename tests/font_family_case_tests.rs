//! The configured font family is matched case-insensitively, then used with the installed
//! spelling: cosmic-text compares family names exactly, so "jetbrains mono" would never match.
use twitty::render::text::fonts::{installed_family, new_font_system};

#[test]
fn family_name_resolves_to_the_installed_spelling() {
    let fs = new_font_system();
    assert_eq!(
        installed_family(&fs, "noto naskh ARABIC").as_deref(),
        Some("Noto Naskh Arabic")
    );
    assert_eq!(
        installed_family(&fs, "Noto Naskh Arabic").as_deref(),
        Some("Noto Naskh Arabic")
    );
    assert_eq!(installed_family(&fs, "No Such Font 123"), None);
}
