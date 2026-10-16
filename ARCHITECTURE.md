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
           render/scene ── quads (backgrounds, box drawing, cursor, chrome) + text runs  [CPU]
                                         │
                                         ▼
           render/gpu   ── wgpu quad pipeline + glyphon text pipeline                    [GPU]
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
| `render/scene/` | The frame on the CPU: row quads, box drawing, cursor, tab bar, status line, IME preedit, z-order. | `render/row`, `render/style` |
| `render/gpu/` | wgpu device and surface, the quad pipeline (`quad.wgsl`) and glyphon's text pipeline. | `wgpu`, `glyphon` |
| `color.rs`, `config.rs` | Themes and palette resolution; `config.json` loading, clamping and atomic saving. | — |
| `links.rs`, `blink.rs`, `ime.rs` | URL detection and opening, cursor blink timing, IME helpers. | — |

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

## Testing

| Layer | How it is tested |
| --- | --- |
| Key/mouse encoding, routing, pointer state | Pure functions and state machines, unit tests in `tests/` |
| Tabs, splits, focus, close order | `Workspace` with a fake pane factory (no PTY) |
| Terminal behavior | `Terminal::new` + `process_bytes` + `snapshot` |
| Bidi shaping, extents, hit testing, quads | `RowShaper` with a real `FontSystem`, no GPU |
| Whole CPU frame | `scene::frame::build_frame` headless |
| GPU pipelines | Exercised by running the app; see CONTRIBUTING.md for the manual checklist |

## Where to start

- A rendering bug in Arabic text: write a failing test modelled on `tests/bidi_layout_tests.rs`,
  then look at `render/text/shaper.rs` (marks, segments) or `render/row/` (extents, hits).
- A key that sends the wrong bytes: `tests/kitty_keyboard_tests.rs` and `input/keys.rs`.
- Tabs or panes misbehaving: `tests/layout_tests.rs`, `session/workspace.rs`, `layout.rs`.
