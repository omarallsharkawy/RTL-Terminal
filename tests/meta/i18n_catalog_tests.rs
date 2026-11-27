//! The translation catalogs (`crates/twitty-core/i18n/*.json`) against the code: every English
//! interface text the code shows has a translation, nothing in a catalog is left unused, and
//! placeholders match.
//!
//! Interface text is found where it is marked: `t("…")`, `.t("…")` and `en!("…")`. Arabic
//! written straight into the code is allowed only in the places listed in [`ARABIC_IN_CODE`].

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
                // A line continuation; `\r` when Git checks out Windows line endings.
                '\n' | '\r' => {
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
                // Skip comments (doc examples).
                let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
                if src[line_start..start].trim_start().starts_with("//") {
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

/// Files that keep Arabic in the code, how many lines, and why. Tests are not counted.
const ARABIC_IN_CODE: &[(&str, usize, &str)] = &[
    (
        "crates/twitty-ai/src/ai/parse.rs",
        1,
        "labels an agent writes in Arabic answers",
    ),
    (
        "crates/twitty-ai/src/ai/prompt.rs",
        2,
        "the answer language named to the agent",
    ),
    (
        "crates/twitty-scene/src/agent/inspector.rs",
        5,
        "Arabic counted nouns (one, two, few, many)",
    ),
    (
        "crates/twitty-scene/src/settings/commands.rs",
        1,
        "an Arabic sample name",
    ),
    (
        "crates/twitty-scene/src/welcome.rs",
        2,
        "Arabic sample text and the name العربية",
    ),
    (
        "crates/twitty-ui/src/input/broadcast.rs",
        2,
        "a badge shown in both languages",
    ),
    (
        "crates/twitty-ui/src/input/keybindings.rs",
        14,
        "startup warnings in both languages",
    ),
    (
        "crates/twitty-ui/src/ui/agent_panel/mod.rs",
        17,
        "Arabic counted nouns (one, two, few, many)",
    ),
    (
        "crates/twitty-ui/src/ui/settings_panel/table.rs",
        1,
        "the name العربية",
    ),
];

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06FF}').contains(&c)
}

/// Lines outside comments and test modules with Arabic after a `"`.
fn arabic_lines(src: &str) -> usize {
    let lines: Vec<&str> = src.lines().collect();
    let mut count = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("#[cfg(test)]")
            && lines.get(i + 1).is_some_and(|l| l.starts_with("mod "))
        {
            break;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(at) = line.find(is_arabic) {
            count += usize::from(line[..at].contains('"'));
        }
    }
    count
}

#[test]
fn arabic_lives_in_the_catalog() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = BTreeMap::new();
    for file in sources() {
        let count = arabic_lines(&std::fs::read_to_string(&file).unwrap());
        if count > 0 {
            let name = file
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(name, count);
        }
    }
    let allowed: BTreeMap<String, usize> = ARABIC_IN_CODE
        .iter()
        .map(|(file, count, _)| (file.to_string(), *count))
        .collect();
    assert_eq!(
        found, allowed,
        "Arabic text belongs in crates/twitty-core/i18n/ar.json: write the English in the code \
         and translate it with t(..); update ARABIC_IN_CODE only for a real exception"
    );
    for english in catalog().keys() {
        assert!(
            !english.chars().any(is_arabic),
            "catalog key {english:?} is not English"
        );
    }
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
    assert_eq!(UiLanguage::Arabic.t("Copied"), "تم النسخ");
    assert_eq!(
        UiLanguage::Arabic.t("A text nobody translated"),
        "A text nobody translated"
    );
    assert_eq!(UiLanguage::English.t("Block|cursor"), "Block");
    assert_eq!(
        UiLanguage::English.t("Type a command…|composer"),
        "Type a command…"
    );
    assert_eq!(
        UiLanguage::English.t("curl | sh, wget | bash (network pipe to shell)"),
        "curl | sh, wget | bash (network pipe to shell)"
    );
    assert_eq!(
        UiLanguage::English
            .t("Allow-list prefixes cannot contain shell metacharacters (;, &&, ||, |, $, >, <)."),
        "Allow-list prefixes cannot contain shell metacharacters (;, &&, ||, |, $, >, <)."
    );
    assert!(!UiLanguage::English.is_rtl());
    assert!(UiLanguage::Arabic.is_rtl());
    assert_eq!(
        twitty_core::language::fill("{1} of {0}", &[&10, &3]),
        "3 of 10"
    );
    assert_eq!(
        twitty_core::language::fill("{} of {}", &[&3, &10]),
        "3 of 10"
    );
}
