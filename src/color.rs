use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    #[allow(dead_code)]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: 1.0,
        }
    }

    /// Linear blend towards `other` (`t` = 0 keeps `self`).
    pub fn mix(self, other: Rgba, t: f32) -> Rgba {
        Rgba {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a,
        }
    }

    pub fn to_linear(self) -> [f32; 4] {
        let conv = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [conv(self.r), conv(self.g), conv(self.b), self.a]
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(
            (self.r * 255.0) as u8,
            (self.g * 255.0) as u8,
            (self.b * 255.0) as u8,
            (self.a * 255.0) as u8,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgba,
    pub foreground: Rgba,
    pub cursor: Rgba,
    /// Background of mouse-selected text.
    pub selection: Rgba,
    /// Background of search matches (text on it uses `background` for contrast).
    pub search_match: Rgba,
    /// Background of the focused search match.
    pub search_focus: Rgba,
    pub ansi: [Rgba; 16],
}

impl Default for Palette {
    /// Tokyo Night: a deep dark palette (OpenCode compatible).
    fn default() -> Self {
        THEMES[0].palette()
    }
}

/// Built-in theme names, as accepted by `Palette::named` and the `theme` config key.
pub fn theme_names() -> &'static [&'static str] {
    &THEME_NAMES
}

/// Parses `#rgb` or `#rrggbb` (the leading `#` is optional).
pub fn parse_hex(s: &str) -> Option<Rgba> {
    let digits = s.trim().strip_prefix('#').unwrap_or(s.trim());
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => {
            let v = |i: usize| {
                u8::from_str_radix(&digits[i..i + 1], 16)
                    .ok()
                    .map(|d| d * 17)
            };
            Some(Rgba::from_rgb8(v(0)?, v(1)?, v(2)?))
        }
        6 => {
            let v = |i: usize| u8::from_str_radix(&digits[i..i + 2], 16).ok();
            Some(Rgba::from_rgb8(v(0)?, v(2)?, v(4)?))
        }
        _ => None,
    }
}

/// Builds an opaque color from a 0xRRGGBB literal.
const fn hex(v: u32) -> Rgba {
    Rgba::new(
        ((v >> 16) & 0xff) as f32 / 255.0,
        ((v >> 8) & 0xff) as f32 / 255.0,
        (v & 0xff) as f32 / 255.0,
        1.0,
    )
}

/// The colors of a built-in theme, as 0xRRGGBB values.
struct ThemeData {
    name: &'static str,
    background: u32,
    foreground: u32,
    cursor: u32,
    selection: u32,
    ansi: [u32; 16],
}

impl ThemeData {
    /// The palette of this theme; search highlights are derived from its yellows.
    fn palette(&self) -> Palette {
        let background = hex(self.background);
        let ansi = self.ansi.map(hex);
        Palette {
            background,
            foreground: hex(self.foreground),
            cursor: hex(self.cursor),
            selection: hex(self.selection),
            search_match: background.mix(ansi[3], 0.6),
            search_focus: ansi[11],
            ansi,
        }
    }
}

/// Built-in themes; the first one is the default.
const THEMES: [ThemeData; 8] = [
    ThemeData {
        name: "tokyo-night",
        background: 0x15161e,
        foreground: 0xc0caf5,
        cursor: 0xf5e0dc,
        selection: 0x33467c,
        ansi: [
            0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
            0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5,
        ],
    },
    ThemeData {
        name: "catppuccin-mocha",
        background: 0x1e1e2e,
        foreground: 0xcdd6f4,
        cursor: 0xf5e0dc,
        selection: 0x585b70,
        ansi: [
            0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
            0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
        ],
    },
    ThemeData {
        name: "gruvbox-dark",
        background: 0x282828,
        foreground: 0xebdbb2,
        cursor: 0xebdbb2,
        selection: 0x504945,
        ansi: [
            0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
            0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
        ],
    },
    ThemeData {
        name: "dracula",
        background: 0x282a36,
        foreground: 0xf8f8f2,
        cursor: 0xf8f8f2,
        selection: 0x44475a,
        ansi: [
            0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
            0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff,
        ],
    },
    ThemeData {
        name: "nord",
        background: 0x2e3440,
        foreground: 0xd8dee9,
        cursor: 0xd8dee9,
        selection: 0x434c5e,
        ansi: [
            0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
            0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4,
        ],
    },
    ThemeData {
        name: "one-dark",
        background: 0x282c34,
        foreground: 0xabb2bf,
        cursor: 0x528bff,
        selection: 0x3e4451,
        ansi: [
            0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
            0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xdcdfe4,
        ],
    },
    ThemeData {
        name: "solarized-dark",
        background: 0x002b36,
        foreground: 0x839496,
        cursor: 0x93a1a1,
        selection: 0x073642,
        ansi: [
            0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
            0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
        ],
    },
    ThemeData {
        name: "github-light",
        background: 0xffffff,
        foreground: 0x24292e,
        cursor: 0x24292e,
        selection: 0xc8e1ff,
        ansi: [
            0x24292e, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781,
            0x57606a, 0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
        ],
    },
];

/// The names of [`THEMES`], in order.
const THEME_NAMES: [&str; THEMES.len()] = {
    let mut names = [""; THEMES.len()];
    let mut i = 0;
    while i < THEMES.len() {
        names[i] = THEMES[i].name;
        i += 1;
    }
    names
};

impl Palette {
    /// Looks up a built-in theme. Names are case-insensitive, and spaces or
    /// underscores may be used in place of dashes.
    pub fn named(name: &str) -> Option<Palette> {
        let key = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        THEMES
            .iter()
            .find(|theme| theme.name == key)
            .map(ThemeData::palette)
    }

    pub fn resolve(&self, color: AnsiColor, is_bg: bool) -> Rgba {
        match color {
            AnsiColor::Named(named) => match named {
                NamedColor::Black => self.ansi[0],
                NamedColor::Red => self.ansi[1],
                NamedColor::Green => self.ansi[2],
                NamedColor::Yellow => self.ansi[3],
                NamedColor::Blue => self.ansi[4],
                NamedColor::Magenta => self.ansi[5],
                NamedColor::Cyan => self.ansi[6],
                NamedColor::White => self.ansi[7],
                NamedColor::BrightBlack => self.ansi[8],
                NamedColor::BrightRed => self.ansi[9],
                NamedColor::BrightGreen => self.ansi[10],
                NamedColor::BrightYellow => self.ansi[11],
                NamedColor::BrightBlue => self.ansi[12],
                NamedColor::BrightMagenta => self.ansi[13],
                NamedColor::BrightCyan => self.ansi[14],
                NamedColor::BrightWhite => self.ansi[15],
                NamedColor::Foreground => self.foreground,
                NamedColor::Background => self.background,
                NamedColor::Cursor => self.cursor,
                _ => {
                    if is_bg {
                        self.background
                    } else {
                        self.foreground
                    }
                }
            },
            AnsiColor::Spec(rgb) => Rgba::from_rgb8(rgb.r, rgb.g, rgb.b),
            AnsiColor::Indexed(idx) => {
                if (idx as usize) < 16 {
                    self.ansi[idx as usize]
                } else if (16..=231).contains(&idx) {
                    let i = idx - 16;
                    let r = (i / 36) % 6;
                    let g = (i / 6) % 6;
                    let b = i % 6;
                    let conv = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
                    Rgba::from_rgb8(conv(r), conv(g), conv(b))
                } else {
                    let gray = (idx - 232) * 10 + 8;
                    Rgba::from_rgb8(gray, gray, gray)
                }
            }
        }
    }
}
