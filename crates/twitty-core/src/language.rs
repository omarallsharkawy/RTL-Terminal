//! Window-independent interface language and locale resolution.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::layout::Rect;

/// The running app's interface language, for messages built away from any window (errors,
/// toasts, PTY notices). The window keeps it in sync with `ui_language`.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// `english` or `arabic`, whichever matches the current interface language.
pub fn tr<'a>(english: &'a str, arabic: &'a str) -> &'a str {
    match UiLanguage::current() {
        UiLanguage::English => english,
        UiLanguage::Arabic => arabic,
    }
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

    pub fn text(self, english: &'static str, arabic: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Arabic => arabic,
        }
    }
}
