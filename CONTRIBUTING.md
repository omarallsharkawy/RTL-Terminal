# Contributing to Twitty

Thanks for helping. Twitty's one hard promise is that Arabic and mixed text render correctly
without changing what programs see, so most of this guide is about not breaking that.

Read [ARCHITECTURE.md](ARCHITECTURE.md) first; it is short and explains where things live.

## Setup

```bash
git clone https://github.com/omarallsharkawy/RTL-Terminal.git
cd RTL-Terminal
cargo run --release
```

Rust 1.90 or newer. No system libraries are needed beyond what winit and wgpu need on your
platform; tests run without a GPU.

## Before you open a pull request

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --release
```

CI runs the same on Linux and Windows, checks the minimum Rust version, and checks
dependency licenses and advisories.

## How changes are made here

- **Bug fixes start with a failing test.** Reproduce the bug in `tests/` (see the table below),
  watch it fail, then fix it. The test stays as a regression guard.
- **Keep the grid logical.** Never reorder or rewrite terminal cells to make text look right; fix
  the drawing instead. See [ADR 0001](docs/adr/0001-grid-is-source-of-truth.md).
- **Small, focused commits** with [Conventional Commit](https://www.conventionalcommits.org/)
  messages: `fix(render): …`, `feat(input): …`, `refactor(session): …`, `test: …`, `docs: …`.
- **Match the surrounding code**: naming, comment density, module doc comments (`//!`) on every
  file explaining its job.
- **User-visible changes** get a line under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md), and
  README updates when they change shortcuts or config.
- **Decisions that are expensive to reverse** get an ADR in [docs/adr/](docs/adr/).

## Where tests go

| You changed | Test it with |
| --- | --- |
| Arabic/bidi layout, extents, mouse hits | `tests/bidi_layout_tests.rs`, `tests/arabic_bidi_tests.rs` |
| Selection and copy | `tests/selection_and_rtl_tests.rs` |
| Key encoding | `tests/kitty_keyboard_tests.rs`, `tests/input_*` |
| Shortcuts, tabs, panes | `tests/pane_shortcut_tests.rs`, `tests/layout_tests.rs` |
| Box drawing | `tests/box_block_tests.rs` |
| Terminal behavior (escape sequences) | `tests/terminal_ansi_tests.rs` |
| Config | `tests/core_audit_tests.rs` (`config` module) |

## Manual checklist for rendering and input changes

Some things only show up in the real window. When you touch `render/`, `app/` or `input/`, run
`cargo run --release` and check:

- [ ] `printf 'مرحبا بالعالم 7abaf40 2026 ✓\n'` reads right to left, the hash and numbers whole.
- [ ] Typing Arabic at the prompt: the cursor stays at the left end of what you typed.
- [ ] Mouse-select an Arabic word from either side; the copied text is that word.
- [ ] A colored Arabic line (`printf '\e[43;30m بس فيه حاجة صغيرة: \e[0m\n'`) has no holes.
- [ ] `htop` or `lazygit`: borders join, no gaps.
- [ ] Split panes (Ctrl+Shift+E / O), drag a divider, close panes (Ctrl+Shift+W).
- [ ] An input method (fcitx5/ibus/Windows IME) composes inline at the cursor.

## Reporting bugs

Include your OS and compositor (e.g. Wayland/niri, X11, Windows 11), the output of
`twitty --version` if available or the release you installed, a screenshot, and if possible the
exact bytes that reproduce it (`printf '…'`).

## License

By contributing you agree that your contribution is dual licensed under MIT OR Apache-2.0, as
described in the README.
