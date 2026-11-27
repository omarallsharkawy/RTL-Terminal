//! The translation catalogs (`crates/twitty-core/i18n/*.json`) against the code: every English
//! interface text the code shows has a translation, nothing in a catalog is left unused, and
//! placeholders match.
//!
//! Interface text is found where it is marked: `t("…")`, `.t("…")` and `en!("…")`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use twitty::ui::chrome::UiLanguage;

fn sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut dirs = vec![root.join("src")];
    for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
        dirs.push(crate_dir.unwrap().path().join("src"));
    }
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// The value of the Rust string literal starting at `src[0] == '"'`, and its length in bytes.
fn literal(src: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut chars = src.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, i + 1)),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '\n' => {
                    while chars.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                        chars.next();
                    }
                }
                'u' => {
                    let mut hex = String::new();
                    for (_, c) in chars.by_ref() {
                        match c {
                            '{' => {}
                            '}' => break,
                            c => hex.push(c),
                        }
                    }
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                'x' => {
                    let hi = chars.next()?.1;
                    let lo = chars.next()?.1;
                    out.push(u8::from_str_radix(&format!("{hi}{lo}"), 16).ok()? as char);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// Every marked English text in the code, with where it was first seen.
fn used_texts() -> BTreeMap<String, String> {
    let mut used = BTreeMap::new();
    for file in sources() {
        let src = std::fs::read_to_string(&file).unwrap();
        for marker in ["t(", "en!("] {
            let mut from = 0;
            while let Some(at) = src[from..].find(marker) {
                let start = from + at;
                from = start + marker.len();
                // `t(` must be the whole name (`t(`, `.t(`, `language::t(`), not `get(`.
                let before = src[..start].chars().next_back();
                if marker == "t(" && before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = src[from..].trim_start();
                if !rest.starts_with('"') {
                    continue;
                }
                let (text, _) = literal(rest).unwrap_or_else(|| {
                    panic!("{}: unreadable literal after {marker}", file.display())
                });
                let line = src[..start].lines().count();
                used.entry(text)
                    .or_insert_with(|| format!("{}:{line}", file.display()));
            }
        }
    }
    used
}

fn catalog() -> BTreeMap<String, String> {
    serde_json::from_str(twitty_core::language::ARABIC_CATALOG).expect("ar.json is valid JSON")
}

fn placeholders(text: &str) -> usize {
    text.matches("{}").count()
}

#[test]
fn every_shown_text_is_translated_and_every_translation_is_used() {
    let used = used_texts();
    let catalog = catalog();
    let missing: Vec<String> = used
        .iter()
        .filter(|(text, _)| !catalog.contains_key(*text))
        .map(|(text, at)| format!("{at}: {text:?}"))
        .collect();
    assert!(
        missing.is_empty(),
        "texts without an Arabic translation in crates/twitty-core/i18n/ar.json:\n{}",
        missing.join("\n")
    );
    let unused: Vec<&String> = catalog.keys().filter(|k| !used.contains_key(*k)).collect();
    assert!(
        unused.is_empty(),
        "translations nothing uses (remove them from ar.json): {unused:#?}"
    );
}

#[test]
fn translations_are_filled_and_keep_their_placeholders() {
    for (english, arabic) in catalog() {
        assert!(
            !arabic.trim().is_empty(),
            "{english:?} has an empty translation"
        );
        assert_eq!(
            placeholders(&english),
            placeholders(&arabic),
            "{english:?} and {arabic:?} have different {{}} placeholders"
        );
    }
}

#[test]
fn lookup_follows_the_language_and_falls_back_to_english() {
    assert_eq!(UiLanguage::English.t("Copied"), "Copied");
    assert_eq!(UiLanguage::Arabic.t("Copied"), "اتنسخ");
    assert_eq!(
        UiLanguage::Arabic.t("A text nobody translated"),
        "A text nobody translated"
    );
}
