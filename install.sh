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

download() {
  local url="$1"
  local out="$2"
  curl -fsSL "$url" -o "$out"
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
  tar -xzf "$archive" -C "$extract_dir"

  binary="${extract_dir}/${expected_name}"
  if [ ! -f "$binary" ]; then
    # Tolerate archives that store just "forall" / "forall.exe".
    if [ -f "${extract_dir}/${BINARY_NAME}" ]; then
      binary="${extract_dir}/${BINARY_NAME}"
    elif [ -f "${extract_dir}/${BINARY_NAME}.exe" ]; then
      binary="${extract_dir}/${BINARY_NAME}.exe"
    else
      err "archive did not contain ${expected_name}"
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
  chmod +x "$tmp"
  mv "$tmp" "${INSTALL_DIR}/${BINARY_NAME}"
}

main() {
  local os arch tag base url
  mkdir -p "$INSTALL_DIR"
  read -r os arch <<<"$(detect_platform)"
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
