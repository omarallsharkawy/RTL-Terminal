---
name: Twitty
description: A calm, terminal-first desktop shell for dependable Arabic and English workflows.
colors:
  terminal-bg: "#0b0d10"
  border-subtle: "#39424c"
  ink-primary: "#d3d9df"
  ink-muted: "#8f9aa5"
  accent: "#38bdf8"
  success: "#4ade80"
  warning: "#facc15"
  danger: "#f87171"
typography:
  terminal:
    fontFamily: "Cascadia Mono, Consolas, JetBrains Mono, DejaVu Sans Mono, Liberation Mono, Menlo, Noto Naskh Arabic, monospace"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1
    letterSpacing: "0"
rounded:
  xs: "4px"
  sm: "6px"
  md: "10px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
---

## Overview

**Creative North Star: “The terminal, made legible.”**

Twitty is used for focused technical work, often for long sessions in mixed ambient light. The interface is dark because the terminal canvas fills the window and must remain visually stable beside existing developer tools. There is no persistent application chrome inside the native window; recovery UI appears only after automatic PTY recovery is exhausted.

**Key Characteristics:**

- Terminal-only composition with no internal rail, tabs, toolbar, status bar, or persistent controls.
- Cool near-black tonal layers and one cyan action/state accent.
- Explicit error recovery only when the PTY cannot reconnect.
- Directionality is scoped: application copy may be Arabic, but terminal geometry remains LTR.

**The Zero-Chrome Rule.** In the healthy state, every pixel inside the native window frame belongs to the terminal canvas. Product identity, shell labels, direction indicators, and shortcut legends do not compete with terminal output.

## Colors

The palette uses a cool-neutral ramp with cyan reserved for active capability and focus. Green, yellow, and red are semantic state colors only. Inactive surfaces never use full-saturation accents.

**The Semantic Accent Rule.** Cyan identifies focus, selection, and active direction support; it is forbidden as ambient decoration.

**The Contrast Rule.** Application text must meet WCAG 2.2 AA against its surface. Muted text is supporting information, never the sole carrier of an essential state.

## Typography

The terminal uses the configured mono stack with the bundled Noto Naskh Arabic font limited to Arabic Unicode ranges. Application chrome uses the same mono voice at a compact fixed scale. No display type, fluid heading scale, or decorative letter spacing is permitted.

**The Cell Integrity Rule.** Terminal font, size, line height, and letter spacing are part of rendering correctness. Changes require box-drawing, Arabic shaping, and cursor-position regression tests.

## Elevation

Twitty is flat by default. Depth is conveyed through tonal layers and single-pixel separators, not broad shadows or translucent glass. Temporary overlays may use a short, tight shadow only to clarify stacking.

**The Flat Workspace Rule.** The terminal canvas and status chrome never use decorative drop shadows.

## Components

### Terminal Workspace

- Fills the available window above the status area.
- Keeps terminal grid direction LTR and isolates bidirectional content at the rendering layer.
- Maintains visible keyboard focus without adding an ornamental frame.
- Shows an inline, non-modal recovery surface when the PTY cannot start.

### Recovery Notice

- Floats above the terminal bottom edge rather than replacing or resizing the terminal.
- States the failure in plain language, preserves diagnostic detail, and offers a keyboard-accessible retry action.

## Do's and Don'ts

### Do:

- **Do** keep the healthy surface visually identical to a normal terminal and make recovery controls contextual.
- **Do** test every visual change with Arabic, English, mixed text, ANSI color, box drawing, selection, and resize.
- **Do** preserve visible focus and readable labels when recovery UI is present.
- **Do** keep all product motion between 150–250ms and disable it under reduced-motion preferences.
- **Do** keep terminal geometry LTR even when surrounding application copy is Arabic.

### Don't:

- **Don't** build a flashy or gamified “AI terminal” with gradients, neon decoration, glass panels, or oversized chrome.
- **Don't** turn Twitty into a heavy IDE shell with persistent dashboards or sidebars.
- **Don't** apply full-page RTL in a way that reverses terminal geometry or Latin commands.
- **Don't** accept rendering that looks correct in a demo but breaks ANSI programs, cursor movement, selection, or interactive TUIs.
- **Don't** use gradient text, side-stripe callouts, broad ghost-card shadows, or card radii above 16px.
