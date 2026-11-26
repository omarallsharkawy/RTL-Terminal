# Third-party code in Twitty

Twitty is AGPL-3.0-or-later. Some files contain code taken or adapted from the projects
below. Each file that does says so in its header comment, with the upstream path.

| Project | License | Copyright | Used in |
|---|---|---|---|
| [cmux-gtk](https://github.com/douglas/cmux-gtk) | AGPL-3.0-or-later ([cmux-gtk.txt](cmux-gtk.txt)) | Douglas Soares de Andrade | `src/control/cmux.rs`, `src/control/claude_hook.rs`, `assets/bin/claude`, `src/ui/search_all.rs`, `src/remote/bootstrap.rs`, `src/remote/daemon/server.rs` |
| [cmux](https://github.com/manaflow-ai/cmux) | GPL-3.0-or-later ([cmux.txt](cmux.txt)); its `web/` and server directories are BUSL-1.1 and are not used | Manaflow, Inc. and contributors | (none yet) |
| [Warp](https://github.com/warpdotdev/warp) | AGPL-3.0 ([warp-AGPL.txt](warp-AGPL.txt)); `warpui_core` and `warpui` under MIT ([warp-MIT.txt](warp-MIT.txt)) | Denver Technologies, Inc. | (none yet) |

Code from GPL-3.0-or-later sources is combined with Twitty under section 13 of the GPL v3 /
AGPL v3. The names and logos of these projects are not used.

## Bundled UI fonts

| Source | License | Copyright | Bundled files |
|---|---|---|---|
| [Inter 4.001](https://github.com/rsms/inter) | SIL OFL 1.1 ([Inter-OFL.txt](Inter-OFL.txt)) | The Inter Project Authors | `TwittyLatinUI-{Regular,SemiBold}.ttf` |
| [IBM Plex Sans Arabic 1.101](https://github.com/IBM/plex) | SIL OFL 1.1 ([IBM-Plex-OFL.txt](IBM-Plex-OFL.txt)) | IBM Corp. | `TwittyArabicUI-{Regular,SemiBold}.ttf` |

The renamed subsets in `assets/fonts/` retain Latin or Arabic script characters for UI text,
common punctuation, and the OpenType tables for shaping. The Arabic subset also keeps basic
Latin for punctuation and numerals used alongside Arabic.

## Bundled libraries

| Library | License | Used in |
|---|---|---|
| [SQLite](https://www.sqlite.org/copyright.html) (via `rusqlite` / `libsqlite3-sys`, MIT) | Public Domain | `crates/twitty-store` |
