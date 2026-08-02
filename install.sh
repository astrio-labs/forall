#!/usr/bin/env bash
# Forall installer — downloads compressed prebuilt binaries from GitHub Releases.
set -euo pipefail

REPO="${FORALL_INSTALL_REPO:-astrio-labs/forall}"
INSTALL_DIR="${FORALL_INSTALL_DIR:-${HOME}/.local/bin}"
BINARY_NAME="forall"

info() { printf '%s\n' "$*"; }
err() { printf 'forall install: %s\n' "$*" >&2; }

detect_platform() {
  local os arch
  os="$(uname -s | tr '[:upper:]' '[:lower:]')"
  arch="$(uname -m)"
  case "$os" in
    darwin) os="macos" ;;
    linux) os="linux" ;;
    mingw*|msys*|cygwin*|windows*) os="windows" ;;
    *) err "unsupported OS: $os"; exit 1 ;;
  esac
  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) err "unsupported architecture: $arch"; exit 1 ;;
  esac
  printf '%s %s\n' "$os" "$arch"
}

latest_release_tag() {
  if command -v gh >/dev/null 2>&1; then
    gh release view --repo "$REPO" --json tagName -q .tagName 2>/dev/null && return 0
  fi
  curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
    | head -n1
}

# Prefer an existing binary from PATH for extraction tools only when needed.
have_cmd() { command -v "$1" >/dev/null 2>&1; }

# A regular file that is not a symlink, so `chmod +x` cannot follow a link the
# archive planted to a file outside the extraction directory.
is_plain_file() { [ -f "$1" ] && [ ! -L "$1" ]; }

download() {
  local url="$1"
  local out="$2"
  curl -fsSL "$url" -o "$out"
}

sha256_of() {
  local file="$1"
  if have_cmd sha256sum; then
    sha256sum "$file" | awk '{print $1}'
  elif have_cmd shasum; then
    shasum -a 256 "$file" | awk '{print $1}'
  elif have_cmd openssl; then
    openssl dgst -sha256 "$file" | awk '{print $NF}'
  else
    return 1
  fi
}

# Compare "$file" against the "<asset>.sha256" published beside it. Exits 0 on
# a match, 1 on a mismatch, and 2 when no digest is published or no digest tool
# is available — the caller decides what to do about 2.
verify_checksum() {
  local file="$1" url="$2" digest expected actual
  digest="$(mktemp)"
  if ! curl -fsSL "${url}.sha256" -o "$digest" 2>/dev/null; then
    rm -f "$digest"
    return 2
  fi
  expected="$(tr -d '\r' <"$digest" | awk 'NR==1 {print $1}')"
  rm -f "$digest"
  if [ -z "$expected" ]; then
    return 2
  fi
  if ! actual="$(sha256_of "$file")"; then
    err "no sha256 tool found (sha256sum, shasum, or openssl); cannot verify the download"
    return 2
  fi
  [ "$actual" = "$expected" ]
}

# Apply the checksum policy to a freshly downloaded file. An unverifiable
# download is fatal when FORALL_REQUIRE_CHECKSUM=1 and a loud warning otherwise,
# because releases do not publish digests yet.
enforce_checksum() {
  local file="$1" url="$2" asset status=0
  asset="$(basename "$url")"
  verify_checksum "$file" "$url" || status=$?
  case "$status" in
    0)
      info "Verified sha256 for ${asset}"
      ;;
    2)
      if [ "${FORALL_REQUIRE_CHECKSUM:-0}" = "1" ]; then
        err "no published sha256 for ${asset} and FORALL_REQUIRE_CHECKSUM=1"
        return 1
      fi
      err "warning: no published sha256 for ${asset}; this download is unverified"
      ;;
    *)
      err "sha256 mismatch for ${asset}; refusing to install"
      return 1
      ;;
  esac
}

install_from_archive() {
  local url="$1"
  local expected_name="$2"
  local archive extract_dir binary
  archive="$(mktemp)"
  extract_dir="$(mktemp -d)"
  cleanup() {
    rm -f "$archive"
    rm -rf "$extract_dir"
  }
  trap cleanup EXIT

  if ! download "$url" "$archive"; then
    trap - EXIT
    cleanup
    return 1
  fi

  if ! have_cmd tar; then
    err "tar is required to unpack the Forall release archive"
    trap - EXIT
    cleanup
    return 1
  fi

  if ! enforce_checksum "$archive" "$url"; then
    trap - EXIT
    cleanup
    exit 1
  fi

  # Reject absolute or parent-relative members before unpacking so the archive
  # cannot write outside the extraction directory.
  local listing
  if ! listing="$(tar -tzf "$archive")"; then
    err "release archive could not be read"
    trap - EXIT
    cleanup
    return 1
  fi
  if printf '%s\n' "$listing" | grep -qE '^/|(^|/)\.\.(/|$)'; then
    err "release archive contains unsafe member paths; refusing to extract"
    trap - EXIT
    cleanup
    exit 1
  fi
  tar -xzf "$archive" -C "$extract_dir"

  binary="${extract_dir}/${expected_name}"
  if ! is_plain_file "$binary"; then
    # Tolerate archives that store just "forall" / "forall.exe".
    if is_plain_file "${extract_dir}/${BINARY_NAME}"; then
      binary="${extract_dir}/${BINARY_NAME}"
    elif is_plain_file "${extract_dir}/${BINARY_NAME}.exe"; then
      binary="${extract_dir}/${BINARY_NAME}.exe"
    else
      err "archive did not contain ${expected_name} as a regular file"
      trap - EXIT
      cleanup
      return 1
    fi
  fi

  chmod +x "$binary"
  mv "$binary" "${INSTALL_DIR}/${BINARY_NAME}"
  trap - EXIT
  cleanup
}

install_raw_binary() {
  local url="$1"
  local tmp
  tmp="$(mktemp)"
  if ! download "$url" "$tmp"; then
    rm -f "$tmp"
    return 1
  fi
  if ! enforce_checksum "$tmp" "$url"; then
    rm -f "$tmp"
    exit 1
  fi
  chmod +x "$tmp"
  mv "$tmp" "${INSTALL_DIR}/${BINARY_NAME}"
}

main() {
  local os arch tag base url platform
  mkdir -p "$INSTALL_DIR"
  # `exit` inside a command substitution only leaves the subshell, so the
  # unsupported-platform failure has to be propagated explicitly.
  if ! platform="$(detect_platform)"; then
    exit 1
  fi
  read -r os arch <<<"$platform"
  tag="$(latest_release_tag || true)"
  if [ -z "${tag:-}" ]; then
    err "no release found at https://github.com/${REPO}/releases yet."
    err "Check back after the first binary release is published."
    exit 1
  fi

  if [ "$os" = "windows" ]; then
    base="${BINARY_NAME}-${os}-${arch}.exe"
  else
    base="${BINARY_NAME}-${os}-${arch}"
  fi

  info "Installing Forall ${tag} (${base}) to ${INSTALL_DIR}/${BINARY_NAME}"

  # Prefer compressed archives (new releases). Fall back to raw binaries
  # so older release tags keep working.
  url="https://github.com/${REPO}/releases/download/${tag}/${base}.tar.gz"
  if install_from_archive "$url" "$base"; then
    info "Installed ${INSTALL_DIR}/${BINARY_NAME}"
  else
    info "Compressed asset unavailable; trying raw binary…"
    url="https://github.com/${REPO}/releases/download/${tag}/${base}"
    if ! install_raw_binary "$url"; then
      err "failed to download ${url}"
      err "Expected release asset: ${base}.tar.gz or ${base}"
      exit 1
    fi
    info "Installed ${INSTALL_DIR}/${BINARY_NAME}"
  fi

  if ! command -v "$BINARY_NAME" >/dev/null 2>&1; then
    info "Add to PATH: export PATH=\"${INSTALL_DIR}:\$PATH\""
  fi

  info ""
  info "Forall CLI installed. Run: forall"
  info "Staying on Cursor, Claude Code, or another MCP client? Skip the CLI — use MCP verify-only:"
  info "  1. Create a key at https://forall.astrio.app/dashboard"
  info "  2. npx @astrio/forall-mcp  (see packages/forall-mcp or docs/getting-started.md)"
}

main "$@"
