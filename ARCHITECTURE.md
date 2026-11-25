# Architecture

Twitty is a terminal emulator whose one unusual job is drawing Arabic (and other right-to-left
text) correctly inside a fixed-width grid, without changing a single byte the shell and its
programs exchange. This document explains how the pieces fit, and where to look when changing
one of them. Decisions with lasting consequences are recorded in [`docs/adr/`](docs/adr/).

## The one rule

**The terminal grid is the source of truth; only drawing is bidirectional.**

`alacritty_terminal` parses the PTY stream into cells exactly as any VT emulator would. Cursor
movement, line wrapping, selection, search and everything programs can observe stay in logical,
left-to-right grid coordinates. Bidi happens afterwards, per row, when the row is shaped for the
screen, and every drawn glyph keeps a link back to the cell it came from. That link is what lets
the cursor, backgrounds, selection, mouse hits and IME follow the text the user sees.

## Data flow

```
             keys / mouse / IME                         PTY output (reader thread)
                    │                                              │
                    ▼                                              ▼
 winit ──► app/ (route event) ──► input/ (encode bytes) ──► pty.rs ──► shell
                    │                                              │
                    │                    AppEvent::PtyOutput ◄─────┘
                    ▼                                              
              session/ Pane ──► terminal.rs (alacritty_terminal: parse, grid, scrollback)
                                         │
                                         │ snapshot(): rows of CellData + cursor
                                         ▼
           render/text  ── split row into segments, wrap Latin runs in isolates,
                            shape with cosmic-text (bidi + Arabic joining) ──► CachedSegment
                                         │   (glyph ↔ cell byte map)
                                         ▼
           render/row   ── visual extent of each cell, hit testing, caret anchor, row cache
                                         │
                                         ▼
           render/scene ── terminal quads + chrome shapes + clipped text runs           [CPU]
                                         │
                                         ▼
           render/gpu   ── wgpu quad and shape pipelines, then glyphon text              [GPU]
```

Everything above `render/gpu` runs without a GPU and is covered by tests in `tests/`.

## Modules

| Module | Responsibility | Depends on |
| --- | --- | --- |
| `app/` | The winit `ApplicationHandler`. `mod.rs` only routes events; `keyboard`, `mouse`, `selection`, `panes` and `event` handle one concern each. `window.rs` is the only file that calls the renderer. | everything below |
| `session/` | Panes (terminal + PTY + per-pane UI state), the `Workspace` of tabs and split trees, and the `ShellFactory` that spawns real shells. `Workspace` takes a factory trait, so tab/pane logic is tested with fake panes. | `terminal`, `pty`, `layout` |
| `input/` | Pure input logic: key encoding (legacy xterm and the kitty keyboard protocol), key routing (search bar / shortcut / program), mouse reporting, click counting and the pointer state machine. | `terminal::TermMode` |
| `ui/` | Window-free UI state: hit testing, search bar, selection lifecycle, zoom/opacity steps, clipboard. | `layout` |
| `layout.rs` | Split-tree geometry: pane rectangles, dividers, tab bar. Pure arithmetic. | — |
| `terminal.rs` | Wrapper around `alacritty_terminal::Term`: feeding bytes, snapshots for drawing, selection, search, links, program-set palette colors. | `alacritty_terminal` |
| `pty.rs` | Shell discovery, environment, PTY spawning, a writer thread so large pastes never block the UI. | `portable-pty` |
| `render/text/` | Fonts, bidi helpers, and `RowShaper`, which turns a row of cells into shaped segments. | `cosmic-text` |
| `render/row/` | A shaped row: per-cell visual extents, mouse hit testing, caret placement, and the per-pane `RowCache`. | `render/text` |
| `render/scene/` | CPU frame composition. Terminal quads, window chrome, sidebars and overlays are layered before glyphon draws. Palette bounds cut a hole in the text runs behind the panel. | `render/row`, `render/style`, `render/shapes` |
| `render/shapes.rs`, `render/icons.rs`, `render/ui_style.rs`, `render/text/ui.rs` | Shared UI foundation. Physical-pixel shapes and icons, scalable theme tokens and single-line proportional UI labels with Arabic fallback and grapheme-safe truncation. | `color`, `layout`, `cosmic-text` |
| `ui/chrome.rs`, `ui/hit.rs` | Shared toolbar, tab strip and pane header geometry. The same pane grid rect drives drawing, terminal hit testing and PTY sizes. | `layout`, `render/metrics` |
| `ui/git_diff.rs`, `app/git_diff.rs` | Bounded Git numstat parsing and subprocess I/O. The app schedules one in-flight scan for the focused local pane and discards stale results. | `std::process`, `rustix` on Linux |
| `render/gpu/` | wgpu device and surface. Plain quads draw first, then one instanced SDF draw per nonempty shape layer, then glyphon text. Shapes use premultiplied alpha. | `wgpu`, `glyphon` |
| `color.rs`, `config.rs` | Themes and palette resolution; `config.json` loading, clamping and atomic saving. | — |
| `links.rs`, `blink.rs`, `ime.rs` | URL detection and opening, cursor blink timing, IME helpers. | — |
| `shell_integration.rs` | Parses the OSC signals shells and agents send (OSC 7 cwd, OSC 9/99/777 notifications, OSC 133/633 command marks) into bounded, cleaned `ShellEvent`s. | `links` |
| `session/shell_setup.rs`, `assets/shell/`, `assets/bin/` | Injects Twitty's bash/zsh/fish integration scripts and private `claude` hook wrapper (`0700` dir) into shells it starts, without modifying user startup or Claude settings files. | — |
| `session/restore.rs` | Versioned `session.json` saves tab order, splits, focus, sidebar, local pane directories and titles, and SSH session identities. Restores local shells and reattaches SSH panes. | `session`, `remote` |
| `blocks/` | Command blocks: turns marks into a per-pane `BlockList` (prompt, command, output, exit, duration) anchored to absolute lines so blocks follow the text as it scrolls. | `shell_integration` |
| `sidebar/` | The vertical tab sidebar model: one entry per tab with cwd, git branch (read from `.git/HEAD`, bounded), running state, last exit and the latest notification. | `session` |
| `notify.rs` | Desktop notifications through `notify-send` (argv, never a shell), throttled per pane. | — |
| `commands.rs`, `ui/palette.rs` | The command table (name, English and Arabic titles, shortcut, action) and the palette state with Arabic-aware matching. | `input` |
| `ui/search_all.rs` | "Search all terminals", the palette's search mode: scans each pane's text lines (`Terminal::for_each_text_line`, newest first, bounded), folds them like the palette, shares at most 200 results between panes, cuts each line around its match. The scan runs in 8 ms slices from `about_to_wait`, resuming each pane by absolute line; the jump goes through `Terminal::focus_search_at` to the scrollback search highlight. | `terminal`, `ui/palette` |
| `control/` | The control socket (`$XDG_RUNTIME_DIR/twitty/twitty-<pid>.sock`, 0600 in a 0700 dir) speaking newline-delimited JSON (`twitty ctl` + cmux v2 JSON-RPC + minimal v1 text hook commands), `twitty claude-hook`, and the `twitty ctl/send/notify/list/read` client. | `session`, `sidebar`, `input`, `ui` |
| `agent.rs` | "Ask an agent": picks a local agent CLI, builds its argv and a fenced prompt from the selection, block or screen. | — |
| `ports.rs` | Listening TCP ports per pane for the sidebar: pure `/proc` parsing and a scanner thread. | — |
| `remote/`, `app/ssh.rs` | Validates SSH identities, verifies and installs the versioned helper over OpenSSH, and reconnects an ordinary pane PTY on transport failure. | `session`, `pty` |
| `remote/daemon/`, `bin/twitty-remote.rs` | GUI-free Linux helper. Owns persistent login-shell PTYs and a 256 KiB output tail per session. Attachments use framed terminal bytes, resize and exit messages on a private Unix socket. | `portable-pty`, `rustix` |

Dependencies point downwards only: nothing in `render/`, `terminal.rs`, `input/` or `ui/` knows
about winit windows or the app.

## Shaping a row

`RowShaper::build_row` (in `render/text/shaper.rs`):

1. **Segments.** A row is cut at box-drawing/block cells (drawn as geometry, never as text) and
   at runs of 8+ spaces, so TUI columns never bleed into each other. Short space runs stay inside
   a segment so bidi sees whole phrases.
2. **Direction marks.** A segment containing Arabic gets a leading LRM so the paragraph stays
   left-to-right and prompts never jump to the other side; the Arabic phrase (first Arabic word
   to the end of the segment) is laid out right to left, isolated with RLI…PDI after a prompt. Latin words and numbers inside Arabic are wrapped in LRI…PDI so `7abaf40` cannot be
   torn into `7 … abaf40`. Neutrals after trailing Arabic get an RLM so `تم ✓` keeps the mark on
   the Arabic side. See [ADR 0002](docs/adr/0002-direction-marks.md).
3. **Shaping.** cosmic-text applies the Unicode bidi algorithm and Arabic joining.
4. **Mapping back.** Each segment records the byte offset of every cell in the shaped text
   (`cell_bytes`). From the glyph clusters, `render/row` derives each cell's visual x-extent,
   which drives backgrounds, selection, underlines, the cursor and mouse hit testing.
5. **Grid lock for fallbacks.** An LTR segment whose glyphs don't land exactly on their columns
   (emoji, CJK, icon fonts with other advances) is reshaped cell by cell, pinned to the grid.

Shaped rows are cached per pane; a row is reshaped only when its cells, cursor, selection, search
or link-hover state change.

## Threads

- **Main thread:** the winit event loop, all terminal state, all rendering. No locks: each pane
  owns its `Terminal` directly.
- **PTY reader thread (per pane):** reads the PTY into the pane's bounded `OutputQueue` and
  sends `AppEvent::PtyOutput` when the queue was empty, so a flood of output arrives in a few
  large batches; a full queue (4 MiB) makes the reader wait for the UI. Events go through the
  event-loop proxy; sends `AppEvent::PtyExit` when the shell ends.
- **PTY writer thread (per pane):** drains a channel of input bytes so a huge paste never blocks
  a frame.
- **Control server:** one accept thread plus at most 16 short-lived client threads, with read,
  write and request timeouts. Requests reach the main thread as `AppEvent::Control` and the
  reply comes back over a channel, so all state is still touched on the main thread only.
- **SSH:** bootstrap runs in the pane's local PTY process, which becomes OpenSSH after verification.
  Each named remote session has a detached host process holding its shell PTY. The attach client
  relays raw input and window sizes. Multiple attachments use the smallest grid while attached.
- **Notifications:** each `notify-send` runs on its own short thread (at most 8 at once).
- **Git diff:** while the top bar has room for the badge, the focused local pane queues a
  scan on cwd change, command completion and every 3 s. Each scan gets a 2 s subprocess
  budget, at most 1 MiB of numstat output, and up to 4096 file records.

## Shell signals

Shells and agents report things through OSC sequences that `alacritty_terminal` ignores.
`Terminal::process_bytes` stops the parser at each OSC terminator, hands the payload (at most
4 KiB) to `shell_integration::parse_osc` and queues the result with the cursor position
(at most 256 pending). After each batch of output the app drains the queue:

- **OSC 7** sets `PaneMeta::cwd` (sidebar, new tabs and splits start there).
- **OSC 9 / 99 / 777** set the pane's attention (sidebar line, ring around the pane) and send a
  desktop notification unless the pane is focused in a focused window.
- **OSC 133 / 633 A B C D** drive `blocks/`; **633 E** gives the command line.

All program text is cleaned (no control or bidi-override characters) and bounded before it is
shown, sent to `notify-send`, returned over the control socket or put into an agent prompt.
- **Port scanner thread (one, Linux):** while the sidebar is shown, every 2 s, reads `/proc` for
  the TCP ports the panes' processes listen on and sends `AppEvent::Ports` when they change.
  It sleeps on its channel while the sidebar is hidden.

## Testing

| Layer | How it is tested |
| --- | --- |
| Key/mouse encoding, routing, pointer state | Pure functions and state machines, unit tests in `tests/` |
| Tabs, splits, focus, close order | `Workspace` with a fake pane factory (no PTY) |
| Terminal behavior | `Terminal::new` + `process_bytes` + `snapshot` |
| Bidi shaping, extents, hit testing, quads | `RowShaper` with a real `FontSystem`, no GPU |
| Whole CPU frame | `scene::frame::build_frame` headless |
| GPU pipelines | Offscreen llvmpipe golden frames and pixel assertions in `tests/golden_tests.rs` |

## Where to start

- A rendering bug in Arabic text: write a failing test modelled on `tests/bidi_layout_tests.rs`,
  then look at `render/text/shaper.rs` (marks, segments) or `render/row/` (extents, hits).
- A key that sends the wrong bytes: `tests/kitty_keyboard_tests.rs` and `input/keys.rs`.
- Tabs or panes misbehaving: `tests/layout_tests.rs`, `session/workspace.rs`, `layout.rs`.
