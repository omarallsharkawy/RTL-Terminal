# Twitty's look (round 4)

The goal: Twitty looks like a modern agent workspace, taking cmux's layout and Warp's
polish, while staying an RTL-first terminal (Arabic UI text right-aligned, the grid logical).
cmux-gtk code may be adapted (AGPL, see licenses/NOTICE.md); Warp is a visual reference only.
No cmux/Warp names, logos or brand colours.

## Layout

```
┌──────────────────────────── top bar (36 px) ─────────────────────────────┐
│ [≡]        [ 🔍 Search sessions, agents, files… / دوّر…   ]   ±+413 −56  🔔2 │
├───────────────┬──────────────────────────────────────────┬───────────────┤
│ sidebar       │ ┌ pane header (28 px): icon title  ⊟ ⊞ ✕ ┐ │ review panel  │
│ (workspace    │ │ terminal grid                         │ │ (optional,    │
│  cards)       │ └───────────────────────────────────────┘ │  right side)  │
└───────────────┴──────────────────────────────────────────┴───────────────┘
```

- **Top bar** (Warp): sidebar toggle; a search field that opens the command palette
  (click or `Ctrl+Shift+P`); on the right the focused pane's git diff stat (`±+413 −56`, opens
  the review panel) and a bell with the unread notification count (opens notification list).
- **Sidebar** (cmux): one card per tab ("workspace"), see below. When the sidebar is shown
  the old tab bar is hidden (the sidebar already lists the tabs).
- **Pane header** (cmux): every pane gets a slim header: program icon, title (command or
  agent name), and buttons split right, split down, close. Focused pane: brighter header and a
  1 px accent outline. A pane that needs attention (agent "needs input", unread notification)
  gets an accent **ring** around it until focused.
- **Review panel** (Warp): a right-side panel with the focused repo's uncommitted changes.

## Workspace card (sidebar)

```
 ● 2  Review and merge PR             ← unread badge, title (UI font, semibold 13)
      Claude is waiting for your input ← latest notification / agent status (2 lines, dim)
      ⎇ main • ~/fun/cmux  :3000       ← branch • cwd (mono 11, dimmer), ports
```
- Selected card: accent background (rounded 8 px), light text. Hover: subtle raise (bg3).
- Agent state pill on the title line: working (animated dots), needs input (orange),
  done (green check), error (red).
- Arabic titles/text right-aligned (existing per-line direction rule); the card layout mirrors
  when the sidebar is on the right.

## Design tokens (one place: `src/render/ui_style.rs`)

- Surfaces derived from the theme background: `bg0` window, `bg1` sidebar/top bar,
  `bg2` cards/headers, `bg3` hover, `border` (fg 10 %), `accent` (theme cursor colour, or
  `ui_accent` config), `text`, `text_dim` (60 %), `text_faint` (40 %), `danger`, `warn`, `ok`.
- Radii 6 / 8 / 12 px; overlay shadow 16 px blur, 30 % black; spacing scale 4/8/12/16/24.
- Fonts: UI text in a proportional sans (config `ui_font_family`, default: system sans via
  fontconfig, Arabic through the bundled Noto Naskh Arabic fallback); code/paths in the
  terminal mono font. Sizes 11 / 12 / 13 / 15 px × scale.
- Icons: drawn from SDF primitives (capsule lines, circles, rounded rects), no icon font or
  image files: menu, search, bell, split-right, split-down, close, git-branch, folder, plus,
  check, warning, terminal, agent (sparkle), diff.

All chrome scales with the window's scale factor and respects light themes (tokens derive from
the theme, never hard-coded dark).

The shared APIs are `render::ui_style::UiStyle`, `render::shapes::{Shape, ShapeLayers}`,
`render::icons::Icon` and `render::text::ui::{UiFont, UiLabel, UiSize, UiWeight}`. Use
`UiStyle::px` for logical dimensions and `DrawContext::color` for shape colors. Add UI shapes to
`Frame::shapes.chrome` or `Frame::shapes.overlay` in paint order. Glyphon runs after both layers,
so clip text runs behind an overlay with `scene::frame::bounds_around`.
