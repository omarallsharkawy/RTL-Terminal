---
name: Twitty
description: A calm, terminal-first desktop shell for dependable Arabic and English workflows.
colors:
  terminal-bg: "#0c0c0c"
  border-subtle: "#767676"
  ink-primary: "#cccccc"
  ink-muted: "#767676"
  accent: "#3a96dd"
  success: "#13a10e"
  warning: "#c19c00"
  danger: "#c50f1f"
typography:
  terminal:
    fontFamily: "Cascadia Mono, Cascadia Code, Consolas, Segoe UI, Noto Naskh Arabic, monospace"
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

## Brand Mark

The mark uses two opposing terminal chevrons around one cursor. Cyan carries
the active direction, neutral white carries the opposing direction, and green
is reserved for the cursor. The geometry must remain recognizable at 16px;
wordmarks, thin strokes, gradients, shadows, and extra decoration are excluded.

**The Small-Silhouette Rule.** Every icon change is reviewed at 16, 24, 32,
48, and 256px. If the mark requires the word “Twitty” to be understood, it is
not an app icon.

## Colors

The palette is Windows Terminal's Campbell palette so ANSI applications retain the same semantic colors they use in the system terminal. Cyan identifies active capability and focus; green, yellow, and red remain semantic state colors.

**The Semantic Accent Rule.** Cyan identifies focus, selection, and active direction support; it is forbidden as ambient decoration.

**The Contrast Rule.** Application text must meet WCAG 2.2 AA against its surface. Muted text is supporting information, never the sole carrier of an essential state.

## Typography

The terminal uses Cascadia Mono/Consolas for the Windows grid, with Segoe UI and bundled Noto Naskh Arabic as Arabic fallbacks in the same xterm-measured stack. Arabic runs keep xterm's allocated span width but override its ligature compensation to zero letter spacing; xterm's compensation is designed for short programming ligatures and otherwise creates 15–25px Arabic word gaps. A scoped `-0.45ch` word-spacing correction removes the excess monospace cell width while leaving a readable gap. Latin cells retain zero kerning and zero optional ligatures. Application chrome uses the same mono voice at a compact fixed scale. No display type, fluid heading scale, or decorative letter spacing is permitted.

**The Cell Integrity Rule.** Terminal font, size, line height, and letter spacing are part of rendering correctness. Changes require box-drawing, Arabic shaping, and cursor-position regression tests.

## Elevation

Twitty is flat by default. Depth is conveyed through tonal layers and single-pixel separators, not broad shadows or translucent glass. Temporary overlays may use a short, tight shadow only to clarify stacking.

**The Flat Workspace Rule.** The terminal canvas and status chrome never use decorative drop shadows.

## Components

### Terminal Workspace

- Fills the entire available window.
- Keeps terminal grid direction LTR and isolates bidirectional content at the rendering layer.
- Maintains visible keyboard focus without adding an ornamental frame.
- Shows an inline, non-modal recovery surface when the PTY cannot start.

### Recovery Notice

- Floats above the terminal bottom edge rather than replacing or resizing the terminal.
- States the failure in plain language, preserves diagnostic detail, and offers a keyboard-accessible retry action.

### Windows Installer

- Uses the standard NSIS Modern UI flow so controls and keyboard behavior are
  familiar to Windows users.
- Offers English and Arabic before the welcome page.
- Installs for the current user by default, avoiding an elevation prompt.
- Shows install location and Start Menu choices before copying files.
- Leaves desktop shortcut creation off by default, while keeping it available
  on the finish page.
- Uses the same app mark for the executable, installer, uninstaller, language
  selector, Start Menu, and desktop shortcuts.
- Uses branded 164×314 sidebar artwork on welcome/finish and restrained 150×57
  header artwork on task pages.

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
