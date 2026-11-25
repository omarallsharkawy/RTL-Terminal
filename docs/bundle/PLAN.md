# The Twitty bundle: cmux + Warp ideas on an RTL-first terminal

Twitty stays what it is (a native Rust/wgpu terminal whose one rule is *the grid is logical,
only drawing is bidirectional*) and gains the features that make cmux and Warp good for
working with coding agents. **No code is copied from either project**: Warp is AGPL-3.0 and
cmux is GPL-3.0 (and Swift/AppKit, macOS only), while Twitty was MIT OR Apache-2.0. Features are
re-implemented from their public behaviour and documentation only. Do not open, paste or
paraphrase their source.

**Update (round 3):** Twitty is now AGPL-3.0-or-later, so code from cmux-gtk (AGPL), cmux (GPL)
and Warp (AGPL) may be taken, with attribution in `licenses/NOTICE.md` and a header comment in
each file. Rounds 1 and 2 were written from scratch under the rule above.

## What we take

| From | Feature | Lane |
| --- | --- | --- |
| cmux | Vertical sidebar of workspaces (tabs): title, cwd, git branch, running/exit state, unseen notification | S |
| cmux | Notifications from agents (OSC 9 / 99 / 777): attention ring, `Ctrl+Shift+U` jumps to it, desktop notification | S |
| cmux | Control socket + `twitty ctl` CLI so agents and scripts drive the window | N |
| Warp | Command blocks from OSC 133: separators, exit status, duration, jump, copy output | O |
| Warp | Automatic shell integration for bash, zsh, fish | O |
| Warp | Command palette (`Ctrl+Shift+P`), Arabic and English titles | L |
| Warp | "Ask an agent" about a block (`Ctrl+Shift+A`), runs a local agent CLI in a split | L |

Lanes: **S** = Sol (Codex, gpt-6.2-sol), **N** = Neon (Antigravity, gemini-4-neon),
**O** = Opus 5.5 agent, **L** = lead (integration, review, palette, agent).

## Foundation (done on branch `bundle`, everyone builds on it)

- `src/shell_integration.rs`: parses OSC 7 / 9 / 99 / 133 / 633 / 777 into `Signal`s.
  `Terminal::process_bytes` captures them (payloads up to 4 KiB, cleaned of control and bidi
  characters) and `Terminal::take_shell_events()` returns `ShellEvent { signal, line, column,
  history }`, stamped with the cursor position when the sequence ended.
- `App::on_shell_event` (in `src/app/event.rs`) updates `Pane::meta` (`PaneMeta`: `cwd`,
  `attention`, `running`, `last_exit`, `last_command`) and calls `App::notify` for
  notifications. A notification is cleared when its pane is drawn focused in a focused window.
- `src/commands.rs`: every command by stable name (`split.right`, `block.copy-output`…) with
  English and Arabic titles. Palette and socket both go through `commands::action(name)` and
  `App::run_action`.
- New `InputAction`s with shortcuts already bound (`src/input/keys.rs`): `SelectTab(n)`
  (Ctrl+Shift+1..9), `ToggleSidebar` (Ctrl+Shift+B), `NextAttention` (Ctrl+Shift+U, works),
  `CommandPalette` (Ctrl+Shift+P), `PrevBlock`/`NextBlock` (Ctrl+Shift+Up/Down),
  `CopyBlockOutput` (Ctrl+Shift+Y), `AskAgent` (Ctrl+Shift+A). Unimplemented ones are no-ops in
  `App::run_action` (`src/app/keyboard.rs`), marked for their lane.
- `pty::SpawnOptions { cwd, command, env }` and `Pty::spawn_with`; `ShellFactory::options`.
  Every pane's process gets `TWITTY_PANE_ID`.
- Config (`src/config.rs`): `sidebar` ("right" | "left" | "hidden"), `sidebar_width`,
  `desktop_notifications`, `control_socket`, `shell_integration`, `agent_command`.

## File ownership

Each lane works in its own git worktree, branched from `bundle`, and owns the files listed.
Touch anything else only where your feature must hook in, and keep those edits small (a call,
a field, a match arm) so merging stays easy. Never reformat files you do not own.

| Lane | Owns (new unless noted) | Hooks allowed in |
| --- | --- | --- |
| S | `src/sidebar/`, `src/render/scene/sidebar.rs`, `src/notify.rs`, `src/layout.rs` (existing) | `render/scene/frame.rs`, `render/scene/mod.rs`, `app/window.rs`, `app/mouse.rs`, `app/keyboard.rs` (ToggleSidebar arm), `app/event.rs` (`notify`) |
| N | `src/control/`, `src/app/control.rs`, `src/main.rs` (existing) | `app/mod.rs` (AppEvent arm, startup), `app/event.rs` (AppEvent variant), `lib.rs` |
| O | `src/blocks/`, `src/render/scene/blocks.rs`, `assets/shell/`, `src/session/shell_setup.rs`, `src/pty.rs` (existing) | `app/event.rs` (Mark arm), `app/keyboard.rs` (block arms), `render/scene/frame.rs`, `session/pane.rs` (a `blocks` field), `session/spawn.rs` |

## Rules for every lane

- Rust only, `#![forbid(unsafe_code)]` stays. New dependencies only if small, MIT/Apache, and
  justified in your report. No network access at run time.
- Arabic is the point of this terminal: any text you draw (titles, cwd, notification bodies,
  command lines) goes through the existing shaping (`render::shape_label` / the row shaper) so
  Arabic joins and runs right to left. Check it with an Arabic title or path in a test.
- Text from programs is untrusted: clean it (`shell_integration::clean_text`), bound its
  length, never pass it to a shell. Spawn processes with argument vectors, not `sh -c`.
- Keep the UI thread fast: no blocking I/O or process waits on the event loop. File reads
  (git HEAD) are small and cached; anything slower runs on a thread and comes back as an
  `AppEvent`.
- Tests for every behaviour, window-free where possible (see `tests/`).
- Gates (all must pass, run from the worktree root):

  ```
  export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
  cargo fmt -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```

  The disk is nearly full: build with the two variables above and do not create other
  target directories.
- Do not commit; the lead reviews and commits. Do not touch `main`.
