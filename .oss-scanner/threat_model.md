# Threat Model: Twitty (RTL Terminal)

## 1. What this project does and where untrusted input enters
Twitty is a GPU-accelerated, bidirectional (Arabic RTL + English LTR) terminal emulator built with Rust, WGPU, Cosmic-Text, and Alacritty PTY/VTE libraries.

Untrusted input enters through:
1. **PTY Output Streams**: Byte streams emitted by untrusted command-line applications, SSH sessions, or files displayed via `cat`/`less`/etc. These streams contain ANSI escape sequences, control characters, and UTF-8 text parsed by the VTE state machine.
2. **OSC Sequences (Operating System Commands)**:
   - OSC 52: Clipboard read and write sequences.
   - OSC 10/11/4: Terminal color query and theme setting sequences.
3. **Clipboard and Paste Payloads**: Content pasted by the user or received from external desktop tools, including bracketed paste escape sequences.
4. **Window and Mouse Events**: Coordinates, mouse reporting sequences (SGR mouse protocol), and keyboard modifier combinations.
5. **Config File & Arguments**: `~/.config/twitty/config.json` and CLI arguments passed to `twitty -e ...`.

## 2. Components that matter most / least
- **Most Critical (In Scope)**:
  - `src/terminal.rs`: VTE parsing, OSC sequence handlers (especially OSC 52 clipboard access control and responses to query events).
  - `src/app.rs`: PTY input/output event loops, paste sanitization (stripping `\x1b[201~` escape delimiters), clipboard integration, and child process execution.
  - `src/shaping.rs` & `src/renderer.rs`: Unicode BiDi algorithm execution (UAX #9), text shaping, font fallback resolution, and geometric quad rendering. Must be resilient against maliciously crafted Unicode grapheme clusters, zero-width joiners, and BiDi override sequences.
  - `src/input.rs` & `src/mouse.rs`: Key mapping and mouse protocol encoding (Kitty keyboard protocol, SGR mouse).
- **Less Important / Out of Scope**:
  - `legacy-web/`: Deprecated web/Tauri prototype, not used in the native v2 build.
  - Desktop UI visual preferences (color palettes, window blur aesthetics, padding).

## 3. How to exercise it
- Automated test suite: `cargo test` runs 52+ unit and integration tests covering:
  - ANSI / VTE parsing and OSC queries (`tests/query_tests.rs`, `tests/terminal_ansi_tests.rs`)
  - Bracketed paste sanitization (`tests/bracketed_paste_tests.rs`)
  - BiDi shaping & Unicode handling (`tests/arabic_bidi_tests.rs`, `tests/tashkeel_tests.rs`, `tests/utf8_stream_tests.rs`)
  - Mouse and keyboard encoding (`tests/mouse_encoding_tests.rs`, `tests/input_zoom_tests.rs`)
- Binary execution: `cargo run --release -- --help` or `cargo run --release -- -e <cmd>`.

## 4. How you rate severity
- **Critical**:
  - Remote Code Execution (RCE) via malicious terminal escape sequences, ANSI parsing, or command argument injection.
  - Arbitrary memory corruption (buffer overflows, out-of-bounds writes) in any native code paths or dependencies.
- **High**:
  - Unauthorized clipboard read/exfiltration via OSC 52 or paste manipulation.
  - Denial of Service (DoS): Crash or infinite loop triggered by untrusted terminal output (e.g. malformed UTF-8, pathological BiDi sequences, or deeply nested escape codes).
- **Medium**:
  - PTY state desynchronization, terminal lockup, or uncontrolled resource consumption (memory leak during sustained high-throughput output).
- **Low**:
  - Display artifacts, visual misordering, or minor font fallback glitches without crash or security impact.

## 5. Anything to leave alone
- Rendering differences dependent on hardware GPU drivers or specific Wayland compositor protocol variations (Niri, Hyprland, Sway).

