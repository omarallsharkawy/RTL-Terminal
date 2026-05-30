# RTL Terminal

RTL Terminal is a Windows-focused, cross-platform-ready desktop terminal emulator scaffold built around an RTL-first rendering pipeline.

## What is implemented

- Tauri v2 desktop shell with transparent window support.
- Rust PTY bridge using `portable-pty` for Windows ConPTY and Unix PTY compatibility.
- Lightweight ANSI parser for SGR colors, cursor moves, clear screen/line, and OSC title updates.
- Grid buffer that keeps logical terminal cells separate from visual presentation.
- Unicode BiDi reordering with Arabic contextual presentation-form shaping.
- React/Vite frontend with a canvas-based terminal renderer, tabs, status, restart, and settings.
- Browser demo mode when the app is opened without Tauri.

## Run the web demo

```bash
npm install
npm run dev
```

Open the printed localhost URL. This mode shows a simulated RTL/mixed-direction terminal stream.

## Run as a desktop app

Install Rust first, then run:

```bash
npm install
npm run tauri:dev
```

On Windows the backend starts the default shell from `%COMSPEC%`, falling back to `powershell.exe`.

## Build

```bash
npm run build
npm run tauri:build
```

## Architecture

The backend receives raw PTY output, parses ANSI control sequences into a logical grid, then emits serialized frames to the frontend. Each line is reordered with the Unicode Bidirectional Algorithm and Arabic letters are shaped before rendering. The frontend paints a stable monospace grid on canvas so mixed Arabic, English, numbers, commands, and paths remain aligned.

## Next production hardening steps

- Replace presentation-form shaping with HarfBuzz/Cosmic Text glyph shaping for full script coverage.
- Expand ANSI support for alternate screen, scroll regions, 24-bit color, and mouse reporting.
- Add visual-to-logical cursor hit-testing and selection mapping.
- Add integration tests around real PTY sessions once Rust is available in CI.
