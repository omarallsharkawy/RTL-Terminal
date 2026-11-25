#!/usr/bin/env bash
# Build an x86_64 AppImage for Twitty from a release build directory.
# Mirrors the CI build-appimage job so it can be run locally without network access
# when appimagetool is already present.
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(CDPATH='' cd -- "$SCRIPT_DIR/../.." && pwd)"

APPIMAGETOOL_VERSION="1.9.1"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VERSION}/appimagetool-x86_64.AppImage"
APPIMAGETOOL_SHA256="ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0"

BIN_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/release"
OUT_PATH=""
VERSION=""
APPIMAGETOOL="${APPIMAGETOOL:-}"
RASTERIZE=0
STAGE_ONLY=""

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Build a Twitty x86_64 AppImage without bundling host GPU drivers.

Options:
  --bin-dir DIR         Directory containing 'twitty' and 'twitty-remote'
                        (default: \${CARGO_TARGET_DIR:-target}/release)
  --out PATH            Output AppImage path
                        (default: dist/twitty-<version>-linux-x86_64.AppImage)
  --version VER         Override package version (default: parsed from Cargo.toml)
  --appimagetool PATH   Path to appimagetool binary (or set APPIMAGETOOL)
  --rasterize           Re-rasterize hicolor PNG icons with rsvg-convert from docs/assets/twitty-mark.svg
  --stage-only DIR      Stage the AppDir into DIR and exit without invoking appimagetool
  -h, --help            Show this help message
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --bin-dir)
      [ "$#" -ge 2 ] || { echo "error: --bin-dir requires a directory" >&2; exit 2; }
      BIN_DIR="$2"
      shift 2
      ;;
    --out)
      [ "$#" -ge 2 ] || { echo "error: --out requires a file path" >&2; exit 2; }
      OUT_PATH="$2"
      shift 2
      ;;
    --version)
      [ "$#" -ge 2 ] || { echo "error: --version requires a value" >&2; exit 2; }
      VERSION="$2"
      shift 2
      ;;
    --appimagetool)
      [ "$#" -ge 2 ] || { echo "error: --appimagetool requires a path" >&2; exit 2; }
      APPIMAGETOOL="$2"
      shift 2
      ;;
    --rasterize)
      RASTERIZE=1
      shift
      ;;
    --stage-only)
      [ "$#" -ge 2 ] || { echo "error: --stage-only requires a directory" >&2; exit 2; }
      STAGE_ONLY="$2"
      shift 2
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

if [ -z "$VERSION" ]; then
  VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1)"
  if [ -z "$VERSION" ]; then
    echo "error: could not determine version from $REPO_ROOT/Cargo.toml" >&2
    exit 1
  fi
fi

if [ -z "$OUT_PATH" ]; then
  OUT_PATH="$REPO_ROOT/dist/twitty-${VERSION}-linux-x86_64.AppImage"
fi

if [ ! -x "$BIN_DIR/twitty" ] || [ ! -x "$BIN_DIR/twitty-remote" ]; then
  cat >&2 <<EOF
error: release binaries not found in: $BIN_DIR
Expected executable files:
  $BIN_DIR/twitty
  $BIN_DIR/twitty-remote

Build them first with:
  cargo build --release
or pass --bin-dir <path>.
EOF
  exit 1
fi

if [ -z "$STAGE_ONLY" ]; then
  if [ -z "$APPIMAGETOOL" ]; then
    if command -v appimagetool >/dev/null 2>&1; then
      APPIMAGETOOL="$(command -v appimagetool)"
    elif command -v appimagetool-x86_64.AppImage >/dev/null 2>&1; then
      APPIMAGETOOL="$(command -v appimagetool-x86_64.AppImage)"
    fi
  fi
  if [ -z "$APPIMAGETOOL" ] || [ ! -x "$APPIMAGETOOL" ]; then
    cat >&2 <<EOF
error: appimagetool was not found (checked --appimagetool, \$APPIMAGETOOL, and \$PATH).

To build the AppImage locally, download pinned appimagetool v${APPIMAGETOOL_VERSION} and verify its SHA-256:
  curl -fsSL -o appimagetool-x86_64.AppImage "${APPIMAGETOOL_URL}"
  echo "${APPIMAGETOOL_SHA256}  appimagetool-x86_64.AppImage" | sha256sum -c -
  chmod +x appimagetool-x86_64.AppImage
  APPIMAGETOOL=./appimagetool-x86_64.AppImage $0 --bin-dir "$BIN_DIR"
EOF
    exit 1
  fi
fi

if [ -n "$STAGE_ONLY" ]; then
  APPDIR="$STAGE_ONLY"
  rm -rf "$APPDIR"
  mkdir -p "$APPDIR"
else
  TMPDIR_WORK="$(mktemp -d)"
  trap 'rm -rf "$TMPDIR_WORK"' EXIT
  APPDIR="$TMPDIR_WORK/Twitty.AppDir"
  mkdir -p "$APPDIR"
fi

# Binaries: both sit side-by-side in usr/bin so current_exe() inside the mount
# finds twitty-remote next to twitty. No GPU drivers (Vulkan/EGL/GL) are bundled;
# they are loaded from the host system at runtime.
install -Dm755 "$BIN_DIR/twitty" "$APPDIR/usr/bin/twitty"
install -Dm755 "$BIN_DIR/twitty-remote" "$APPDIR/usr/bin/twitty-remote"

# Desktop entry and AppStream metainfo.
install -Dm644 "$REPO_ROOT/packaging/linux/twitty.desktop" "$APPDIR/twitty.desktop"
install -Dm644 "$REPO_ROOT/packaging/linux/twitty.desktop" \
  "$APPDIR/usr/share/applications/twitty.desktop"
install -Dm644 "$REPO_ROOT/packaging/linux/dev.twitty.Twitty.metainfo.xml" \
  "$APPDIR/usr/share/metainfo/dev.twitty.Twitty.metainfo.xml"

# Icons: hicolor PNGs 16..512 + scalable SVG, plus root twitty.png and .DirIcon.
for size in 16 24 32 48 64 128 256 512; do
  dst="$APPDIR/usr/share/icons/hicolor/${size}x${size}/apps/twitty.png"
  mkdir -p "$(dirname "$dst")"
  if [ "$RASTERIZE" -eq 1 ]; then
    if ! command -v rsvg-convert >/dev/null 2>&1; then
      echo "error: --rasterize requires rsvg-convert" >&2
      exit 1
    fi
    rsvg-convert -w "$size" -h "$size" "$REPO_ROOT/docs/assets/twitty-mark.svg" -o "$dst"
  else
    install -Dm644 "$REPO_ROOT/packaging/linux/icons/hicolor/${size}x${size}/apps/twitty.png" "$dst"
  fi
done
install -Dm644 "$REPO_ROOT/docs/assets/twitty-mark.svg" \
  "$APPDIR/usr/share/icons/hicolor/scalable/apps/twitty.svg"
install -Dm644 "$APPDIR/usr/share/icons/hicolor/256x256/apps/twitty.png" "$APPDIR/twitty.png"
ln -sf twitty.png "$APPDIR/.DirIcon"

# Licenses.
install -Dm644 "$REPO_ROOT/LICENSE" "$APPDIR/usr/share/licenses/twitty/LICENSE"
install -Dm644 "$REPO_ROOT/assets/fonts/OFL.txt" "$APPDIR/usr/share/licenses/twitty/OFL.txt"
for f in "$REPO_ROOT/licenses"/*; do
  [ -f "$f" ] || continue
  install -Dm644 "$f" "$APPDIR/usr/share/licenses/twitty/$(basename "$f")"
done

# AppRun: exec usr/bin/twitty directly so /proc/self/exe (std::env::current_exe())
# points inside the AppImage mount next to usr/bin/twitty-remote.
cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
set -eu
HERE="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
export PATH="$HERE/usr/bin${PATH:+:$PATH}"
exec "$HERE/usr/bin/twitty" "$@"
EOF
chmod 0755 "$APPDIR/AppRun"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$APPDIR/twitty.desktop"
fi
if command -v appstreamcli >/dev/null 2>&1; then
  appstreamcli validate --no-net "$APPDIR/usr/share/metainfo/dev.twitty.Twitty.metainfo.xml"
fi

if [ -n "$STAGE_ONLY" ]; then
  echo "Staged AppDir at $APPDIR"
  exit 0
fi

mkdir -p "$(dirname "$OUT_PATH")"
# --no-appstream avoids network screenshot checks during packaging; we validated offline above.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$APPIMAGETOOL" --no-appstream "$APPDIR" "$OUT_PATH"
chmod 0755 "$OUT_PATH"
echo "Built AppImage: $OUT_PATH"
