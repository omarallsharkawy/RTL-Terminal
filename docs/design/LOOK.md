# Twitty Look v2

Twitty is an Arabic-first terminal workspace. Terminal output has the strongest contrast.
Use small changes in surface tone to group controls, and reserve the accent for focus,
selection and attention. The terminal grid stays logical while drawing handles bidi text.

## Window chrome

- The **36 px top bar** has a flat sidebar toggle, a centered search control and trailing
  git counts and notifications. The search field opens the command palette by click or
  `Ctrl+Shift+P`. It shows the placeholder in the UI language and shortens it on narrow
  windows. Show the shortcut keycap when there is room. The git
  counts open code review. The bell opens notification history and shows its unread count.
- **The workspace sidebar** replaces the horizontal tab strip when visible. When it is hidden,
  the tab strip keeps a pill for each tab, a hover close control and a plus button.
- **24 px pane headers** have a program icon, one title and split-right, split-down and close
  controls. Idle titles are dimmed and control icons brighten on hover. The focused title
  uses the normal foreground and a one-pixel accent rule marks the header's bottom edge.
  Unread attention can mark the pane perimeter with a fine ring. A pane flash temporarily
  draws a stronger perimeter highlight.
- **Code review** uses the same surfaces and hairlines. Its header contains the branch,
  file and line counts, refresh and close icons and the Commit action. Files have rounded
  rows and hunk headings have quieter bars. Diff lines use terminal shaping with a dim
  gutter, green/red tints and an accent selection outline.

## Workspace cards

```text
 Terminal rendering                    1
 /src/twitty · look-v2         · Working
 cargo test --test bidi_layout_tests
```

- Give the single-line, 13 px semibold title its own row. Keep a whole-word prefix when
  possible and use whole graphemes for long words. A bare ellipsis is never a title.
- Use an 11 px dim monospace metadata line. Put the working directory first and the git
  branch after a middle dot. Keep the directory tail when abbreviating. Remote state and
  listening ports use available room on this row.
- Align the small status glyph and its 11 px caption to the trailing edge on the
  metadata line. Working uses animated dots, needs input a warning, done a check and error
  a cross. Keep the glyph by itself when space is tight.
- Show a single optional 12 px third line for a notification, agent message or last command.
  Titles and messages keep their text direction within labels anchored to the card's start edge.
  The unread badge follows the title in the UI direction. English starts at the left on either
  sidebar placement, and Arabic starts at the right.
- Idle rows sit on the sidebar surface. Hover uses a subtly raised surface. Selection uses
  a slightly stronger surface and a slim accent marker at the leading edge. Text retains the
  theme foreground. Hover reveals the close control. The list scrolls and supports dragging
  to reorder. The final row is a quiet, rounded **New tab** button.

## Command blocks

- Draw command blocks behind the terminal rows. Blend a faint, borderless tint over the
  terminal background, with a slim 2 px
  leading status strip, 6 px corners and 4 px between adjacent blocks. The tint and spacing
  are visual and leave row and column positions intact.
- Place the floating action toolbar on the trailing edge on hover. Duration and exit status
  can appear at that edge in clear space. The toolbar and status strip mirror in Arabic UI,
  while the command and its output keep the terminal's per-row bidi behavior.

## Shared tokens

- Derive all surfaces from the terminal background. `bg0` is the terminal, `bg1` is the
  chrome and faint block tint, `bg2` is a small lift for controls and hover, and `bg3`
  gives selection and interactive controls slightly more definition. The steps blend 3.5%,
  6.8% and 12% of the foreground into the background, darkening light themes and lightening
  dark themes. Hairlines are 14% foreground, snapped from 1 logical px. The dim and faint
  foregrounds use 69% and 48%. The accent comes from the theme cursor or `ui_accent`,
  and status colors come from the active ANSI palette.
- Apply window opacity once to the composed terminal and chrome material before drawing text.
  Overlapping fills and hairlines share the same opacity as the surrounding material.
- Use the 4 px spacing grid (4, 8, 12, 16, 24 px) and 6, 8 or 12 px radii. Thin rules,
  strokes and status markers are sized separately. Chrome dimensions and type scale with DPI.
- UI text uses bundled Inter for Latin and IBM Plex Sans Arabic for Arabic. Paths, branches
  and terminal content use the terminal mono stack. Use the existing 11, 12, 13 and 15 px
  type sizes and SDF glyphs for controls.
- **Twitty Dark** uses warm graphite, pale neutral text and a muted copper accent.
  **Twitty Light** uses warm paper, deep graphite text and a darker copper accent. Both
  retain contrast for Arabic letterforms and diacritics. Set `theme` to `twitty-dark` or
  `twitty-light`. An unset theme defaults to `twitty-dark`. Named built-in themes share
  the same chrome derivation.
- Hover fades take 120 ms. Panel slides, tab selection and sidebar reorder settle in 160 ms.
  `reduce_motion` skips chrome animation and holds the working indicator steady.

Build with `render::ui_style::UiStyle`, `render::shapes::{Shape, ShapeLayers}`,
`render::icons::Icon` and `render::text::ui::{UiFont, UiLabel, UiSize, UiWeight}`.
Use `UiStyle::px` for scaled dimensions and `DrawContext::color` for shapes. Glyphon draws
text after quads and shapes. Clip runs behind chrome overlays using
`scene::frame::bounds_around`.

## Offscreen showcase

Run `TWITTY_SHOWCASE_DIR=/tmp/twitty-showcase cargo test --test showcase_tests` to export both
themes as 1280×800 at 1× (English) and 951×1002 at 1.5× (Arabic). Each has a right sidebar
and an empty composer. All four scenes are composited over a grey checker at 100%, 78%,
and 50% opacity. A thirteenth PNG samples the palette entrance at 78%. The showcase uses
the production frame builder without a window.
