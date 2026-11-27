//! Window-independent interface language and locale resolution, and the translations.
//!
//! Interface text is written in English in the code and passed through [`t`] (or
//! [`UiLanguage::t`]); the English text is the key into one catalog per language in
//! `crates/twitty-core/i18n/` (`ar.json`). Text in a table that is built at compile time is
//! marked with [`en!`](crate::en) so the catalog check finds it, and translated where it is
//! shown. A key may carry `|context` when one English word has two meanings.
//! `tests/i18n_catalog_tests.rs` fails when the code uses a text the catalog lacks or
//! the catalog keeps one nothing uses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use crate::layout::Rect;

/// The running app's interface language, for messages built away from any window (errors,
/// toasts, PTY notices). The window keeps it in sync with `ui_language`.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The Arabic catalog: English text to its translation.
pub const ARABIC_CATALOG: &str = include_str!("../i18n/ar.json");

fn arabic() -> &'static HashMap<String, String> {
    static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(ARABIC_CATALOG).unwrap_or_default())
}

/// `english` in the current interface language (see [`UiLanguage::t`]).
pub fn t(english: &str) -> &str {
    UiLanguage::current().t(english)
}

/// `template` with each `{}` replaced by the next of `args`, in order. Templates go through the
/// catalog like any text: `fill(t("Unknown key {}"), &[&key])`.
pub fn fill(template: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut args = args.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        match args.next() {
            Some(arg) => out.push_str(&arg.to_string()),
            None => out.push_str("{}"),
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// Mark English interface text in a table built at compile time, so the catalog check finds
/// it. It is the literal itself; translate it with [`t`] where it is shown.
#[macro_export]
macro_rules! en {
    ($text:literal) => {
        $text
    };
}

/// Configured interface language preference (`config.json` key `"ui_language"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLanguagePref {
    /// Follow the system locale environment (`LC_ALL`, `LC_MESSAGES`, `LANG`).
    #[default]
    Auto,
    English,
    Arabic,
}

impl UiLanguagePref {
    pub const ALL: [Self; 3] = [Self::English, Self::Arabic, Self::Auto];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Arabic => "ar",
        }
    }

    /// Resolve to a concrete UI language, falling back to `env_fallback` when set to `Auto`.
    pub fn resolve_with(self, env_fallback: UiLanguage) -> UiLanguage {
        match self {
            Self::Auto => env_fallback,
            Self::English => UiLanguage::English,
            Self::Arabic => UiLanguage::Arabic,
        }
    }

    /// Resolve using the current process locale environment for `Auto`.
    pub fn resolve(self) -> UiLanguage {
        self.resolve_with(UiLanguage::from_environment())
    }
}

impl std::str::FromStr for UiLanguagePref {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "en" => Ok(Self::English),
            "ar" => Ok(Self::Arabic),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLanguage {
    #[default]
    English,
    Arabic,
}

impl UiLanguage {
    pub fn from_locale(locale: &str) -> Self {
        if locale
            .get(..2)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ar"))
        {
            Self::Arabic
        } else {
            Self::English
        }
    }

    /// The interface language set with [`UiLanguage::set_current`] (English until then).
    pub fn current() -> Self {
        match CURRENT.load(Ordering::Relaxed) {
            1 => Self::Arabic,
            _ => Self::English,
        }
    }

    /// Make this the language [`tr`] and [`UiLanguage::current`] answer with.
    pub fn set_current(self) {
        CURRENT.store(u8::from(self == Self::Arabic), Ordering::Relaxed);
    }

    pub fn from_environment() -> Self {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|locale| !locale.is_empty())
            .map_or(Self::default(), |locale| Self::from_locale(&locale))
    }

    pub fn mirror(self, rect: Rect, within: Rect) -> Rect {
        if self == Self::Arabic {
            Rect::new(
                within.right() - (rect.right() - within.x),
                rect.y,
                rect.w,
                rect.h,
            )
        } else {
            rect
        }
    }

    /// `english` in this language: itself, or its translation from the catalog (English when
    /// the catalog has none). A key may end in `|context` to tell apart one English word with
    /// two meanings (`"Block|cursor"`); the context is never shown.
    pub fn t(self, english: &str) -> &str {
        let shown = english.split_once('|').map_or(english, |(text, _)| text);
        match self {
            Self::English => shown,
            Self::Arabic => arabic().get(english).map_or(shown, String::as_str),
        }
    }

    /// One of two texts that come with the data (a command's own Arabic name, say). Interface
    /// text uses [`t`](Self::t).
    pub fn text(self, english: &'static str, arabic: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Arabic => arabic,
        }
    }
}
