# 0001. The terminal grid stays logical; bidi is applied only when drawing

**Status:** Accepted

## Context

A terminal is a grid of fixed-width cells addressed by programs through cursor movement. Arabic
is written right to left with contextual letter shapes of varying width. Earlier attempts at RTL
terminals either reorder the bytes (breaking every program that positions the cursor) or render
the whole screen right to left (breaking prompts, TUIs and mixed output).

## Decision

The emulator state is never touched: `alacritty_terminal` keeps cells in logical order and
programs see a normal xterm. Bidi reordering and Arabic shaping happen per row at draw time, and
every glyph keeps a map back to its source cell. Cursor, selection, search, backgrounds and mouse
hits are translated through that map, not assumed to match the column.

## Consequences

- Any program works unchanged: vim, tmux, htop, shells, Claude Code.
- Copy and search operate on logical text, so copied Arabic is in correct reading order.
- The renderer must solve the hard part: mapping glyph clusters to cells, stretching extents so
  colored backgrounds have no holes, and hit-testing the mouse against the drawn layout
  (`render/row`).
- A program cannot ask for a right-to-left screen; Twitty decides direction per segment
  (see 0002). Programs that implement their own bidi would be double-processed; none are common.
