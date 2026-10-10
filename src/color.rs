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
    pub ansi: [Rgba; 16],
}

impl Default for Palette {
    fn default() -> Self {
        // Deep dark terminal palette (Tokyo Night / OpenCode compatible)
        Self {
            background: Rgba::from_rgb8(21, 22, 30), // #15161e (crisp dark background)
            foreground: Rgba::from_rgb8(192, 202, 245), // #c0caf5
            cursor: Rgba::from_rgb8(245, 224, 220),  // #f5e0dc
            selection: Rgba::from_rgb8(51, 70, 124), // #33467c (Tokyo Night selection)
            ansi: [
                Rgba::from_rgb8(21, 22, 30),    // 0: Black
                Rgba::from_rgb8(247, 118, 142), // 1: Red
                Rgba::from_rgb8(158, 206, 106), // 2: Green
                Rgba::from_rgb8(224, 175, 104), // 3: Yellow
                Rgba::from_rgb8(122, 162, 247), // 4: Blue
                Rgba::from_rgb8(187, 154, 247), // 5: Magenta
                Rgba::from_rgb8(125, 207, 255), // 6: Cyan
                Rgba::from_rgb8(169, 177, 214), // 7: White
                Rgba::from_rgb8(65, 72, 104),   // 8: Bright Black (gutter/borders)
                Rgba::from_rgb8(247, 118, 142), // 9: Bright Red
                Rgba::from_rgb8(158, 206, 106), // 10: Bright Green
                Rgba::from_rgb8(224, 175, 104), // 11: Bright Yellow
                Rgba::from_rgb8(122, 162, 247), // 12: Bright Blue
                Rgba::from_rgb8(187, 154, 247), // 13: Bright Magenta
                Rgba::from_rgb8(125, 207, 255), // 14: Bright Cyan
                Rgba::from_rgb8(192, 202, 245), // 15: Bright White
            ],
        }
    }
}

/// Built-in theme names, as accepted by `Palette::named` and the `theme` config key.
pub fn theme_names() -> &'static [&'static str] {
    &[
        "tokyo-night",
        "catppuccin-mocha",
        "gruvbox-dark",
        "dracula",
        "nord",
        "one-dark",
        "solarized-dark",
        "github-light",
    ]
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

impl Palette {
    /// Looks up a built-in theme. Names are case-insensitive, and spaces or
    /// underscores may be used in place of dashes.
    pub fn named(name: &str) -> Option<Palette> {
        let key = name.trim().to_ascii_lowercase().replace([' ', '_'], "-");
        let p = match key.as_str() {
            "tokyo-night" => Palette::default(),
            "catppuccin-mocha" => Palette {
                background: hex(0x1e1e2e),
                foreground: hex(0xcdd6f4),
                cursor: hex(0xf5e0dc),
                selection: hex(0x585b70),
                ansi: [
                    hex(0x45475a),
                    hex(0xf38ba8),
                    hex(0xa6e3a1),
                    hex(0xf9e2af),
                    hex(0x89b4fa),
                    hex(0xf5c2e7),
                    hex(0x94e2d5),
                    hex(0xbac2de),
                    hex(0x585b70),
                    hex(0xf38ba8),
                    hex(0xa6e3a1),
                    hex(0xf9e2af),
                    hex(0x89b4fa),
                    hex(0xf5c2e7),
                    hex(0x94e2d5),
                    hex(0xa6adc8),
                ],
            },
            "gruvbox-dark" => Palette {
                background: hex(0x282828),
                foreground: hex(0xebdbb2),
                cursor: hex(0xebdbb2),
                selection: hex(0x504945),
                ansi: [
                    hex(0x282828),
                    hex(0xcc241d),
                    hex(0x98971a),
                    hex(0xd79921),
                    hex(0x458588),
                    hex(0xb16286),
                    hex(0x689d6a),
                    hex(0xa89984),
                    hex(0x928374),
                    hex(0xfb4934),
                    hex(0xb8bb26),
                    hex(0xfabd2f),
                    hex(0x83a598),
                    hex(0xd3869b),
                    hex(0x8ec07c),
                    hex(0xebdbb2),
                ],
            },
            "dracula" => Palette {
                background: hex(0x282a36),
                foreground: hex(0xf8f8f2),
                cursor: hex(0xf8f8f2),
                selection: hex(0x44475a),
                ansi: [
                    hex(0x21222c),
                    hex(0xff5555),
                    hex(0x50fa7b),
                    hex(0xf1fa8c),
                    hex(0xbd93f9),
                    hex(0xff79c6),
                    hex(0x8be9fd),
                    hex(0xf8f8f2),
                    hex(0x6272a4),
                    hex(0xff6e6e),
                    hex(0x69ff94),
                    hex(0xffffa5),
                    hex(0xd6acff),
                    hex(0xff92df),
                    hex(0xa4ffff),
                    hex(0xffffff),
                ],
            },
            "nord" => Palette {
                background: hex(0x2e3440),
                foreground: hex(0xd8dee9),
                cursor: hex(0xd8dee9),
                selection: hex(0x434c5e),
                ansi: [
                    hex(0x3b4252),
                    hex(0xbf616a),
                    hex(0xa3be8c),
                    hex(0xebcb8b),
                    hex(0x81a1c1),
                    hex(0xb48ead),
                    hex(0x88c0d0),
                    hex(0xe5e9f0),
                    hex(0x4c566a),
                    hex(0xbf616a),
                    hex(0xa3be8c),
                    hex(0xebcb8b),
                    hex(0x81a1c1),
                    hex(0xb48ead),
                    hex(0x8fbcbb),
                    hex(0xeceff4),
                ],
            },
            "one-dark" => Palette {
                background: hex(0x282c34),
                foreground: hex(0xabb2bf),
                cursor: hex(0x528bff),
                selection: hex(0x3e4451),
                ansi: [
                    hex(0x282c34),
                    hex(0xe06c75),
                    hex(0x98c379),
                    hex(0xe5c07b),
                    hex(0x61afef),
                    hex(0xc678dd),
                    hex(0x56b6c2),
                    hex(0xabb2bf),
                    hex(0x5c6370),
                    hex(0xe06c75),
                    hex(0x98c379),
                    hex(0xe5c07b),
                    hex(0x61afef),
                    hex(0xc678dd),
                    hex(0x56b6c2),
                    hex(0xdcdfe4),
                ],
            },
            "solarized-dark" => Palette {
                background: hex(0x002b36),
                foreground: hex(0x839496),
                cursor: hex(0x93a1a1),
                selection: hex(0x073642),
                ansi: [
                    hex(0x073642),
                    hex(0xdc322f),
                    hex(0x859900),
                    hex(0xb58900),
                    hex(0x268bd2),
                    hex(0xd33682),
                    hex(0x2aa198),
                    hex(0xeee8d5),
                    hex(0x002b36),
                    hex(0xcb4b16),
                    hex(0x586e75),
                    hex(0x657b83),
                    hex(0x839496),
                    hex(0x6c71c4),
                    hex(0x93a1a1),
                    hex(0xfdf6e3),
                ],
            },
            "github-light" => Palette {
                background: hex(0xffffff),
                foreground: hex(0x24292e),
                cursor: hex(0x24292e),
                selection: hex(0xc8e1ff),
                ansi: [
                    hex(0x24292e),
                    hex(0xcf222e),
                    hex(0x116329),
                    hex(0x4d2d00),
                    hex(0x0969da),
                    hex(0x8250df),
                    hex(0x1b7c83),
                    hex(0x6e7781),
                    hex(0x57606a),
                    hex(0xa40e26),
                    hex(0x1a7f37),
                    hex(0x633c01),
                    hex(0x218bff),
                    hex(0xa475f9),
                    hex(0x3192aa),
                    hex(0x8c959f),
                ],
            },
            _ => return None,
        };
        Some(p)
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
