#!/usr/bin/env bash
# Prepare a Twitty release in the local working tree without committing,
# tagging, or pushing.
#
# Steps performed:
#   1. Verify a clean git working tree.
#   2. Validate VERSION is valid SemVer 2.0.0 and strictly greater than Cargo.toml.
#   3. Bump Cargo.toml (+ Cargo.lock offline).
#   4. Move CHANGELOG.md "Unreleased" entries to "## VERSION - DATE".
#   5. Insert a <release> entry into packaging/linux/dev.twitty.Twitty.metainfo.xml.
#   6. Update packaging/aur/*/PKGBUILD (pkgver=VERSION, pkgrel=1, reset twitty-bin sha256)
#      and regenerate packaging/aur/*/.SRCINFO.
#   7. Run release gates (fmt, clippy, tests, desktop/AppStream validation).
#   8. Print exact git commit / signed tag / push / aur-checksums commands WITHOUT running them.
#
# Pass --dry-run to preview the unified diff without modifying the working tree.
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO_ROOT="$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd)"
REPO_URL="https://github.com/omarallsharkawy/RTL-Terminal"

usage() {
  cat <<'EOF'
Usage:
  release.sh [OPTIONS] VERSION
  release.sh --check-version OLD NEW

Prepare release VERSION (e.g. 3.1.0) without committing, tagging, or pushing.

Options:
  -n, --dry-run              Show the unified diff only; leave the working tree untouched
  --date YYYY-MM-DD          Release date (default: UTC today)
  --repo-root DIR            Repository root directory (default: auto-detected)
  --skip-gates               Skip cargo fmt / clippy / test / validator gates
  --no-makepkg               Use built-in .SRCINFO generator instead of makepkg
  --check-version OLD NEW    Exit 0 iff OLD and NEW are valid SemVer and NEW > OLD
  -h, --help                 Show this help message
EOF
}

validate_semver() {
  local ver="$1"
  if [[ "$ver" == v* ]]; then
    echo "error: version must not start with 'v' (pass e.g. 3.1.0, not v3.1.0): $ver" >&2
    return 1
  fi
  local re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?(\+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?$'
  if [[ ! "$ver" =~ $re ]]; then
    echo "error: invalid SemVer 2.0.0 version: $ver" >&2
    return 1
  fi
  local pre="${BASH_REMATCH[5]:-}"
  if [[ -n "$pre" ]]; then
    local IFS='.'
    local part
    for part in $pre; do
      if [[ "$part" =~ ^0[0-9]+$ ]]; then
        echo "error: invalid SemVer prerelease identifier with leading zero: $ver" >&2
        return 1
      fi
    done
  fi
  return 0
}

# Compare two valid SemVer 2.0.0 strings A and B (ignoring +build metadata per spec).
# Prints -1 if A < B, 0 if A == B, 1 if A > B.
semver_compare() {
  local a="${1%%+*}"
  local b="${2%%+*}"

  local a_core="${a%%-*}"
  local b_core="${b%%-*}"
  local a_pre="" b_pre=""
  [[ "$a" == *-* ]] && a_pre="${a#*-}"
  [[ "$b" == *-* ]] && b_pre="${b#*-}"

  local a_maj a_min a_pat b_maj b_min b_pat
  IFS='.' read -r a_maj a_min a_pat <<< "$a_core"
  IFS='.' read -r b_maj b_min b_pat <<< "$b_core"

  local pairs=("$a_maj:$b_maj" "$a_min:$b_min" "$a_pat:$b_pat")
  local pair lhs rhs
  for pair in "${pairs[@]}"; do
    lhs="${pair%%:*}"
    rhs="${pair#*:}"
    if (( 10#$lhs < 10#$rhs )); then
      echo -1
      return 0
    elif (( 10#$lhs > 10#$rhs )); then
      echo 1
      return 0
    fi
  done

  if [[ -z "$a_pre" && -z "$b_pre" ]]; then
    echo 0
    return 0
  elif [[ -z "$a_pre" ]]; then
    echo 1
    return 0
  elif [[ -z "$b_pre" ]]; then
    echo -1
    return 0
  fi

  local -a a_parts b_parts
  IFS='.' read -r -a a_parts <<< "$a_pre"
  IFS='.' read -r -a b_parts <<< "$b_pre"

  local len_a="${#a_parts[@]}"
  local len_b="${#b_parts[@]}"
  local min_len="$len_a"
  (( len_b < min_len )) && min_len="$len_b"

  local i pa pb a_num b_num
  for (( i = 0; i < min_len; i++ )); do
    pa="${a_parts[$i]}"
    pb="${b_parts[$i]}"
    [[ "$pa" == "$pb" ]] && continue
    a_num=0
    b_num=0
    [[ "$pa" =~ ^[0-9]+$ ]] && a_num=1
    [[ "$pb" =~ ^[0-9]+$ ]] && b_num=1
    if (( a_num && b_num )); then
      if (( 10#$pa < 10#$pb )); then
        echo -1
      else
        echo 1
      fi
      return 0
    elif (( a_num && !b_num )); then
      echo -1
      return 0
    elif (( !a_num && b_num )); then
      echo 1
      return 0
    else
      if [[ "$pa" < "$pb" ]]; then
        echo -1
      else
        echo 1
      fi
      return 0
    fi
  done

  if (( len_a < len_b )); then
    echo -1
  elif (( len_a > len_b )); then
    echo 1
  else
    echo 0
  fi
}

semver_gt() {
  local new_ver="$1"
  local old_ver="$2"
  validate_semver "$old_ver" || return 1
  validate_semver "$new_ver" || return 1
  local cmp
  cmp="$(semver_compare "$new_ver" "$old_ver")"
  [[ "$cmp" -eq 1 ]]
}

read_cargo_version() {
  local cargo_toml="$1"
  awk '
    /^\[package\][[:space:]]*$/ { in_pkg = 1; next }
    /^\[/ { in_pkg = 0 }
    in_pkg && /^version[[:space:]]*=/ {
      val = $0
      sub(/^version[[:space:]]*=[[:space:]]*"/, "", val)
      sub(/".*$/, "", val)
      print val
      exit
    }
  ' "$cargo_toml"
}

bump_cargo_toml() {
  local cargo_toml="$1"
  local new_ver="$2"
  local tmp
  tmp="$(mktemp "${cargo_toml}.tmp.XXXXXX")"
  if ! awk -v ver="$new_ver" '
    BEGIN { in_pkg = 0; done = 0 }
    /^\[package\][[:space:]]*$/ { in_pkg = 1; print; next }
    /^\[/ { in_pkg = 0; print; next }
    in_pkg && !done && /^version[[:space:]]*=/ {
      print "version = \"" ver "\""
      done = 1
      next
    }
    { print }
    END { if (!done) exit 3 }
  ' "$cargo_toml" > "$tmp"; then
    rm -f "$tmp"
    echo "error: failed to update [package] version in $cargo_toml" >&2
    return 1
  fi
  mv "$tmp" "$cargo_toml"
}

update_cargo_lock_file() {
  local cargo_lock="$1"
  local new_ver="$2"
  local tmp
  tmp="$(mktemp "${cargo_lock}.tmp.XXXXXX")"
  if ! awk -v ver="$new_ver" '
    BEGIN { in_twitty = 0; updated = 0 }
    /^\[\[package\]\]/ { in_twitty = 0; print; next }
    /^name = "twitty"$/ { in_twitty = 1; print; next }
    in_twitty && !updated && /^version = "/ {
      print "version = \"" ver "\""
      updated = 1
      in_twitty = 0
      next
    }
    { print }
    END { if (!updated) exit 3 }
  ' "$cargo_lock" > "$tmp"; then
    rm -f "$tmp"
    echo "error: failed to update twitty version in $cargo_lock" >&2
    return 1
  fi
  mv "$tmp" "$cargo_lock"
}

refresh_cargo_lock_offline() {
  local repo_root="$1"
  local new_ver="$2"
  if [[ ! -f "$repo_root/Cargo.lock" ]]; then
    return 0
  fi
  # Prefer Cargo itself (offline) when a full workspace/crate layout is present.
  if command -v cargo >/dev/null 2>&1 && [[ -d "$repo_root/src" ]]; then
    if cargo metadata --offline --format-version 1 --manifest-path "$repo_root/Cargo.toml" >/dev/null 2>&1; then
      return 0
    fi
  fi
  update_cargo_lock_file "$repo_root/Cargo.lock" "$new_ver"
}

move_changelog() {
  local changelog="$1"
  local new_ver="$2"
  local rel_date="$3"
  local tmp
  tmp="$(mktemp "${changelog}.tmp.XXXXXX")"

  if ! awk -v ver="$new_ver" -v dt="$rel_date" '
    BEGIN {
      state = 0
      found_unreleased = 0
      has_content = 0
      dup = 0
      prefix = "## " ver
      plen = length(prefix)
      body_count = 0
    }
    {
      sub(/\r$/, "")
      if (substr($0, 1, plen) == prefix) {
        rest = substr($0, plen + 1)
        if (rest == "" || substr(rest, 1, 1) == " ") {
          dup = 1
        }
      }
      if (state == 0) {
        if ($0 == "## Unreleased") {
          found_unreleased = 1
          state = 1
          next
        }
        before[++before_count] = $0
      } else if (state == 1) {
        if ($0 ~ /^## /) {
          state = 2
          after[++after_count] = $0
          next
        }
        body[++body_count] = $0
        if ($0 ~ /[^[:space:]]/) {
          has_content = 1
        }
      } else {
        after[++after_count] = $0
      }
    }
    END {
      if (dup) {
        print "error: CHANGELOG.md already contains a section for " ver > "/dev/stderr"
        exit 4
      }
      if (!found_unreleased) {
        print "error: CHANGELOG.md is missing an \"## Unreleased\" section" > "/dev/stderr"
        exit 5
      }
      if (!has_content) {
        print "error: CHANGELOG.md \"## Unreleased\" section is empty" > "/dev/stderr"
        exit 6
      }

      for (i = 1; i <= before_count; i++) {
        print before[i]
      }
      print "## Unreleased"
      print ""
      print "## " ver " - " dt

      # Preserve body with single leading blank line and clean separation before next section.
      first = 1
      while (first <= body_count && body[first] == "") first++
      last = body_count
      while (last >= first && body[last] == "") last--

      print ""
      for (i = first; i <= last; i++) {
        print body[i]
      }
      if (after_count > 0) {
        print ""
        for (i = 1; i <= after_count; i++) {
          print after[i]
        }
      }
    }
  ' "$changelog" > "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  mv "$tmp" "$changelog"
}

add_metainfo_release() {
  local metainfo="$1"
  local new_ver="$2"
  local rel_date="$3"
  local tmp
  tmp="$(mktemp "${metainfo}.tmp.XXXXXX")"

  if ! awk -v ver="$new_ver" -v dt="$rel_date" -v repo_url="$REPO_URL" '
    BEGIN { inserted = 0; dup = 0; needle = "version=\"" ver "\"" }
    index($0, needle) > 0 { dup = 1 }
    {
      print
      if (!inserted && $0 ~ /^[[:space:]]*<releases>[[:space:]]*$/) {
        print "    <release version=\"" ver "\" date=\"" dt "\">"
        print "      <url type=\"details\">" repo_url "/releases/tag/v" ver "</url>"
        print "      <description>"
        print "        <p>Twitty " ver " release. See CHANGELOG.md for the full list of changes.</p>"
        print "      </description>"
        print "    </release>"
        inserted = 1
      }
    }
    END {
      if (dup) {
        print "error: AppStream metainfo already contains release " ver > "/dev/stderr"
        exit 4
      }
      if (!inserted) {
        print "error: <releases> element not found in AppStream metainfo" > "/dev/stderr"
        exit 5
      }
    }
  ' "$metainfo" > "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  mv "$tmp" "$metainfo"
}

update_pkgbuild_version() {
  local pkgbuild="$1"
  local new_ver="$2"
  local tmp
  tmp="$(mktemp "${pkgbuild}.tmp.XXXXXX")"

  if ! awk -v ver="$new_ver" '
    BEGIN { saw_ver = 0; saw_rel = 0 }
    /^pkgver=/ {
      print "pkgver=" ver
      saw_ver = 1
      next
    }
    /^pkgrel=/ {
      print "pkgrel=1"
      saw_rel = 1
      next
    }
    /^sha256sums_x86_64=\(/ {
      print "sha256sums_x86_64=(\x27SKIP\x27)"
      next
    }
    { print }
    END {
      if (!saw_ver || !saw_rel) {
        exit 3
      }
    }
  ' "$pkgbuild" > "$tmp"; then
    rm -f "$tmp"
    echo "error: failed to update pkgver/pkgrel in $pkgbuild" >&2
    return 1
  fi
  mv "$tmp" "$pkgbuild"
}

regenerate_aur_srcinfo() {
  local pkg_dir="$1"
  local use_makepkg="$2"
  local args=("--generate-srcinfo" "$pkg_dir/PKGBUILD" "--out" "$pkg_dir/.SRCINFO")
  if [[ "$use_makepkg" -eq 0 ]]; then
    args+=("--no-makepkg")
  fi
  "$SCRIPT_DIR/aur-checksums.sh" "${args[@]}"
}

check_clean_tree() {
  local repo_root="$1"
  if ! git -C "$repo_root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "error: $repo_root is not a git working tree" >&2
    return 1
  fi
  local dirty
  dirty="$(git -C "$repo_root" status --porcelain)"
  if [[ -n "$dirty" ]]; then
    echo "error: git working tree is not clean; commit or stash changes first:" >&2
    printf '%s\n' "$dirty" >&2
    return 1
  fi
}

run_gates() {
  local repo_root="$1"
  echo "==> Running release gates..."
  (
    cd "$repo_root"
    cargo fmt -- --check
    cargo clippy --all-targets -- -D warnings
    cargo test
    if command -v desktop-file-validate >/dev/null 2>&1; then
      desktop-file-validate packaging/linux/twitty.desktop
    fi
    if command -v appstreamcli >/dev/null 2>&1; then
      appstreamcli validate --no-net packaging/linux/dev.twitty.Twitty.metainfo.xml
    fi
  )
}

main() {
  local dry_run=0
  local skip_gates=0
  local use_makepkg=1
  local rel_date=""
  local repo_root="$DEFAULT_REPO_ROOT"
  local version=""

  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      -n|--dry-run)
        dry_run=1
        shift
        ;;
      --date)
        [[ "$#" -ge 2 ]] || { echo "error: --date requires YYYY-MM-DD" >&2; exit 2; }
        rel_date="$2"
        shift 2
        ;;
      --repo-root)
        [[ "$#" -ge 2 ]] || { echo "error: --repo-root requires a directory" >&2; exit 2; }
        repo_root="$2"
        shift 2
        ;;
      --skip-gates)
        skip_gates=1
        shift
        ;;
      --no-makepkg)
        use_makepkg=0
        shift
        ;;
      --check-version)
        [[ "$#" -ge 3 ]] || { echo "error: --check-version requires OLD NEW" >&2; exit 2; }
        if semver_gt "$3" "$2"; then
          exit 0
        else
          exit 1
        fi
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      -*)
        echo "error: unknown option: $1" >&2
        usage >&2
        exit 2
        ;;
      *)
        if [[ -n "$version" ]]; then
          echo "error: unexpected extra argument: $1" >&2
          usage >&2
          exit 2
        fi
        version="$1"
        shift
        ;;
    esac
  done

  if [[ -z "$version" ]]; then
    echo "error: VERSION is required" >&2
    usage >&2
    exit 2
  fi

  if [[ -z "$rel_date" ]]; then
    rel_date="$(date -u +%Y-%m-%d)"
  fi
  if [[ ! "$rel_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
    echo "error: --date must be YYYY-MM-DD, got: $rel_date" >&2
    exit 2
  fi

  check_clean_tree "$repo_root"

  local current_ver
  current_ver="$(read_cargo_version "$repo_root/Cargo.toml")"
  if [[ -z "$current_ver" ]]; then
    echo "error: could not read [package] version from $repo_root/Cargo.toml" >&2
    exit 1
  fi

  validate_semver "$current_ver"
  validate_semver "$version"
  if ! semver_gt "$version" "$current_ver"; then
    echo "error: new version $version must be strictly greater than Cargo.toml version $current_ver" >&2
    exit 1
  fi

  local metainfo_rel="packaging/linux/dev.twitty.Twitty.metainfo.xml"
  local -a aur_pkgs=()
  local pkgbuild
  for pkgbuild in "$repo_root"/packaging/aur/*/PKGBUILD; do
    [[ -f "$pkgbuild" ]] || continue
    aur_pkgs+=("$(basename -- "$(dirname -- "$pkgbuild")")")
  done
  if [[ "${#aur_pkgs[@]}" -eq 0 ]]; then
    echo "error: no packaging/aur/*/PKGBUILD files found in $repo_root" >&2
    exit 1
  fi

  if [[ "$dry_run" -eq 1 ]]; then
    TMP_STAGE="$(mktemp -d)"
    trap 'if [[ -n "${TMP_STAGE:-}" && -d "${TMP_STAGE:-}" ]]; then rm -rf "$TMP_STAGE"; fi' EXIT
    local stage="$TMP_STAGE"

    mkdir -p "$stage/packaging/linux"
    cp "$repo_root/Cargo.toml" "$stage/Cargo.toml"
    if [[ -f "$repo_root/Cargo.lock" ]]; then
      cp "$repo_root/Cargo.lock" "$stage/Cargo.lock"
    fi
    cp "$repo_root/CHANGELOG.md" "$stage/CHANGELOG.md"
    cp "$repo_root/$metainfo_rel" "$stage/$metainfo_rel"

    local pkg
    for pkg in "${aur_pkgs[@]}"; do
      mkdir -p "$stage/packaging/aur/$pkg"
      cp "$repo_root/packaging/aur/$pkg/PKGBUILD" "$stage/packaging/aur/$pkg/PKGBUILD"
      if [[ -f "$repo_root/packaging/aur/$pkg/.SRCINFO" ]]; then
        cp "$repo_root/packaging/aur/$pkg/.SRCINFO" "$stage/packaging/aur/$pkg/.SRCINFO"
      fi
    done

    bump_cargo_toml "$stage/Cargo.toml" "$version"
    if [[ -f "$stage/Cargo.lock" ]]; then
      update_cargo_lock_file "$stage/Cargo.lock" "$version"
    fi
    move_changelog "$stage/CHANGELOG.md" "$version" "$rel_date"
    add_metainfo_release "$stage/$metainfo_rel" "$version" "$rel_date"
    for pkg in "${aur_pkgs[@]}"; do
      update_pkgbuild_version "$stage/packaging/aur/$pkg/PKGBUILD" "$version"
      regenerate_aur_srcinfo "$stage/packaging/aur/$pkg" "$use_makepkg"
    done

    local -a rel_files=("Cargo.toml")
    [[ -f "$repo_root/Cargo.lock" ]] && rel_files+=("Cargo.lock")
    rel_files+=("CHANGELOG.md" "$metainfo_rel")
    for pkg in "${aur_pkgs[@]}"; do
      rel_files+=("packaging/aur/$pkg/PKGBUILD" "packaging/aur/$pkg/.SRCINFO")
    done

    local rel
    for rel in "${rel_files[@]}"; do
      diff -u --label "a/$rel" --label "b/$rel" "$repo_root/$rel" "$stage/$rel" || true
    done
    exit 0
  fi

  bump_cargo_toml "$repo_root/Cargo.toml" "$version"
  refresh_cargo_lock_offline "$repo_root" "$version"
  move_changelog "$repo_root/CHANGELOG.md" "$version" "$rel_date"
  add_metainfo_release "$repo_root/$metainfo_rel" "$version" "$rel_date"

  local pkg
  for pkg in "${aur_pkgs[@]}"; do
    update_pkgbuild_version "$repo_root/packaging/aur/$pkg/PKGBUILD" "$version"
    regenerate_aur_srcinfo "$repo_root/packaging/aur/$pkg" "$use_makepkg"
  done

  if [[ "$skip_gates" -eq 0 ]]; then
    run_gates "$repo_root"
  else
    echo "Skipping release gates (--skip-gates)." >&2
  fi

  local branch
  branch="$(git -C "$repo_root" rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)"

  cat <<EOF

Release v${version} is prepared in the working tree (no commit, tag, or push was run).
Review the changes, then run:

  git diff
  git add Cargo.toml Cargo.lock CHANGELOG.md ${metainfo_rel} packaging/aur/
  git commit -m "release: v${version}"
  git tag -s "v${version}" -m "Twitty v${version}"
  git push origin ${branch}
  git push origin "v${version}"

After CI finishes publishing GitHub release v${version}, update the AUR binary checksums:

  ./scripts/aur-checksums.sh "${version}"
EOF
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
