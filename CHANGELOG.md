# Changelog

## 2.0.1 — 2026-09-29

### TUI layout and BiDi isolation

- Isolated multi-column terminal layouts from bidirectional text bleed by treating space runs of 3 or more characters and background color changes as strict layout boundaries.
- Confined wide whitespace tracking strictly to active cursor input fields, preventing TUI sidebars and tables (e.g. OpenCode, Neovim) from merging into chat text.
- Extracted bidirectional text segmentation and shaping into a dedicated `src/shaping.rs` module, cleanly decoupling shaping passes from GPU surface rendering.

### Mouse selection and clipboard

- Added native click-and-drag text selection across all interactive TUIs without requiring Shift modifier keys.
- Implemented double-click semantic word selection and triple-click whole-line selection with automatic clipboard population on release.
- Added persistent Wayland clipboard preservation via `wl-copy` (both `CLIPBOARD` and `PRIMARY` selections) and `xclip` fallback.
- Added right-click selection copy: right-clicking on active selection copies text immediately to the clipboard.
- Added full Arabic layout parity for core control shortcuts (`Ctrl + C/V/X/Z/A` matching `ؤ/ر/ء/ئ/ش`).

### Security, testing, and platforms

- Added automated Cargo dependency security auditing via `cargo-audit` in CI.
- Automated SHA-256 checksum generation for Linux and Windows release binaries.
- Resolved Windows `%APPDATA%\twitty\config.json` configuration path resolution.
- Expanded test coverage across 13 test suites (28 tests) including chunked UTF-8 streaming fuzzing, high-DPI scaling, font fallback chains, and PTY throughput benchmarks exceeding 97 MB/s.
- Unified licensing to standard MIT License across `Cargo.toml`, `README.md`, and `LICENSE.txt`.

## 2.0.0 — 2026-09-28

### Architecture and rendering

- Replaced the hybrid web/DOM architecture and xterm.js wrapper with an end-to-end native Rust core.
- Implemented hardware-accelerated GPU surface rendering using WGPU 30 (Vulkan on Linux, Direct3D 12 on Windows).
- Integrated Glyphon and Cosmic-Text for native text rasterization and caching.
- Implemented zero-gap geometric rendering for VT box-drawing and block-character sequences (U+2500..U+259F) to eliminate font-margin gaps in TUIs.
- Configured native surface alpha compositing (PreMultiplied and PostMultiplied modes) for Wayland compositor transparency and Hyprland background blur.

### Terminal and BiDi engine

- Implemented standard Unicode Bidirectional Algorithm (UAX #9) processing backed by pure-Rust HarfBuzz text shaping.
- Introduced scoped RTL paragraph segmenting to prevent Arabic phrases from displacing adjacent TUI columns, sidebars, or prompt geometry.
- Enforced an LTR paragraph base across terminal prompt lines to keep prompt chevrons and shell paths from inverting when followed by Arabic user input.
- Anchored the text-editing cursor to the visual termination of the active shaped run, providing linear advance across whitespace and eliminating out-of-container cursor jumps.
- Connected the VT engine query responder to the PTY write channel, correctly answering DSR, DA, and DECID queries without terminal escape sequence leakage.

### Desktop and configuration

- Added persistent user configuration via `~/.config/twitty/config.json` supporting custom font sizes, background opacity, and cursor styles.
- Added support for customizable cursor geometries, defaulting to a 2px vertical beam (`beam`) with support for `underline` and `block`.
- Created and installed a Linux desktop entry (`twitty.desktop`) and scalable vector application icon under `~/.local/share/applications` and `~/.local/share/icons`.
- Implemented interactive keyboard and mouse zoom controls (`Ctrl + Plus`, `Ctrl + Minus`, `Ctrl + 0`, and `Ctrl + MouseWheel`) that write immediately to the persistent configuration.

### Performance and input

- Implemented full row-level differential caching, eliminating redundant font shaping and buffer recreation on unchanged screen lines.
- Configured low-latency Mailbox present mode with single-frame presentation to eliminate input latency.
- Supported SGR mouse reporting (`1000`, `1002`, `1006`), allowing mouse-wheel scrolling and click events to pass directly to interactive TUI applications.
- Switched shortcut processing to hardware keycodes to keep core terminal controls (`Ctrl+C`, `Ctrl+D`, `Ctrl+Shift+V`) functional across non-Latin keyboard layouts.
- Implemented native mouse text selection with visual contrast highlighting and integrated clipboard operations (`Ctrl+C`, `Ctrl+V`, `Ctrl+X`, `Ctrl+Z`, `Ctrl+A`) with full Arabic layout parity and bracketed paste support.

## 1.2.1 — 2026-07-26

### Security and release integrity

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

## 1.2.0 — 2026-07-26

### Brand and installer

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

### Terminal

- Restored full ANSI and 24-bit color rendering in native WebView2.
- Fixed Arabic word spacing, mixed Arabic/English paragraph direction, and
  ANSI-split Arabic ordering without changing PTY data.
- Reduced native terminal startup time and hardened ConPTY UTF-8/session
  handling.
