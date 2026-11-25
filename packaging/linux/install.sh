#!/usr/bin/env sh
# Install (or uninstall) Twitty from an extracted Linux release tarball.
# Defaults to ~/.local; pass --system (or --prefix /usr/local) for a system-wide install.
set -eu

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
UNINSTALL=0

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Install Twitty binaries, desktop entry, icons, metainfo, and licenses.

Options:
  --prefix DIR    Install prefix (default: \$PREFIX or ~/.local)
  --system        Install to /usr/local (may require sudo)
  --uninstall     Remove files previously installed under the prefix
  -h, --help      Show this help message
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --prefix)
      [ "$#" -ge 2 ] || { echo "error: --prefix requires a directory" >&2; exit 2; }
      PREFIX="$2"
      shift 2
      ;;
    --prefix=*)
      PREFIX="${1#--prefix=}"
      shift
      ;;
    --system)
      PREFIX="/usr/local"
      shift
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

refresh_caches() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
  fi
  if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
  fi
}

if [ "$UNINSTALL" -eq 1 ]; then
  rm -f \
    "$PREFIX/bin/twitty" \
    "$PREFIX/bin/twitty-remote" \
    "$PREFIX/share/applications/twitty.desktop" \
    "$PREFIX/share/metainfo/dev.twitty.Twitty.metainfo.xml" \
    "$PREFIX/share/icons/hicolor/scalable/apps/twitty.svg"
  for size in 16 24 32 48 64 128 256 512; do
    rm -f "$PREFIX/share/icons/hicolor/${size}x${size}/apps/twitty.png"
  done
  rm -rf "$PREFIX/share/licenses/twitty"
  refresh_caches
  echo "Uninstalled Twitty from $PREFIX"
  exit 0
fi

if [ ! -f "$SCRIPT_DIR/bin/twitty" ] || [ ! -f "$SCRIPT_DIR/bin/twitty-remote" ]; then
  echo "error: expected bin/twitty and bin/twitty-remote next to $(basename "$0")" >&2
  exit 1
fi

install -Dm755 "$SCRIPT_DIR/bin/twitty" "$PREFIX/bin/twitty"
install -Dm755 "$SCRIPT_DIR/bin/twitty-remote" "$PREFIX/bin/twitty-remote"

if [ -f "$SCRIPT_DIR/share/applications/twitty.desktop" ]; then
  install -Dm644 "$SCRIPT_DIR/share/applications/twitty.desktop" \
    "$PREFIX/share/applications/twitty.desktop"
fi
if [ -f "$SCRIPT_DIR/share/metainfo/dev.twitty.Twitty.metainfo.xml" ]; then
  install -Dm644 "$SCRIPT_DIR/share/metainfo/dev.twitty.Twitty.metainfo.xml" \
    "$PREFIX/share/metainfo/dev.twitty.Twitty.metainfo.xml"
fi

for size in 16 24 32 48 64 128 256 512; do
  icon="$SCRIPT_DIR/share/icons/hicolor/${size}x${size}/apps/twitty.png"
  if [ -f "$icon" ]; then
    install -Dm644 "$icon" "$PREFIX/share/icons/hicolor/${size}x${size}/apps/twitty.png"
  fi
done
if [ -f "$SCRIPT_DIR/share/icons/hicolor/scalable/apps/twitty.svg" ]; then
  install -Dm644 "$SCRIPT_DIR/share/icons/hicolor/scalable/apps/twitty.svg" \
    "$PREFIX/share/icons/hicolor/scalable/apps/twitty.svg"
fi

if [ -d "$SCRIPT_DIR/share/licenses/twitty" ]; then
  for f in "$SCRIPT_DIR/share/licenses/twitty"/*; do
    [ -f "$f" ] || continue
    install -Dm644 "$f" "$PREFIX/share/licenses/twitty/$(basename "$f")"
  done
fi

refresh_caches

echo "Installed Twitty to $PREFIX"
case ":${PATH:-}:" in
  *":$PREFIX/bin:"*) ;;
  *)
    echo "Note: $PREFIX/bin is not in your PATH. Add it to your shell profile to run 'twitty' directly."
    ;;
esac
