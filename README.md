# RTL Terminal

RTL Terminal (Twitty) is a Windows-focused, cross-platform-ready desktop terminal emulator built with Tauri v2 and React 19. It runs a real shell inside a pseudo-terminal and renders it with xterm.js, with extra handling for mixed Arabic/English text.

## What is implemented

- Tauri v2 desktop shell.
- Rust PTY bridge using `portable-pty` for Windows ConPTY and Unix PTY compatibility.
- xterm.js frontend renderer with full ANSI support (alternate screen, scroll regions, 24-bit color, mouse reporting, scrollback).
- Arabic contextual presentation-form shaping and BiDi reordering applied to PTY output before it reaches xterm.js.
- Tabs-free single terminal with auto-resize via `@xterm/addon-fit`.
- Native Windows `Ctrl+C` interrupt via `GenerateConsoleCtrlEvent`.
- Auto-respawn of the shell on exit.
- Browser demo mode when the app is opened without Tauri.

## Run the web demo

```bash
npm install
npm run dev
```

Open the printed localhost URL. This mode shows a demo banner since there is no PTY in the browser.

## Run as a desktop app

Install Rust first, then run:

```bash
npm install
npm run tauri:dev
```

On Windows the backend starts `powershell.exe -NoLogo`. On Unix it uses `$SHELL`, falling back to `/bin/bash`.

## Build

```bash
npm run build
npm run tauri:build
```

## Architecture

The backend spawns a real shell inside a PTY. Raw PTY output is emitted to the frontend as `terminal://data` events. The frontend applies Arabic shaping and BiDi reordering to each chunk, then writes it into xterm.js, which handles all ANSI parsing, cursor movement, alternate screen, colors, and scrollback. Keystrokes are forwarded to the PTY via the `write_terminal` command; window resizes are forwarded via `resize_terminal`.

### Key files

- `src/components/XtermTerminal.tsx` - xterm.js terminal component plus the Arabic shaping/BiDi engine.
- `src/App.tsx` - app entry that mounts the terminal.
- `src/styles.css` - full-screen layout and terminal styling.
- `src-tauri/src/pty.rs` - PTY session lifecycle and raw I/O.
- `src-tauri/src/lib.rs` - Tauri commands (`start_terminal`, `write_terminal`, `interrupt_terminal`, `resize_terminal`, `stop_terminal`).
- `src-tauri/tauri.conf.json` - Tauri app and bundle config.
- `.github/workflows/build.yml` - GitHub Actions for Windows and Linux builds + releases.

## Notes

- The `kitty/` directory is vendored reference source and is not part of the build. It is gitignored.
- Windows and Linux builds are produced by a single GitHub Actions workflow.
