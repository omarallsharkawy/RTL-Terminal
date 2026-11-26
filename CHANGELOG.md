# Changelog

## Unreleased

### Arabic text

- Right-to-left Arabic phrases now read right-to-left even after a shell prompt (`❯ مرحبا hello` shows `hello` left of `مرحبا`), while English sentences containing an Arabic word keep reading left-to-right.
- Selection, background colors, and underlines follow drawn right-to-left glyphs; trailing punctuation and symbols (`تم ✓`, `مرحبا!`) stay on the Arabic side, and adjacent Latin words and numbers stay grouped.
- Embedded Noto Naskh Arabic ensures Arabic renders on machines without a system Arabic font, while installed system fonts still take precedence.
- Roughly 5× faster Arabic row shaping (full-screen redraw down from 77 ms to 18 ms) and ~4× cheaper Arabic scrolling by naming the Arabic font up front instead of falling back per word.
- Input method (IME) support draws composition inline at the cursor with the candidate popup anchored to the caret and no bleed-through of underlying text.
- Arabic punctuation (`،`, `؛`, `؟`) and closing quotes after URLs are no longer absorbed into clickable links.

### Look

- New **Twitty Dark** (default) and **Twitty Light** themes, alongside `tokyo-night`, `catppuccin-mocha`, `gruvbox-dark`, `dracula`, `nord`, `one-dark`, `solarized-dark`, `github-light`, and per-field `colors` overrides.
- Theme-derived chrome surfaces with unified window translucency, smooth 120–160 ms transitions (`reduce_motion` disables animation), and bundled Inter + IBM Plex Sans Arabic proportional UI fonts (`ui_font_family`, `ui_accent`).
- Compact 36 px top bar (`top_bar`) with a sidebar toggle, bilingual command-palette search field, focused Git diff badge, and unread notification bell.
- 24 px pane headers (`pane_headers`) with program/agent icons, titles, split and close buttons, zoom indicator, and focus/attention outlines.
- Redesigned command palette with proportional UI typography, rounded surfaces, and shortcut keycaps.
- Pixel-exact box drawing (double, heavy, and dashed lines, including fixed `╬` and `╓╖╙╜` joins) and block elements; OSC 4 program color redefinition; full-width block cursor over wide CJK/emoji cells; accurate colors on non-sRGB surfaces.

### Sidebar & workspaces

- Vertical workspace sidebar (`sidebar`: `"right"`, `"left"`, `"hidden"`; `sidebar_width`) showing each tab's title, working directory, Git branch, listening TCP ports, running/exit/agent state, unread count, and latest message, with hover close, drag-to-reorder, and a New tab button.
- Hidden-sidebar tab strip uses rounded pills with hover-to-close and a plus button.
- Tabs and split panes (`Ctrl+Shift+T`, `Ctrl+Shift+E`, `Ctrl+Shift+O`, `Ctrl+Shift+W`, `Alt+Arrow`, draggable dividers) open in the focused pane's directory on Linux, with pane zoom (`Ctrl+Shift+Z`), outline flash (`Ctrl+Shift+H`), tab rename (`Ctrl+Shift+N`), close other panes (`Ctrl+Shift+K`), tab reordering (`Ctrl+Shift+PageUp`/`PageDown`), and reopen closed tab (`Ctrl+Shift+Alt+T`).
- Session restore (`restore_session`) reopens tabs, split sizes, directories, titles, focus, sidebar visibility, and SSH sessions saved on normal window close.
- Project switcher (`Ctrl+Shift+L`) combines recent pane directories with Git repositories discovered under `project_roots`.
- Named layouts (save, open, and delete split trees per tab) and opt-in broadcast input to all panes of the active tab (`Esc-Esc` to turn off).
- Scrollback search (`Ctrl+Shift+F`) and Arabic-aware **Search all terminals** (`Ctrl+Shift+G`) across every tab and pane.
- Desktop notifications (`desktop_notifications` via `notify-send`), notification history (`Ctrl+Shift+I`), and unread jumps (`Ctrl+Shift+J` / `Ctrl+Shift+U`).
- Custom workflow commands in `config.json` and per-project `.twitty/commands.json` with English/Arabic titles and `insert` or `run` execution modes.

### Command blocks

- Automatic shell integration (`shell_integration`) for bash, zsh, and fish providing OSC 133 command marks and OSC 7 working directories, with per-build script caching so updates take effect immediately.
- Rounded command block cards (`block_cards`, default `true`) behind main-screen rows with a status strip and hover/selected toolbar (copy command, copy output, ask agent, rerun without newline, duration and exit status); hidden on alternate-screen programs.
- Keyboard navigation between blocks (`Ctrl+Shift+Up`/`Down`) and quick output copy (`Ctrl+Shift+Y`).

### Composer

- Multi-line Arabic-aware command composer (`Ctrl+Shift+Space`; `composer`: `"off"`, `"on-demand"`, `"always"`) docked below the focused pane with bracketed-paste submission.
- History browsing, spec-driven and dynamic completions with Arabic-aware matching and inline ghost suggestions, shell syntax highlighting, and non-blocking pre-run lint marks for unknown commands, unclosed quotes/brackets, and trailing operators.

### AI

- Built-in read-only AI assistant (`ai`, `ai_agent`, `ai_timeout_secs`) powered by locally installed coding-agent CLIs (`claude`, `codex`, `gemini`, `opencode`) — Twitty never stores API keys or makes network calls itself.
- Natural-language command generation in the composer and **Explain error** / **Suggest a fix** on failed command blocks, with one-time privacy consent and review-before-run.
- Interactive **Ask an agent** (`Ctrl+Shift+A`, `agent_command`) opens a local agent in a split with fenced context from the selected block or screen.
- Claude Code hook integration (`claude_hooks`) via a private `claude` wrapper on `PATH` reports live per-pane status (`working` / `needs input` / `done`) and notifications without modifying user Claude settings.

### Settings

- In-app bilingual Settings overlay (`Ctrl+,`) covering Appearance, Behaviour, Keys, and About with live theme and font pickers, search, and atomic `config.json` saving.
- Invalid configuration entries display visible warnings at startup without overwriting hand-edited `config.json` files; `font_family` matches case-insensitively and out-of-range values are clamped.

### SSH

- Persistent Linux SSH tabs via `twitty ssh` and `tab.new-ssh` that verify and upload `twitty-remote` automatically over OpenSSH.
- Remote shells survive dropped connections, replay up to 256 KiB of recent output on reattach, show connection state in the sidebar, and reopen from session restore.

### Control socket

- Private Unix control socket (`control_socket`) and `twitty ctl` CLI for managing tabs and panes, sending input, reading output, posting notifications, and listing or running trusted commands.
- cmux v2 JSON-RPC and minimal v1 hook compatibility (`cmux_compat`), exposing `TWITTY_*` and `CMUX_*` pane variables while stripping inherited ones.

### Review panel

- Side-by-side Git code review panel (`Ctrl+Shift+R`) showing uncommitted changes (`git diff HEAD`) and untracked files with foldable files, line numbers, tinted diff lines, and Arabic-shaped text.
- Comment on any line (click its line number or press `c`) to paste `path:line: comment` into the tab's coding agent without auto-sending; **Commit** inserts `git commit` without running it.
- Runs Git off the UI thread with fixed arguments, timeouts, size/file/line bounds, and external diff/textconv drivers disabled.

### Terminal & input

- Bounded output batching keeps the screen responsive and `Ctrl+C` immediate during heavy output floods.
- Clickable OSC 8 hyperlinks and plain URLs (`Ctrl+Click`), plus configurable blinking cursor (`cursor_blink`, solid while typing or unfocused).
- Input and platform fixes: layout-aware `Ctrl+letter` on AZERTY and other layouts, single-pane/single-tab passthrough for `Alt+Arrow` and `Ctrl+Tab`/`PageUp`/`PageDown`, clean kitty keyboard release handling, fixed Shift+drag mouse reporting and trackpad zoom increments, and no extra console window on Windows release builds.

### Packaging

- Relicensed to AGPL-3.0-or-later with third-party notices in `licenses/`.
- Linux desktop integration: bilingual `twitty.desktop` (matching Wayland `app_id` / X11 `WM_CLASS`), AppStream metainfo (`dev.twitty.Twitty.metainfo.xml`), 16–512 px PNG + scalable SVG hicolor icons, and a release tarball `install.sh` targeting `~/.local` or `/usr/local`.
- AUR packages (`twitty-bin` and `twitty-git` with `.SRCINFO`), an `x86_64` AppImage build, a Windows release zip archive, and release automation scripts (`scripts/release.sh`, `scripts/aur-checksums.sh`).

### Security

- `Ctrl+Click` on a `file://` link to an executable, script, or launcher (`.desktop`, `.AppImage`, `.exe`, `.bat`, or execute-bit set) opens its folder instead of running it, and links to remote hosts are refused.
- Holding `Ctrl` over a link previews its real target in the status line with bidi and control characters replaced by the Unicode replacement character (U+FFFD).
- Bidi overrides, embeddings, and isolates (U+202A–202E, U+2066–2069) in program output and titles are neutralized to prevent Trojan Source spoofing.
- OSC payloads exceeding 8 MiB are cut and ignored; row-cache keys use a per-process random seed; concealed text (`\e[8m`) stays hidden from transparency, selection, and search.
- Per-project `.twitty/commands.json` workflows require explicit SHA-256 trust confirmation in `trust.json` and cannot be run untrusted or auto-trusted over the control socket.
- Private `0700`/`0600` permissions on control sockets, hook wrappers, and state files; inherited terminal variables (`KITTY_WINDOW_ID`, `TMUX`, `CMUX_*`, etc.) are stripped; clipboard paste via `command.run` is blocked.

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
