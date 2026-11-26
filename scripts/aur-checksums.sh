#!/usr/bin/env bash
# Fetch SHA256SUMS.txt from a published Twitty GitHub release, update
# packaging/aur/twitty-bin/PKGBUILD with the Linux tarball checksum, and
# regenerate .SRCINFO (using `makepkg --printsrcinfo` when available, or a
# built-in fallback parser).
#
# Never touches the network unless invoked without --checksums-file / --generate-srcinfo.
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_REPO_ROOT="$(CDPATH='' cd -- "$SCRIPT_DIR/.." && pwd)"
DEFAULT_REPO_URL="https://github.com/omarallsharkawy/RTL-Terminal"

usage() {
  cat <<'EOF'
Usage:
  aur-checksums.sh [OPTIONS] VERSION
  aur-checksums.sh --generate-srcinfo PKGBUILD [--no-makepkg] [--out PATH]

Download SHA256SUMS.txt for release vVERSION, fill sha256sums_x86_64 in
packaging/aur/twitty-bin/PKGBUILD, and regenerate .SRCINFO.

Options:
  --checksums-file PATH      Read SHA256SUMS.txt from a local file (no network)
  --pkg-dir DIR              AUR package directory (default: packaging/aur/twitty-bin)
  --repo-url URL             GitHub repository base URL
                             (default: parsed from PKGBUILD url= or Twitty repo URL)
  --no-makepkg               Force the built-in .SRCINFO generator instead of makepkg
  --generate-srcinfo FILE    Generate .SRCINFO from PKGBUILD FILE without network access
  --out PATH                 Write --generate-srcinfo output to PATH instead of stdout
  -h, --help                 Show this help message
EOF
}

validate_semver() {
  local ver="$1"
  if [[ "$ver" == v* ]]; then
    echo "error: version must not have a leading 'v': $ver" >&2
    return 1
  fi
  local re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?(\+([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?$'
  if [[ ! "$ver" =~ $re ]]; then
    echo "error: invalid semver version: $ver" >&2
    return 1
  fi
  # Validate numeric prerelease identifiers have no leading zeros.
  local pre="${BASH_REMATCH[5]:-}"
  if [[ -n "$pre" ]]; then
    local IFS='.'
    local part
    for part in $pre; do
      if [[ "$part" =~ ^0[0-9]+$ ]]; then
        echo "error: invalid semver prerelease identifier with leading zero: $ver" >&2
        return 1
      fi
    done
  fi
  return 0
}

read_pkgbuild_scalar() {
  local pkgbuild="$1"
  local key="$2"
  awk -v k="$key" '
    $0 ~ ("^" k "=") {
      val = substr($0, length(k) + 2)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", val)
      if ((substr(val, 1, 1) == "\x27" && substr(val, length(val), 1) == "\x27") ||
          (substr(val, 1, 1) == "\"" && substr(val, length(val), 1) == "\"")) {
        val = substr(val, 2, length(val) - 2)
      }
      print val
      exit
    }
  ' "$pkgbuild"
}

extract_tarball_sha256() {
  local checksums_file="$1"
  local asset_name="$2"
  local matches=()
  local line hash name

  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line%$'\r'}"
    [[ -z "${line// }" ]] && continue
    [[ "$line" =~ ^[[:space:]]*# ]] && continue
    # Standard sha256sum output: "<64-hex>  <filename>" or "<64-hex> *<filename>"
    if [[ "$line" =~ ^[[:space:]]*([0-9a-fA-F]{64})[[:space:]]+\*?(\./)?([^[:space:]]+)[[:space:]]*$ ]]; then
      hash="${BASH_REMATCH[1]}"
      name="${BASH_REMATCH[3]}"
      if [[ "$name" == "$asset_name" ]]; then
        matches+=("${hash,,}")
      fi
    fi
  done < "$checksums_file"

  if [[ "${#matches[@]}" -eq 0 ]]; then
    echo "error: asset '$asset_name' not found in $checksums_file" >&2
    return 1
  fi
  if [[ "${#matches[@]}" -gt 1 ]]; then
    echo "error: multiple checksum entries for '$asset_name' in $checksums_file" >&2
    return 1
  fi
  printf '%s\n' "${matches[0]}"
}

update_pkgbuild_sha256() {
  local pkgbuild="$1"
  local sha256="$2"
  if [[ ! "$sha256" =~ ^[0-9a-f]{64}$ ]]; then
    echo "error: invalid SHA-256 digest: $sha256" >&2
    return 1
  fi

  local tmp
  tmp="$(mktemp "${pkgbuild}.tmp.XXXXXX")"
  if ! awk -v hash="$sha256" '
    BEGIN { replaced = 0 }
    /^sha256sums_x86_64=\(/ {
      print "sha256sums_x86_64=(\x27" hash "\x27)"
      replaced = 1
      next
    }
    { print }
    END {
      if (!replaced) {
        exit 3
      }
    }
  ' "$pkgbuild" > "$tmp"; then
    rm -f "$tmp"
    echo "error: could not find sha256sums_x86_64=(...) in $pkgbuild" >&2
    return 1
  fi
  mv "$tmp" "$pkgbuild"
}

# Pure, non-evaluating PKGBUILD -> .SRCINFO generator.
# Parses standard PKGBUILD scalar and array assignments (single- or multi-line)
# without executing arbitrary shell code, expands safe scalar references
# (${pkgname}, ${pkgver}, ${pkgrel}, ${url}), and emits the exact field order
# produced by `makepkg --printsrcinfo`.
generate_srcinfo_fallback() {
  local pkgbuild="$1"
  awk '
    function trim(s) {
      sub(/^[[:space:]]+/, "", s)
      sub(/[[:space:]]+$/, "", s)
      return s
    }
    function unquote(s) {
      s = trim(s)
      if (length(s) >= 2) {
        first = substr(s, 1, 1)
        last = substr(s, length(s), 1)
        if ((first == "\x27" && last == "\x27") || (first == "\"" && last == "\"")) {
          return substr(s, 2, length(s) - 2)
        }
      }
      return s
    }
    function expand_vars(s) {
      if (scalars["pkgname"] != "") {
        gsub(/\$\{pkgname\}/, scalars["pkgname"], s)
        gsub(/\$pkgname/, scalars["pkgname"], s)
      }
      if (scalars["pkgver"] != "") {
        gsub(/\$\{pkgver\}/, scalars["pkgver"], s)
        gsub(/\$pkgver/, scalars["pkgver"], s)
      }
      if (scalars["pkgrel"] != "") {
        gsub(/\$\{pkgrel\}/, scalars["pkgrel"], s)
        gsub(/\$pkgrel/, scalars["pkgrel"], s)
      }
      if (scalars["url"] != "") {
        gsub(/\$\{url\}/, scalars["url"], s)
        gsub(/\$url/, scalars["url"], s)
      }
      return s
    }
    function push_item(key, item,    idx) {
      idx = ++arr_len[key]
      arr[key, idx] = item
    }
    function parse_array_fragment(key, frag,    i, n, ch, quote, tok, in_tok) {
      n = length(frag)
      quote = ""
      tok = ""
      in_tok = 0
      for (i = 1; i <= n; i++) {
        ch = substr(frag, i, 1)
        if (quote != "") {
          if (ch == quote) {
            quote = ""
          } else {
            tok = tok ch
          }
          continue
        }
        if (ch == "#") {
          break
        }
        if (ch == "\x27" || ch == "\"") {
          quote = ch
          in_tok = 1
          continue
        }
        if (ch ~ /[[:space:]]/) {
          if (in_tok) {
            push_item(key, tok)
            tok = ""
            in_tok = 0
          }
          continue
        }
        if (ch == ")") {
          if (in_tok) {
            push_item(key, tok)
            tok = ""
            in_tok = 0
          }
          in_array = ""
          return
        }
        tok = tok ch
        in_tok = 1
      }
      if (in_tok) {
        push_item(key, tok)
      }
    }
    function emit_scalar(key) {
      if (key in scalars && scalars[key] != "") {
        printf "\t%s = %s\n", key, expand_vars(scalars[key])
      }
    }
    function emit_array(key,    i) {
      for (i = 1; i <= arr_len[key]; i++) {
        printf "\t%s = %s\n", key, expand_vars(arr[key, i])
      }
    }

    {
      sub(/\r$/, "")
      # Stop scanning once function definitions start (e.g. pkgver(), build(), package()).
      if (!in_array && $0 ~ /^[A-Za-z_][A-Za-z0-9_]*\(\)[[:space:]]*\{/) {
        exit
      }
      if (in_array != "") {
        parse_array_fragment(in_array, $0)
        next
      }
      if ($0 ~ /^[[:space:]]*#/ || $0 ~ /^[[:space:]]*$/) {
        next
      }
      if ($0 ~ /^[A-Za-z_][A-Za-z0-9_]*=\(/) {
        eq = index($0, "=")
        key = substr($0, 1, eq - 1)
        rest = substr($0, eq + 2)
        in_array = key
        arr_len[key] = 0
        parse_array_fragment(key, rest)
        next
      }
      if ($0 ~ /^[A-Za-z_][A-Za-z0-9_]*=/) {
        eq = index($0, "=")
        key = substr($0, 1, eq - 1)
        val = substr($0, eq + 1)
        scalars[key] = unquote(val)
        next
      }
    }

    END {
      pkgbase = (scalars["pkgbase"] != "") ? scalars["pkgbase"] : scalars["pkgname"]
      if (pkgbase == "" || scalars["pkgname"] == "" || scalars["pkgver"] == "") {
        print "error: PKGBUILD missing required pkgname or pkgver" > "/dev/stderr"
        exit 2
      }
      printf "pkgbase = %s\n", pkgbase
      emit_scalar("pkgdesc")
      emit_scalar("pkgver")
      emit_scalar("pkgrel")
      emit_scalar("epoch")
      emit_scalar("url")
      emit_array("arch")
      emit_array("license")
      emit_array("makedepends")
      emit_array("checkdepends")
      emit_array("depends")
      emit_array("optdepends")
      emit_array("provides")
      emit_array("conflicts")
      emit_array("replaces")
      emit_array("backup")
      emit_array("options")
      emit_array("source")
      emit_array("source_x86_64")
      emit_array("source_aarch64")
      emit_array("sha256sums")
      emit_array("sha256sums_x86_64")
      emit_array("sha256sums_aarch64")
      printf "\npkgname = %s\n", scalars["pkgname"]
    }
  ' "$pkgbuild"
}

render_srcinfo() {
  local pkgbuild="$1"
  local use_makepkg="$2"
  local pkg_dir
  pkg_dir="$(CDPATH='' cd -- "$(dirname -- "$pkgbuild")" && pwd)"
  local pkg_file
  pkg_file="$(basename -- "$pkgbuild")"

  if [[ "$use_makepkg" -eq 1 && "$pkg_file" == "PKGBUILD" ]] && command -v makepkg >/dev/null 2>&1; then
    (cd "$pkg_dir" && makepkg --printsrcinfo) | sed 's/[[:space:]]*$//'
  else
    generate_srcinfo_fallback "$pkgbuild" | sed 's/[[:space:]]*$//'
  fi
}

write_srcinfo() {
  local pkg_dir="$1"
  local use_makepkg="$2"
  local pkgbuild="$pkg_dir/PKGBUILD"
  local srcinfo="$pkg_dir/.SRCINFO"
  local tmp
  tmp="$(mktemp "${srcinfo}.tmp.XXXXXX")"
  if ! render_srcinfo "$pkgbuild" "$use_makepkg" > "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  mv "$tmp" "$srcinfo"
}

main() {
  local checksums_file=""
  local pkg_dir="$DEFAULT_REPO_ROOT/packaging/aur/twitty-bin"
  local repo_url=""
  local use_makepkg=1
  local generate_srcinfo_path=""
  local out_path=""
  local version=""

  while [[ "$#" -gt 0 ]]; do
    case "$1" in
      --checksums-file)
        [[ "$#" -ge 2 ]] || { echo "error: --checksums-file requires a path" >&2; exit 2; }
        checksums_file="$2"
        shift 2
        ;;
      --pkg-dir)
        [[ "$#" -ge 2 ]] || { echo "error: --pkg-dir requires a directory" >&2; exit 2; }
        pkg_dir="$2"
        shift 2
        ;;
      --repo-url)
        [[ "$#" -ge 2 ]] || { echo "error: --repo-url requires a URL" >&2; exit 2; }
        repo_url="$2"
        shift 2
        ;;
      --no-makepkg)
        use_makepkg=0
        shift
        ;;
      --generate-srcinfo)
        [[ "$#" -ge 2 ]] || { echo "error: --generate-srcinfo requires a PKGBUILD path" >&2; exit 2; }
        generate_srcinfo_path="$2"
        shift 2
        ;;
      --out)
        [[ "$#" -ge 2 ]] || { echo "error: --out requires a path" >&2; exit 2; }
        out_path="$2"
        shift 2
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

  if [[ -n "$generate_srcinfo_path" ]]; then
    if [[ ! -f "$generate_srcinfo_path" ]]; then
      echo "error: PKGBUILD not found: $generate_srcinfo_path" >&2
      exit 1
    fi
    if [[ -n "$out_path" ]]; then
      local tmp
      tmp="$(mktemp "${out_path}.tmp.XXXXXX")"
      if ! render_srcinfo "$generate_srcinfo_path" "$use_makepkg" > "$tmp"; then
        rm -f "$tmp"
        exit 1
      fi
      mv "$tmp" "$out_path"
    else
      render_srcinfo "$generate_srcinfo_path" "$use_makepkg"
    fi
    exit 0
  fi

  if [[ -z "$version" ]]; then
    echo "error: VERSION is required" >&2
    usage >&2
    exit 2
  fi

  validate_semver "$version"

  local pkgbuild="$pkg_dir/PKGBUILD"
  if [[ ! -f "$pkgbuild" ]]; then
    echo "error: PKGBUILD not found: $pkgbuild" >&2
    exit 1
  fi

  local pkgver
  pkgver="$(read_pkgbuild_scalar "$pkgbuild" pkgver)"
  if [[ "$pkgver" != "$version" ]]; then
    echo "error: $pkgbuild has pkgver=$pkgver, expected $version (run scripts/release.sh $version first)" >&2
    exit 1
  fi

  if [[ -z "$repo_url" ]]; then
    repo_url="$(read_pkgbuild_scalar "$pkgbuild" url)"
    repo_url="${repo_url:-$DEFAULT_REPO_URL}"
  fi
  repo_url="${repo_url%/}"

  local asset_name="twitty-${version}-linux-x86_64.tar.gz"
  TMP_SUMS=""
  cleanup() {
    if [[ -n "${TMP_SUMS:-}" && -f "${TMP_SUMS:-}" ]]; then
      rm -f "$TMP_SUMS"
    fi
  }
  trap cleanup EXIT

  if [[ -n "$checksums_file" ]]; then
    if [[ ! -f "$checksums_file" ]]; then
      echo "error: checksums file not found: $checksums_file" >&2
      exit 1
    fi
  else
    if ! command -v curl >/dev/null 2>&1; then
      echo "error: curl is required to download SHA256SUMS.txt" >&2
      exit 1
    fi
    local url="${repo_url}/releases/download/v${version}/SHA256SUMS.txt"
    TMP_SUMS="$(mktemp)"
    echo "Downloading $url ..." >&2
    curl -fsSL -o "$TMP_SUMS" "$url"
    checksums_file="$TMP_SUMS"
  fi

  local sha256
  sha256="$(extract_tarball_sha256 "$checksums_file" "$asset_name")"

  update_pkgbuild_sha256 "$pkgbuild" "$sha256"
  write_srcinfo "$pkg_dir" "$use_makepkg"

  echo "Updated $pkgbuild and $pkg_dir/.SRCINFO"
  echo "  asset:  $asset_name"
  echo "  sha256: $sha256"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
