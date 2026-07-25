# Product

## Register

product

## Users

Windows developers who work in PowerShell and interactive terminal tools such as Antigravity, often mixing Arabic and English in the same prompt or response. They need the terminal to disappear into the workflow rather than becoming another interface to manage.

## Product Purpose

Twitty is a Windows-native terminal whose TUI geometry, ANSI color, keyboard behavior, and responsiveness match a dependable system terminal while making Arabic shaping and right-to-left reading usable. Success means a CLI renders with the same structure and timing it has in Windows Terminal, with better Arabic and no protocol corruption.

## Brand Personality

Native, precise, quiet. Twitty should feel like a serious Windows developer tool: familiar at first glance, visually stable during long sessions, and confident without decorative product chrome.

## Brand Identity

Twitty's mark is a bidirectional prompt: two opposing terminal chevrons meet
around one cursor. It communicates mixed RTL/LTR work without copying the
Windows Terminal prompt icon, using letters, or relying on detail that
disappears at taskbar size.

## Anti-references

- Browser-styled “terminal” mockups whose font metrics or colors differ from native terminal behavior.
- Persistent status bars, badges, rails, loading pages, or product decoration inside the terminal canvas.
- Arabic fixes that rewrite PTY data, move cursor cells, break ANSI styles, or distort TUI borders.
- Flashy AI-tool aesthetics, gradients, glass, neon glow, oversized controls, or gratuitous motion.
- Cross-platform compromises that make the primary Windows experience less exact.

## Design Principles

1. Preserve terminal protocol and cell geometry before adding visual correction.
2. Match Windows terminal conventions for color, density, keyboard behavior, and startup.
3. Scope RTL to Arabic visual runs and completed Arabic prose; keep prompts, the terminal grid, and Latin commands LTR.
4. Validate changes in real PowerShell and TUI programs, not only browser demos or unit fixtures.
5. Keep the healthy application surface invisible: the user should see their terminal, not Twitty.
6. Keep setup familiar and reversible: no elevation for a normal install,
   explicit location and shortcut choices, and standard Windows controls.

## Accessibility & Inclusion

Recovery UI meets WCAG 2.2 AA, is keyboard reachable, and never relies on color alone. The terminal supports keyboard-only operation, reduced-motion preferences, readable Arabic diacritics, mixed Arabic/English content, and stable focus/cursor behavior at common Windows DPI scaling levels.
