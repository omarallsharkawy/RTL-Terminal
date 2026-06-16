<div align="center">

# Twitty · RTL Terminal

**A bidirectional terminal emulator that renders Arabic the way it's meant to be read.**

Mixed Arabic/English text shapes contextually and flows right-to-left — inline, in real shells, and inside full-screen TUI apps — without breaking cursor positioning.

[![Build RTL Terminal](https://github.com/omarallsharkawy/RTL-Terminal/actions/workflows/build.yml/badge.svg)](https://github.com/omarallsharkawy/RTL-Terminal/actions/workflows/build.yml)
&nbsp;·&nbsp; Tauri v2 &nbsp;·&nbsp; React 19 &nbsp;·&nbsp; xterm.js 6 &nbsp;·&nbsp; Rust

<img src="docs/assets/screenshot-hero.png" alt="Twitty rendering mixed Arabic and English: Arabic letters joined contextually and flowing right-to-left, English staying left-to-right, with crisp box-drawing borders." width="900">

</div>

---

## Why Twitty

Most terminals treat Arabic as a stream of disconnected, left-to-right code points. Letters don't join, words read backwards, and anything bidirectional turns into noise. Twitty fixes that at the rendering layer: Arabic is **contextually shaped** (letters take their correct initial/medial/final/isolated forms and join), each Arabic run is **ordered right-to-left**, and Latin text, numbers, and box-drawing stay exactly where they belong.

Crucially, none of this rewrites the byte stream. The terminal buffer stays in logical order, so the cursor, scroll regions, and TUI redraws all behave correctly — Arabic just *looks* right.

## Features

- **Contextual Arabic shaping** — initial / medial / final / isolated presentation forms with correct joining.
- **Per-run bidirectional ordering** — Arabic flows right-to-left; English, digits, and symbols stay left-to-right, mixed on the same line.
- **Render-layer, not stream-layer** — shaping happens as the GPU draws cells, so the buffer stays in logical order and the cursor never desyncs.
- **TUI-safe** — works inside alternate-screen apps (editors, pagers, CLIs); box-drawing and block glyphs tile seamlessly with no gaps.
- **Real shell, real PTY** — a genuine pseudo-terminal via Rust `portable-pty` (Windows ConPTY and Unix PTY).
- **Full ANSI support** — 24-bit color, alternate screen, scroll regions, mouse reporting, 10k-line scrollback.
- **Bundled Arabic font** — Noto Naskh Arabic ships with the app so presentation forms render on every OS, including Linux.
- **F11 fullscreen**, native **Ctrl+C** interrupt, auto-resize, and shell auto-respawn on exit.
- **Live status bar** — connection state, active shell, and an AR⇄EN capability indicator.
- **Browser demo mode** — open without a PTY to preview the rendering (the screenshots above are this mode).
- **Cross-platform builds** — Windows installer + Linux `.deb` / `.rpm` / AppImage from one GitHub Actions workflow.

## Screenshots

| Mixed Arabic + English, shaped and ordered | Crisp box-drawing for TUIs |
| --- | --- |
| ![Arabic and English mixed inline](docs/assets/screenshot-main.png) | ![Box-drawing table rendering](docs/assets/screenshot-wide.png) |

> The Arabic phrase `مرحبا بك في Twitty` reads right-to-left while `Twitty` stays left-to-right; the comma in `السلام عليكم, world` lands between the two scripts correctly.

## Quick start

### Browser demo (no shell)

```bash
npm install
npm run dev
```

Open the printed `localhost` URL. There's no PTY in the browser, so this shows a demo banner — useful for previewing the Arabic rendering.

### Desktop app (real shell)

Install [Rust](https://rustup.rs/) first, then:

```bash
npm install
npm run tauri:dev
```

On Windows the backend launches `powershell.exe -NoLogo`; on Unix it uses `$SHELL`, falling back to `/bin/bash`.

### Production build

```bash
npm run build         # type-check + bundle the frontend
npm run tauri:build   # produce native installers
```

## How the RTL rendering works

The hard part of an Arabic terminal isn't shaping — it's shaping **without corrupting the terminal**. Earlier approaches rewrote the PTY output before handing it to the renderer, which changed cell counts and columns and desynced the cursor, producing duplicated and misplaced text in TUI apps.

Twitty does it the other way around:

```
┌─────────────┐   raw bytes    ┌──────────────┐  logical-order   ┌─────────────────────┐
│  Real shell │ ─────────────▶ │  Rust PTY    │ ───────────────▶ │  xterm.js buffer    │
│ (PTY/ConPTY)│  terminal://   │  bridge      │   terminal://    │  (unchanged order)  │
└─────────────┘     data       └──────────────┘     data         └──────────┬──────────┘
                                                                             │ render
                                                       character joiner ◀────┤ each Arabic
                                                       marks Arabic runs     │ run drawn via
                                                                             ▼ GPU fillText
                                                                  ┌─────────────────────┐
                                                                  │  Browser applies     │
                                                                  │  shaping + RTL       │
                                                                  │  *within the run*    │
                                                                  └─────────────────────┘
```

1. **The byte stream is never modified.** Raw PTY output is written into xterm.js exactly as received, so the buffer stays in logical order.
2. **A character joiner marks Arabic runs.** A run begins and ends on an Arabic letter and may include the spaces between Arabic words, so multi-word phrases order RTL together.
3. **The GPU renderer draws each run through the browser's text engine.** `@xterm/addon-webgl` draws joined ranges via canvas `fillText`, which applies native contextual shaping and right-to-left ordering *inside the run only*.

Because reordering happens at draw time and not in the buffer, cursor math, scroll regions, and alternate-screen redraws all stay correct. If WebGL is unavailable, the app falls back to the DOM renderer, which applies the same shaping.

> **Note on `allowProposedApi`:** `registerCharacterJoiner` is a proposed API in xterm.js v6, so the terminal is constructed with `allowProposedApi: true`. Without it the joiner throws at startup and the app never mounts.

## Architecture

The Rust backend spawns a real shell inside a PTY and streams its raw output to the frontend as `terminal://data` events. The frontend writes that output unmodified into xterm.js, which handles all ANSI parsing, cursor movement, colors, and scrollback. Keystrokes flow back to the PTY via `write_terminal`; window resizes via `resize_terminal`.

### Key files

| File | Responsibility |
| --- | --- |
| `src/components/XtermTerminal.tsx` | xterm.js setup, WebGL renderer, Arabic character joiner, key handling |
| `src/components/StatusBar.tsx` | Live connection / shell / AR⇄EN status bar |
| `src/App.tsx` | App shell; owns status state |
| `src/styles.css` | OKLCH dark palette, bundled `@font-face`, layout |
| `src-tauri/src/pty.rs` | PTY session lifecycle and raw I/O |
| `src-tauri/src/lib.rs` | Tauri commands: `start_terminal`, `write_terminal`, `interrupt_terminal`, `resize_terminal`, `stop_terminal` |
| `src-tauri/tauri.conf.json` | App + bundle config, CSP, window |
| `.github/workflows/build.yml` | Windows + Linux builds and GitHub release |

### Tech stack

- **Shell:** [Tauri v2](https://tauri.app/) (Rust core, system WebView).
- **Backend:** Rust with [`portable-pty`](https://crates.io/crates/portable-pty) for cross-platform PTY.
- **Frontend:** [React 19](https://react.dev/) + [Vite 6](https://vitejs.dev/) + [xterm.js 6](https://xtermjs.org/) with `@xterm/addon-fit` and `@xterm/addon-webgl`.
- **Font:** Noto Naskh Arabic (SIL OFL), subset to Arabic Unicode ranges.

## Platform support

| Platform | WebView engine | Status |
| --- | --- | --- |
| Windows | WebView2 (Chromium) | Primary target |
| Linux | webkit2gtk (WebKit) | Built via CI (`.deb` / `.rpm` / AppImage) |
| macOS | WebKit | Buildable from source |

## Development

```bash
npm run dev           # Vite dev server (browser demo)
npm run tauri:dev     # Tauri dev (real shell)
npm run build         # tsc + vite build
node scripts/test-shaping.mjs    # Arabic run-detection tests
node scripts/capture-shots.mjs   # regenerate docs screenshots
```

## Documentation

A full PDF write-up of the app and its design decisions lives at
[`docs/twitty-overview.pdf`](docs/twitty-overview.pdf).

## License

Twitty is distributed under the End User License Agreement in [`LICENSE.txt`](LICENSE.txt). The bundled Noto Naskh Arabic font is licensed separately under the SIL Open Font License.

## Notes

- The `kitty/` directory is vendored reference source, not part of the build, and is gitignored.
- Windows and Linux installers are produced by a single GitHub Actions workflow on tagged releases.
