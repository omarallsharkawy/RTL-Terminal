# 0004. Box drawing and block elements are drawn as quads, not glyphs

**Status:** Accepted

## Context

Font glyphs for `─│┌╬█▌` rarely fill the cell exactly. With a line height larger than the font's
ascent + descent, borders show gaps; with a fallback font, widths differ. TUIs (htop, lazygit,
vim splits) look broken.

## Decision

Cells holding box-drawing (U+2500–U+257F) and block elements (U+2580–U+259F) are not sent to the
shaper. They are drawn as rectangles computed from the cell size (`render/scene/boxdraw.rs`),
including double, heavy and dashed lines, quadrants and shades. They also act as segment
boundaries for shaping.

## Consequences

- Borders join pixel-exactly at any font size or zoom.
- Every supported character needs geometry code and a test (`tests/box_block_tests.rs`,
  `tests/box_double_join_tests.rs`).
- Unsupported line characters fall back to the font.
