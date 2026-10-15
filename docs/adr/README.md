# Architecture decision records

Short records of decisions that shape Twitty and are expensive to reverse. Each one says what
was decided, why, and what it costs. Add a new numbered file instead of rewriting an accepted one;
mark the old one *Superseded by NNNN* if a decision changes.

| # | Decision | Status |
| --- | --- | --- |
| [0001](0001-grid-is-source-of-truth.md) | The terminal grid stays logical; bidi is applied only when drawing | Accepted |
| [0002](0002-direction-marks.md) | LTR paragraphs with isolates and direction marks per segment | Accepted |
| [0003](0003-native-rust-stack.md) | Native Rust: wgpu, cosmic-text, alacritty_terminal | Accepted |
| [0004](0004-box-drawing-as-geometry.md) | Box drawing and block elements are drawn as quads, not glyphs | Accepted |
