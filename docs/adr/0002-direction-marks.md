# 0002. LTR paragraphs with isolates and direction marks per segment

**Status:** Accepted

## Context

Running the Unicode bidi algorithm on a raw terminal row gives poor results: a prompt jumps to
the right when it contains Arabic, `git log` hashes split into `7 … abaf40`, a `✓` after Arabic
lands on the wrong side, and the line being typed reflows on every key press.

## Decision

Each text segment is shaped with explicit marks:

- **LRM prefix** on segments with Arabic, so the paragraph direction is left to right and the row
  never mirrors.
- **Arabic phrases read right to left.** From the first word holding Arabic to the end of the
  segment is a phrase: a segment that starts with it is laid out right to left as a whole (RLM
  prefix), and a phrase after a left-to-right lead (a prompt, `echo `) is wrapped in RLI…PDI so the
  lead stays on the left. English typed after an Arabic word therefore lands on its left, as in
  any Arabic text, including on the line being typed. In finished output a phrase where Latin
  words outnumber Arabic ones (`Error: ملف not found`) stays left to right. (Until October 2026
  the line being typed was kept left to right, which put English typed after Arabic on the wrong
  side.)
- **LRI…PDI** around Latin words and numbers inside Arabic, so they stay whole and in order.
- **RLM** after neutrals (spaces, punctuation, symbols) that trail Arabic, so they stay on its
  left — including spaces just typed after an Arabic word.
- Segments break at box drawing and at runs of 8+ spaces, so TUI columns are shaped
  independently.

## Consequences

- Prompts, tables and logs keep their layout; Arabic phrases read correctly inside them.
- Every rule is pinned by regression tests (`tests/bidi_layout_tests.rs`,
  `tests/arabic_bidi_tests.rs`, `tests/selection_and_rtl_tests.rs`).
- The marks are invisible but occupy bytes in the shaped text, so cell↔glyph maps must account
  for them (`cell_bytes`).
- This is a heuristic, not the full paragraph-level UBA a word processor uses. Cases that span
  several rows (wrapped Arabic paragraphs) are shaped row by row.
