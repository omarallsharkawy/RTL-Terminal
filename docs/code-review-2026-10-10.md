# Twitty v2 (native Rust) — Code Review & Fix Plan
Date: 2026-10-10 · Scope: src/*.rs, tests/, CI, docs (legacy-web excluded)
Note: findings verified by reading code; could not compile locally (crates.io blocked in review sandbox).

## Critical
1. terminal.rs:110,128 — snapshot ignores display_offset; scrolled rows have negative line → dropped. Scrollback is broken.
2. terminal.rs:118 — only cell.c copied; zerowidth (Arabic harakat / combining marks) dropped.
3. renderer.rs:358,474-477 — trailing spaces trimmed from segment; cursor snaps to text end → cursor one+ cell left after "PS C:\> ".
4. renderer.rs:310,376 — cell flags INVERSE/DIM/HIDDEN/UNDERLINE/STRIKEOUT ignored (reverse video broken in TUIs).
5. renderer.rs:330-440 — whole segment shaped proportionally; any fallback glyph (icons, ✓, emoji, CJK, Arabic) shifts later cells off-grid. WIDE_CHAR_SPACER cells add extra spaces.
6. app.rs:333-341 — paste ignores bracketed paste mode; multi-line paste executes immediately (security).
7. pty.rs:107 — Windows always picks %COMSPEC% (cmd.exe), never PowerShell; contradicts README/PRODUCT.

## High
8. input.rs — missing: Alt/Meta (ESC prefix), F1-F12, modifiers on arrows/Home/End (Ctrl+Left etc.), Ctrl+M, Ctrl+Space, Ctrl+/, Ctrl+Backspace.
9. app.rs:342 — Copy is a no-op; no selection at all.
10. No DPI handling (scale_factor / ScaleFactorChanged) — tiny text at 125-150%.
11. app.rs:157-166 — PtyExit respawns forever: `exit` never closes, crash-loop if shell dies at start, old screen not reset.
12. renderer.rs:506 — block cursor drawn under text in same fg color → char under cursor near-invisible; no cursor shapes/blink/unfocused.
13. CI (build.yml) still builds legacy Tauri at src-tauri/ + root npm → native v2 tests never run; README/DESIGN describe old xterm app; palette is Tokyo Night but DESIGN says Campbell.

## Medium
14. config.rs:18 — uses HOME; unset on Windows → zoom never saved. Use %APPDATA%.
15. app.rs:264-268 — trackpad PixelDelta: every event scrolls 3 lines; no accumulation.
16. app.rs:189-203 — mouse motion sent on every pixel (not cell change); MOUSE_DRAG (1002) ignored; always SGR encoding; coords not clamped.
17. Typing while scrolled doesn't return to bottom.
18. renderer.rs:263-267 — Lost/Outdated surface not reconfigured → frozen frame.
19. terminal.rs ForwardListener ignores Title, ColorRequest (OSC 10/11 queries never answered), Bell, ClipboardStore.
20. renderer.rs:100 — GPU adapter .expect() panics; no fallback adapter.
21. pty writes on UI thread (big paste can freeze UI); config saved to disk every zoom step.
22. Font is generic Monospace, not Cascadia Mono/Consolas; line height 1.55 vs WT ~1.2.

## Low
23. quad.rs: uniform screen_size never set at init (masked by first Resized); new vertex buffer every frame.
24. renderer.rs:385 — attrs_match ignores italic.
25. Box drawing missing double lines ═║╔, shades ░▒▓, eighth blocks ▁▂, braille, rounded/dashed; fixed 1.5px non-pixel-aligned lines.
26. main.rs declares modules again instead of using lib crate; TWITTY_SHELL can't take args; LANG forced to en_US.UTF-8 on Unix.

## Plan
Phase 0 — foundation: fix CI for native crate (fmt/clippy/test on windows), update README/DESIGN/CHANGELOG 2.0.0, regression tests for every bug below.
Phase 1 — correctness: #1 scrollback, #2 harakat, #3 cursor, #4 cell flags, #5 grid-locked glyph placement + wide chars, #12 cursor rendering, #7 shell selection.
Phase 2 — input & safety: #6 bracketed paste, #8 full key encoding, #9 selection + copy, #11 exit/respawn policy, #15-17 mouse/scroll.
Phase 3 — Windows polish: #10 DPI, #14 config path, #22 fonts/line height, #18/#20 GPU robustness, #19 terminal events (title, OSC replies, bell).
Phase 4 — performance: PTY writer thread, persistent vertex buffer, lazy font load, drop Mutex, debounce config save.
Phase 5 — "beast" features: tabs/splits, config file (themes, fonts, keybinds), search in scrollback, hyperlinks (OSC 8), ligatures, full box/braille set, Kitty keyboard protocol, IME.

## Status (updated 2026-10-10)
Built and tested locally (Linux): `cargo fmt --check`, `cargo clippy --all-targets -D warnings` and `cargo test` all pass. Regression tests live in `tests/review_regressions.rs`.

Done: #1 scrollback · #2 harakat/combining marks · #3 cursor after trailing spaces · #4 INVERSE/DIM/HIDDEN/underline/double underline/strikeout · #5 LTR glyphs grid-locked (only segments that drift get split per cell) and wide-char spacers · #6 bracketed paste (ESC stripped) · #7 pwsh → powershell → %COMSPEC% · #8 Alt/Meta, F1–F12, modified arrows/Home/End/PgUp/PgDn/Ins/Del, Ctrl+Space / Ctrl+/ / Ctrl+M / Ctrl+Backspace, AltGr passthrough · #10 DPI (scale factor + ScaleFactorChanged) · #11 window closes when the shell exits · #12 cursor shapes, glyph under block cursor recolored, hollow when unfocused · #13 CI builds/tests the native crate (README/DESIGN not updated yet) · #14 %APPDATA% / XDG_CONFIG_HOME · #15 wheel/trackpad accumulation · #16 motion only on cell change, 1002 drag, legacy X10 encoding, clamped coords, modifier bits, Shift bypass · #17 input scrolls back to bottom · #18 Lost/Outdated surface reconfigured · #19 title, bell, OSC 4/10/11/12 color replies, OSC 52 store (load refused), text area size · #20 software adapter fallback · #21 PTY writer thread, config saved on exit · #23 · #24 · #26 (main uses lib crate, TWITTY_SHELL args, LANG only if unset).

Still open: #9 selection + copy · #13 docs (README/DESIGN/CHANGELOG) · #22 font family / line height · #25 extra box-drawing glyphs · cursor blink · Phase 4–5.
