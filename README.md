<div align="center">

<img src="docs/assets/twitty-logo.png" alt="Twitty bidirectional terminal mark" width="128">

# Twitty · RTL Terminal

**A bidirectional terminal emulator that renders Arabic the way it's meant to be read.**

Mixed Arabic/English text shapes contextually and flows right-to-left — inline and in real shells — with guarded PTY/session handling for stable day-to-day terminal use.

[![Build RTL Terminal](https://github.com/omarallsharkawy/RTL-Terminal/actions/workflows/build.yml/badge.svg)](https://github.com/omarallsharkawy/RTL-Terminal/actions/workflows/build.yml)
&nbsp;·&nbsp; Tauri v2 &nbsp;·&nbsp; React 19 &nbsp;·&nbsp; xterm.js 6 &nbsp;·&nbsp; Rust

<img src="docs/assets/screenshot-hero.png" alt="Twitty rendering mixed Arabic and English: Arabic letters joined contextually and flowing right-to-left, English staying left-to-right, with crisp box-drawing borders." width="900">

</div>

---

## Why Twitty

Most terminals treat Arabic as a stream of disconnected, left-to-right cells. Letters don't join, words read backwards, and anything bidirectional turns into noise. Twitty keeps the PTY stream unchanged and groups each visible Arabic phrase into a single xterm.js DOM render run. The browser then recalculates contextual shaping and right-to-left order from the complete current line after every character, while Latin text, numbers, ANSI styling, and box-drawing keep their normal terminal positions.

The app also hardens the PTY bridge around real terminal behavior: backend reads preserve split UTF-8 sequences, frontend listeners are cleaned up safely, ordered input batching prevents keystroke races across the async bridge, and session IDs prevent stale shell events from respawning or writing into the wrong terminal instance.

## Features

- **Contextual Arabic shaping** — full-line browser shaping updates correctly even when a shell echoes one character at a time.
- **Cross-style bidirectional ordering** — Arabic flows right-to-left even across adjacent ANSI-colored spans; English, digits, symbols, and TUI geometry stay left-to-right.
- **Protocol-safe rendering** — Unicode and ANSI/control sequences reach xterm unchanged; direction handling exists only in the DOM renderer.
- **Session-hardened PTY bridge** — split UTF-8 reads, stale shell events, and reconnect cleanup are handled defensively.
- **Ordered mixed-script input** — burst typing and paste are serialized per session, preserving Arabic, English, and control-key order exactly.
- **Fast native startup** — the Tauri bridge is bundled eagerly, shell listeners register in parallel, and Windows shell discovery avoids slow PATH scans.
- **Real Windows shell, real PTY** — PowerShell runs inside Windows ConPTY via Rust `portable-pty`.
- **Full ANSI support** — xterm's dynamic 24-bit palette stylesheet is permitted by the native CSP; alternate screen, scroll regions, mouse reporting, and 10k-line scrollback remain intact.
- **Windows-native typography** — Cascadia Mono/Consolas for the grid, Segoe UI for shaped Arabic, and bundled Noto Naskh Arabic as fallback.
- **F11 fullscreen**, native **Ctrl+C** interrupt, auto-resize, and shell auto-respawn on exit.
- **Terminal-only surface** — no internal title rail, shell selector, direction switch, status bar, or shortcut legend.
- **Browser demo mode** — open without a PTY to preview the rendering (the screenshots above are this mode).
- **Windows-focused builds** — one tested x64 NSIS installer and Windows-native CI gates.

## Screenshots

| Mixed Arabic + English, shaped and ordered | Crisp box-drawing for TUIs |
| --- | --- |
| ![Arabic and English mixed inline](docs/assets/screenshot-main.png) | ![Box-drawing table rendering](docs/assets/screenshot-wide.png) |

> The Arabic phrase `مرحبا بك في Twitty` reads right-to-left while `Twitty` stays left-to-right; the comma in `السلام عليكم, world` lands between the two scripts correctly.

## Quick start

### Install on Windows

1. Open the [latest Twitty release](https://github.com/omarallsharkawy/RTL-Terminal/releases/latest).
2. Under **Assets**, download `Twitty_<version>_x64-setup.exe` and run it.
3. Choose English or العربية, then continue through the setup pages.

The x64 installer is for Windows 10/11 and installs for the current user, so
the default installation does not require an Administrator prompt.

During setup you can:

- keep the default `%LOCALAPPDATA%\Twitty` destination or choose another writable folder;
- create the Start Menu folder, rename it, or opt out of Start Menu shortcuts;
- optionally create a desktop shortcut on the finish page (off by default);
- launch Twitty immediately after installation.

Setup includes the WebView2 bootstrapper: it detects an existing WebView2
runtime and installs it when needed. An internet connection may therefore be
needed on a fresh Windows installation. Setup also blocks accidental downgrades.

### Browser demo (no shell)

```bash
npm ci
npm run dev
```

Open the printed `localhost` URL. There's no PTY in the browser, so this shows a demo banner — useful for previewing the Arabic rendering.

### Desktop app (real shell)

Install [Rust](https://rustup.rs/) first, then:

```bash
npm ci
npm run tauri:dev
```

The backend uses `TWITTY_SHELL` when set, then checks the standard PowerShell 7 and Windows PowerShell locations before falling back through PATH, `%COMSPEC%`, and `cmd.exe`. PowerShell starts with `-NoLogo -NoProfile`.

### Production build

```bash
npm ci
npm run build         # type-check + bundle the frontend
npm run tauri:build -- --bundles nsis --ci --no-sign
```

The installer is written to
`src-tauri/target/release/bundle/nsis/Twitty_<version>_x64-setup.exe`.

## How the RTL rendering works

The hard part of an Arabic terminal isn't only shaping — it's shaping without changing terminal protocol data. Twitty incrementally decodes PTY UTF-8 so Arabic characters are not split at read boundaries, sends the original Unicode and ANSI stream into xterm, then joins Arabic phrases only at DOM-render time.

```
┌─────────────┐   raw bytes    ┌──────────────┐  unchanged UTF-8/ANSI  ┌─────────────────────┐
│  Real shell │ ─────────────▶ │  Rust PTY    │ ────────────────────▶ │ xterm.js buffer     │
│ (PTY/ConPTY)│                │  bridge      │ session-scoped events  │ terminal grid       │
└─────────────┘                └──────────────┘                         └──────────┬──────────┘
                                                                                  │ visible line
                                                                                  ▼
                                                                       ┌─────────────────────┐
                                                                       │ DOM character joiner│
                                                                       │ browser Arabic/BiDi │
                                                                       └─────────────────────┘
```

1. **The backend preserves UTF-8 boundaries.** PTY bytes are decoded incrementally, so Arabic characters split across read chunks are not replaced with `�`.
2. **Events are session-scoped.** `terminal://data` and `terminal://exited` include a session ID so stale killed shells cannot write to or respawn the current terminal.
3. **The xterm buffer stays canonical.** Shell input and output remain ordinary Unicode; the app never replaces Arabic with presentation-form characters.
4. **Arabic joins in the DOM renderer.** A character joiner groups each complete Arabic phrase so the browser shapes and orders the latest full line, including slow character-by-character echo.
5. **ANSI styles share one local RTL group.** Adjacent colored Arabic spans keep their individual colors and cell widths inside one atomic RTL wrapper, so styling individual words does not reverse their sentence order.
6. **The terminal grid stays LTR.** Active prompts remain on xterm's LTR grid; completed Arabic-led or Arabic-dominant prose receives a scoped RTL paragraph base while embedded Latin remains LTR.

> **Note on `allowProposedApi`:** `registerCharacterJoiner` is a proposed API in xterm.js v6, so the terminal is constructed with `allowProposedApi: true`.

## Architecture

The Rust backend spawns a real shell inside a PTY and streams incrementally decoded output to the frontend as session-scoped `terminal://data` events. The frontend writes that stream unchanged into xterm.js, whose DOM character joiner handles visible Arabic phrases. Keystrokes enter a per-session queue that batches same-turn input and serializes every `write_terminal` call; window resizes via `resize_terminal`.

### Key files

| File | Responsibility |
| --- | --- |
| `src/components/XtermTerminal.tsx` | xterm.js setup, session-scoped PTY events, key handling |
| `src/components/arabicRenderer.ts` | Arabic phrase detection for xterm's DOM character joiner |
| `src/components/terminalInputQueue.ts` | Ordered, session-scoped terminal input batching |
| `src/App.tsx` | Terminal-only surface and contextual recovery notice |
| `src/styles.css` | Full-window terminal layout and bundled Arabic `@font-face` |
| `src-tauri/src/pty.rs` | PTY session lifecycle, incremental UTF-8 decoding, shell selection |
| `src-tauri/src/lib.rs` | Tauri commands: `start_terminal`, `write_terminal`, `interrupt_terminal`, `resize_terminal`, `stop_terminal` |
| `branding/` | Canonical app mark and branded NSIS artwork |
| `src-tauri/installer-assets/` | Generated Windows installer ICO/BMP assets |
| `src-tauri/installer-hooks.nsh` | Installer defaults such as the optional desktop shortcut |
| `src-tauri/tauri.conf.json` | App + Windows installer config, CSP, window |
| `.github/workflows/build.yml` | Windows build, quality gates, and GitHub release |

### Tech stack

- **Shell:** [Tauri v2](https://tauri.app/) (Rust core, system WebView).
- **Backend:** Rust with [`portable-pty`](https://crates.io/crates/portable-pty) for Windows ConPTY.
- **Frontend:** [React 19](https://react.dev/) + [Vite 6](https://vitejs.dev/) + [xterm.js 6](https://xtermjs.org/) with `@xterm/addon-fit`.
- **Font:** Cascadia Mono/Consolas + Segoe UI, with Noto Naskh Arabic (SIL OFL) fallback.

## Platform support

| Platform | WebView engine | Status |
| --- | --- | --- |
| Windows 10/11 | WebView2 (Chromium) | Supported target |

## Development

```bash
npm run dev           # Vite dev server (browser demo)
npm run tauri:dev     # Tauri dev (real shell; PTY failure surfaces an inline retry notice)
npm run build         # tsc + vite build
npm test              # Arabic rendering + session/reconnect + ordered-input regression tests
node scripts/capture-shots.mjs   # regenerate docs screenshots
```

## License

Twitty is distributed under the End User License Agreement in [`LICENSE.txt`](LICENSE.txt). The bundled Noto Naskh Arabic font is licensed separately under the SIL Open Font License.

## Notes

- The `kitty/` directory is vendored reference source, not part of the build, and is gitignored.
- The Windows installer is built by GitHub Actions; GitHub releases are created only for version tags.
