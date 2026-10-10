# Changelog

## Unreleased

- Clickable links: Ctrl+Click opens OSC 8 hyperlinks and plain URLs (http, https, file, ftp, mailto). The link is underlined and the cursor becomes a pointer while Ctrl is held over it.
- Relicensed under MIT OR Apache-2.0, at your option. This replaces the old end-user license agreement, which was written for the legacy installer.

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
