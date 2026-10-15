# 0003. Native Rust: wgpu, cosmic-text, alacritty_terminal

**Status:** Accepted (replaced the Tauri + xterm.js app in 3.0.0)

## Context

Versions 1.x and 2.x were a Tauri app embedding xterm.js. xterm.js renders each cell
independently, so Arabic letters never joined and bidi was impossible without fighting the
library. Startup time and memory were those of a browser.

## Decision

Rewrite natively:

- `alacritty_terminal` for VT parsing, grid, scrollback, selection and search — mature and
  independent of Alacritty's renderer.
- `cosmic-text` for shaping (rustybuzz/HarfBuzz rules), bidi and font fallback, through `glyphon`
  for GPU glyph atlases.
- `wgpu` for drawing, with a software adapter fallback for VMs and RDP.
- `winit` for windows and input methods, `portable-pty` for ConPTY/Unix PTYs.

## Consequences

- One codebase for Linux (Wayland and X11) and Windows; macOS is not supported for now.
- Arabic shaping quality follows cosmic-text; font fallback order follows fontdb.
- No plugin or scripting surface, by design for now.
