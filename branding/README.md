# Twitty brand assets

The mark is a bidirectional prompt: two terminal chevrons converge on one
cursor. Cyan represents the active terminal direction, neutral white keeps the
opposing direction readable, and the small green cursor is the only success
accent. The silhouette is deliberately geometric so it remains recognizable
at Windows taskbar and Start Menu sizes.

Source files:

- `twitty-mark.svg` — canonical square app mark.
- `installer-sidebar.svg` — source for the NSIS 164×314 welcome/finish image.
- `installer-header.svg` — source for the NSIS 150×57 page header image.

Generated files under `src-tauri/icons/` and `src-tauri/installer-assets/` are
release artifacts derived from these SVG sources.

The NSIS installer uses a current-user install by default. This keeps setup
free of an elevation prompt while still exposing the install directory,
Start Menu shortcut, optional desktop shortcut, and launch-on-finish choices.
