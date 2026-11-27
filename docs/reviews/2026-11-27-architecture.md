# Architecture review, 2026-11-27

Scope: how easy Twitty is to maintain and how well it grows with more features, panels,
languages and contributors. Measured at `6f4653e` (round 11).

## What is in good shape

- **One rule, kept.** The grid stays logical and only drawing is bidirectional
  (`ARCHITECTURE.md`). Every Arabic fix so far fit inside `twitty-core::text` without touching
  the parser or the PTY.
- **Pure UI logic.** Menus, settings, palette, composer, review and welcome keep state,
  geometry and hit testing in window-free modules (`src/ui`, `src/composer`, `src/review`).
  This is why 750+ tests and 18 golden images run headless.
- **One command registry.** `commands.rs` names every action once. The palette, keybindings,
  control socket, context menus and tooltips all reuse it.
- **Safety nets.** CI runs clippy with `-D warnings`, tests on Linux and Windows, MSRV, golden
  images, fuzzing, `cargo-deny` and audit. `forbid(unsafe_code)` holds.
- **Crate edges are clean.** `twitty-core` depends on nothing in the workspace; the others
  depend only on core (plus control for ai). There are no cycles.

## Problems, ordered by cost to the next feature

### 1. Every overlay is wired by hand through six places (high)

Adding the right-click menu meant editing: a field in `App`, the key ladder in
`app/keyboard.rs`, the mouse ladder in `app/mouse.rs`, the motion handler, a forwarding setter
in `app/window.rs`, a field, setter and draw call in `render/mod.rs`, and fields plus text-run
clipping in `render/scene/frame.rs`.

Numbers: `on_key` checks 10 overlays in a fixed `if` ladder and `on_mouse_input` 12;
`app/window.rs` has 41 `set_*` forwarders; `Frame` has 44 fields, most of them a
`*_labels`/`*_rect` pair per overlay; `Renderer` has 38 fields.

Risks: the order of the ladders is the z-order, but it is written three times (keys, mouse,
drawing) and can drift. A new panel costs ~150 lines of plumbing before its own code.

Fix: a layer stack. Each overlay implements one trait:

```rust
trait Layer {
    fn key(&mut self, cx: &mut LayerCx, event: &KeyEvent) -> Handled;
    fn mouse(&mut self, cx: &mut LayerCx, event: MouseEvent) -> Handled;
    fn deadline(&self) -> Option<Instant>;
    fn scene(&self, cx: &mut SceneCx) -> LayerScene; // shapes, labels, holes
}
```

`App` keeps `Vec<Box<dyn Layer>>` in z-order; keys, mouse and drawing walk the same list.
`Frame` keeps a generic `Vec<LayerScene>`. The menu, notices and tooltip already draw this way
(`Frame::floating`), so they are the first examples. Migrate one overlay at a time:
consent, settings, palette, memory, welcome, block AI, review, agent panel, then the composer.

### 2. `App` is a god object (high)

`App` has 41 fields, and 26 files add methods to it. Every feature can reach every other
feature's state, so changes are hard to review and test without a window.

Fix: move each feature's state and logic into its own controller (`ComposerController`,
`ReviewController`, ...) that receives only what it needs (`&mut Workspace`, a `Services`
handle for clipboard, toasts, store and command dispatch). `App` becomes routing plus the
layer stack. This goes together with item 1.

### 3. The root crate holds 72% of the code (medium-high)

The root `twitty` crate has 64k lines; the five library crates together have 24k. Any UI
change recompiles all of it (about 70 s for a release build here), and the crate boundary does
not stop UI code from reaching into sessions or the control socket.

Fix, after item 1: split `twitty-ui` (pure state, geometry, hit testing: `src/ui`,
`src/composer`, `src/review`, `src/sidebar`) and `twitty-scene` (`src/render/scene`). Keep
`twitty` as the app shell (winit, sessions, wiring). Expect much faster incremental builds and
enforced layering.

### 4. Very large files (medium)

| File | Lines | Suggested split |
| --- | --- | --- |
| `src/ui/settings_panel.rs` | 4074 | one module per section, like `settings_commands.rs` |
| `src/ui/agent_panel.rs` | 2723 | transcript model / input / layout |
| `src/render/scene/settings.rs` | 2681 | follow the panel split |
| `src/control/cmux.rs` | 2055 | one module per method group |
| `src/render/scene/agent.rs` | 1759 | cards / input / header |

Settings also lists every setting in several places (`SettingId`, per-section arrays, value
read/write). A declarative table (id, section, label EN/AR, kind, get, set) would make a new
setting one entry.

### 5. Translations are inline (medium, high if a third language comes)

Texts are `tr("English", "عربي")` (107 calls), `language.text(..)` (169) and `title_ar` fields
(281). Fine for two languages. A third language, or a check that nothing is left untranslated,
needs a catalog: one `strings` module with an enum key per text and one table per language,
plus a test that every key has every language. The current interface language is a process
global (`language::tr`); that is correct for one window per process and must become
per-window if Twitty ever opens several windows in one process.

### 6. Test setup is copied (low-medium)

Each golden test repeats ~80 lines of font, palette, terminal, frame and offscreen setup.
A `GoldenScene::new(w, h, theme).draw(|frame, cx| ...).check(name)` helper would cut new
golden tests to about 15 lines and make more of them likely to be written.

### 7. Smaller items (low)

- 24 functions carry `#[allow(clippy::too_many_arguments)]`, mostly draw functions; pass a
  `DrawCx { ctx, style, font, language }` instead.
- `show_pane_warning` now shows a notice and ignores its pane id; rename to `warn` and drop the
  argument.
- `format_pane_warning` is only used by a test after the notices change.

## Scalability of the running app

Measured in the maturity round: 3 million lines in under a second, a 20 MB line without a
hang, under 200 MB of memory, PTY output batched with a 4 MiB bound and a 12 ms parse budget,
50 MB `cat` in about 0.5 s. Row shaping is cached. Nothing here blocks growth; keep the
"time a 50 MB cat" check for any PTY or scheduler change.

## Suggested order

1. Layer stack for overlays (item 1), starting with menu, notices, tooltip and consent.
2. Feature controllers out of `App` (item 2), one overlay at a time with item 1.
3. Split settings by section plus a settings table (item 4).
4. Golden test helper (item 6), so the refactors above get screenshots cheaply.
5. `twitty-ui` and `twitty-scene` crates (item 3).
6. String catalog (item 5) before adding any third language.

Each step lands on its own with green CI and no behavior change.

## Progress

- Step 1 done (`ea248ea`): `Layer::STACK` drives key, mouse and motion routing and drawing;
  `Frame` keeps ordered text layers.
- Step 2 started: one `OverlayViews` per frame (−16 forwarders), one `Appearance` (−12
  setters), and the context menu and tooltip as window-free models with effects. Next: the
  same for consent, palette, memory and welcome, then the larger panels.
- Step 3 done: `SETTINGS` table (one entry per setting, checked at compile time against the
  `SettingId` order); `ui/settings_panel` split into table, values, keys, pointer, layout and
  state; `render/scene/settings` split by section. Largest file there is now 1.2k lines.
- Step 4 done: `tests/support/golden.rs` holds fonts, the GPU target, comparison and
  `GoldenScene`; overlay goldens are 10 to 25 lines each, and all 18 references pass
  unchanged.
