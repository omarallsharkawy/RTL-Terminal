# Changelog

## Unreleased

- New tabs and splits open in the focused pane's directory (Linux).
- Colors that programs redefine with OSC 4 (pywal, vim themes) are used for drawing and reported back to color queries.
- Config problems (invalid JSON, unknown theme, bad color, missing font) are shown in yellow at the top of the first pane, not only on stderr.
- Fixed: a config.json with a syntax error was silently replaced by the defaults on the next zoom or opacity change. It is now left untouched; saves are atomic, and out-of-range `font_size` / `background_opacity` are clamped.
- Fixed: concealed text (`\e[8m`, passwords) showed through a see-through background, and was revealed by selection or search.
- Fixed: text was drawn darker than its theme color on non-sRGB surfaces.
- Fixed: the block cursor covered only half of a wide (CJK, emoji) character, and box-drawing characters under it vanished.
- Fixed: `╬` and the `╓╖╙╜` joins were drawn wrong.
- Fixed: `font_family` is matched case-insensitively (`"jetbrains mono"` works).
- Fixed: IME composition in the middle of a line no longer shows the text behind it.
- Fixed: on AZERTY and other layouts, Ctrl+letter sent the control byte of the physical key instead of the typed letter.
- Fixed: Alt+Arrow with one pane, and Ctrl+Tab / Ctrl+PageUp/PageDown with one tab, now reach the program (fish, vim).
- Fixed: Shift+drag selections leaked mouse drag reports to programs, and a focus loss mid-drag left a button stuck.
- Fixed: the release of a key Twitty used as a shortcut was still reported to programs using the kitty keyboard protocol.
- Fixed: Arabic punctuation (`،` `؛` `؟`) and closing quotes after a URL are no longer part of the link.
- Fixed: a trackpad Ctrl+swipe zoomed to the minimum or maximum font size at once.
- Fixed: shells started from inside kitty, tmux or WezTerm inherited that terminal's variables (KITTY_WINDOW_ID, TMUX, TERMINFO…). `TERM_PROGRAM_VERSION` is now set.
- Fixed: the Windows release opened an extra console window.
- Internals: the app and the renderer are split into focused modules (`app/`, `session/`, `input/`, `ui/`, `render/`); terminal state no longer sits behind a mutex; per-frame CPU work is separated from the GPU code and tested without a GPU.
- Tabs and split panes. Ctrl+Shift+T opens a tab, Ctrl+Shift+E / Ctrl+Shift+O split the focused pane side by side or stacked, Ctrl+Shift+W closes a pane (the last pane closes the tab, the last tab closes the window), Ctrl+Tab / Ctrl+PageDown cycle tabs, Alt+Arrow moves focus between panes, and dragging a divider resizes a split. Each pane has its own shell, selection, search and links; the unfocused pane's cursor is hollow. A tab bar (titles from the program's OSC 0/2 title, Arabic included) shows with two or more tabs. Ctrl+Tab no longer reaches programs; Ctrl+Alt+Tab still does.
- Noto Naskh Arabic is embedded, so Arabic renders on machines without an Arabic font. System fonts still win where they cover the script.
- `font_family` option picks the terminal font; a family that is not installed falls back to the system monospace font.
- Scrollback search: Ctrl+Shift+F opens a search bar at the bottom. Matching is literal and case-insensitive, updates as you type (Arabic included), highlights every visible match, and shows `n/N`. Enter jumps to the next older match, Shift+Enter to a newer one, Esc closes.
- Input method (IME) support: composition text is drawn inline at the cursor, and the candidate popup follows the caret.
- Clickable links: Ctrl+Click opens OSC 8 hyperlinks and plain URLs (http, https, file, ftp, mailto). The link is underlined and the cursor becomes a pointer while Ctrl is held over it.
- Blinking cursor (530 ms on/off), controlled by `cursor_blink` (default `true`) and by programs that request a blinking or steady cursor. It stays solid while typing and while the window is unfocused.
- Arabic layout: selection, background colors and underlines now follow the drawn glyphs of right-to-left text; punctuation and symbols after Arabic (`تم ✓`, `مرحبا!`) stay on its left; finished output lines that start with Arabic read right to left as a whole (the line being typed keeps its stable layout); neighbouring Latin words and numbers keep their order together.
- Relicensed under MIT OR Apache-2.0, at your option. This replaces the old end-user license agreement, which was written for the legacy installer.
- Color themes: pick one with `"theme"` in config.json (`tokyo-night` is still the default; also `catppuccin-mocha`, `gruvbox-dark`, `dracula`, `nord`, `one-dark`, `solarized-dark`, `github-light`). The optional `"colors"` block overrides the background, foreground, cursor, selection and ANSI colors.
- Box drawing: double, heavy and dashed lines, block eighths, quadrants and shades are drawn pixel-exact.

## 3.0.0 — 2026-10-11

Twitty is now a native Rust terminal (wgpu + cosmic-text + alacritty_terminal). It replaces the earlier Tauri/xterm.js app. This release builds on the native rewrite from the `v2-native` line. The 2.1.0 line is archived on the `archive/old-main` branch.

### Arabic and mixed text
- Latin words, commit hashes and numbers inside Arabic are wrapped in LRI/PDI isolates, so `7abaf40` no longer splits into `7 … abaf40`.
- While typing Arabic, the cursor sits on the left of the text. Spaces typed after an Arabic word stay on its left instead of jumping to the right.
- Arabic harakat and other combining marks are kept on their letters.
- Wide characters (CJK, emoji) no longer add extra spaces. Latin text after icons and emoji stays on the grid.

### New
- Mouse selection: drag for characters, double-click for a word, triple-click for a line. Releasing the mouse copies, and the highlight then clears. Ctrl+Shift+C also copies. Hold Shift to select inside apps that capture the mouse.
- See-through window background (`background_opacity`, adjustable with Ctrl+Shift+wheel).
- `cursor_style` option: `block`, `beam` or `underline`. The cursor turns hollow when the window is unfocused.
- Scrollback that actually renders, with smooth trackpad scrolling. Typing jumps back to the bottom.
- HiDPI support (scale factor and live monitor changes).
- Window title, bell, OSC 4/10/11/12 color replies, and OSC 52 copy (clipboard reads are refused).
- Software GPU fallback for VMs and remote desktops.
- Linux release binary next to the Windows one.

### Fixes
- Bracketed paste is honored, and the paste-end marker is stripped from pasted text so it can't inject commands. Windows CRLF line endings are no longer doubled.
- Full key encoding: Alt/Meta, F1–F12, Ctrl/Shift/Alt with arrows, Home/End, PgUp/PgDn, Insert/Delete, plus Ctrl+Space, Ctrl+/, Ctrl+Backspace. AltGr layouts type correctly.
- Inverse, dim, hidden, underline, double underline and strikethrough are rendered.
- The character under a block cursor stays readable.
- Mouse reporting: motion only on cell change, drag mode (1002), legacy X10 encoding with clamped coordinates, and modifier bits.
- Windows uses PowerShell 7, then Windows PowerShell, then `%COMSPEC%`. `TWITTY_SHELL` accepts arguments.
- The window closes when the shell exits instead of respawning it forever.
- The config lives in `%APPDATA%` on Windows and `$XDG_CONFIG_HOME` elsewhere. It is saved once on exit, and keys Twitty doesn't know are kept.
- PTY writes run on their own thread, so large pastes don't freeze the window.
- The surface recovers after it is lost or outdated (resize, GPU reset).

### Project
- Removed the legacy web app, its scripts and assets, and the outdated design docs. README rewritten for the native app.
- CI runs fmt, clippy and tests, builds Windows and Linux binaries, and publishes releases with notes taken from this changelog.

### Not carried over from 2.1.0
Kitty keyboard protocol, scrollback search (Ctrl+Shift+F), embedded Noto Naskh Arabic font, `font_family` option, and IME preedit.

## 1.x — legacy web app

Versions 1.0–1.2.1 were the Tauri + React + xterm.js app, removed in 3.0.0.

### 1.2.1 — 2026-07-26

#### Security and release integrity

- Updated the vulnerable development-only PostCSS and transitive Quick XML
  dependencies, patched an Anyhow soundness advisory, and added high-severity
  npm and Rust dependency audits to CI.
- Hardened the native Content Security Policy against object embeds, base URL
  rewriting, and form submissions.
- Made Vite fail closed when Tauri's fixed development port is already occupied.
- Added weekly Dependabot coverage for npm, Cargo, and GitHub Actions.
- Added SHA-256 checksum generation for future GitHub release assets.
- Documented the `TWITTY_SHELL` trust boundary, unsigned installer status, and
  the current single-session model.

### 1.2.0 — 2026-07-26

#### Brand and installer

- Introduced the Twitty bidirectional terminal mark across the app, installer,
  uninstaller, Start Menu, desktop shortcut, and browser demo.
- Added branded NSIS welcome, finish, and task-page artwork.
- Added English and Arabic installer languages.
- Switched to a current-user install under `%LOCALAPPDATA%\Twitty` so normal
  setup does not require Administrator privileges.
- Added explicit install-directory and Start Menu shortcut choices.
- Kept the desktop shortcut optional and off by default.
- Embedded the WebView2 bootstrapper and blocked accidental downgrades.
- Replaced the old setup license copy with a concise professional bilingual
  EULA.

#### Terminal

- Restored full ANSI and 24-bit color rendering in native WebView2.
- Fixed Arabic word spacing, mixed Arabic/English paragraph direction, and
  ANSI-split Arabic ordering without changing PTY data.
- Reduced native terminal startup time and hardened ConPTY UTF-8/session
  handling.
