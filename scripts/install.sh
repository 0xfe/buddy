#!/usr/bin/env bash
set -euo pipefail

BIN_NAME="buddy"
DEFAULT_REPO="0xfe/buddy"
FORCE=0
SKIP_INIT=0
VERSION=""
REPO="${BUDDY_INSTALL_REPO:-$DEFAULT_REPO}"
INSTALL_DIR="${BUDDY_INSTALL_DIR:-$HOME/.local/bin}"
FROM_DIST=""

usage() {
  cat <<'USAGE'
Install buddy from GitHub releases.

Usage:
  install.sh [options]

Options:
  --version <vX.Y.Z|X.Y.Z>  Install an explicit version (default: latest release)
  --repo <owner/repo>       GitHub repository (default: 0xfe/buddy)
  --install-dir <path>      Install destination (default: ~/.local/bin)
  --from-dist <dir>         Read artifact/checksum from local dist dir (offline mode)
  --force                   Overwrite installed binary if version differs
  --skip-init               Do not run `buddy init` after install
  -h, --help                Show this help

Examples:
  curl -fsSL https://raw.githubusercontent.com/0xfe/buddy/main/scripts/install.sh | bash
  ./scripts/install.sh --version v0.1.0
  ./scripts/install.sh --from-dist dist --version v0.1.0
USAGE
}

log() {
  echo "• $*"
}

warn() {
  echo "warning: $*" >&2
}

err() {
  echo "error: $*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      [[ $# -ge 2 ]] || err "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --repo)
      [[ $# -ge 2 ]] || err "--repo requires a value"
      REPO="$2"
      shift 2
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || err "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --from-dist)
      [[ $# -ge 2 ]] || err "--from-dist requires a value"
      FROM_DIST="$2"
      shift 2
      ;;
    --force)
      FORCE=1
      shift
      ;;
    --skip-init)
      SKIP_INIT=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      err "unknown option: $1 (run with --help)"
      ;;
  esac
done

normalize_tag() {
  local tag="$1"
  if [[ -z "$tag" ]]; then
    echo ""
    return 0
  fi
  if [[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "$tag"
    return 0
  fi
  if [[ "$tag" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "v$tag"
    return 0
  fi
  err "invalid version/tag '$tag' (expected vX.Y.Z or X.Y.Z)"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os:$arch" in
    Darwin:x86_64) echo "x86_64-apple-darwin" ;;
    Darwin:arm64|Darwin:aarch64) echo "aarch64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) echo "x86_64-unknown-linux-gnu" ;;
    Linux:arm64|Linux:aarch64) echo "aarch64-unknown-linux-gnu" ;;
    *)
      err "unsupported platform '$os/$arch' (supported: macOS/Linux x86_64/aarch64)"
      ;;
  esac
}

resolve_latest_tag() {
  local api_url response tag
  api_url="https://api.github.com/repos/${REPO}/releases/latest"
  response="$(curl -fsSL "$api_url")" || err "failed to fetch latest release metadata from $api_url"
  tag="$(printf "%s" "$response" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  [[ -n "$tag" ]] || err "failed to parse latest release tag from GitHub API response"
  normalize_tag "$tag"
}

verify_sha256() {
  local archive="$1" checksum_file="$2"
  if command -v shasum >/dev/null 2>&1; then
    (
      cd "$(dirname "$archive")"
      shasum -a 256 -c "$(basename "$checksum_file")"
    )
    return 0
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    (
      cd "$(dirname "$archive")"
      sha256sum -c "$(basename "$checksum_file")"
    )
    return 0
  fi
  warn "no sha256 verifier found (shasum/sha256sum); skipping checksum verification"
  return 0
}

run_post_install_init() {
  local bin_path="$1"
  local config_path="$HOME/.config/buddy/buddy.toml"

  if [[ "$SKIP_INIT" -eq 1 ]]; then
    log "skipping init (--skip-init)"
    return 0
  fi

  if [[ -f "$config_path" ]]; then
    log "config exists at ${config_path}; init not required"
    return 0
  fi

  if [[ -t 1 && -r /dev/tty ]]; then
    log "running first-time setup: ${bin_path} init"
    if "${bin_path}" init </dev/tty; then
      return 0
    fi
    warn "init did not complete successfully; run '${bin_path} init' manually"
    return 0
  fi

  log "no config found; run '${bin_path} init' (or just 'buddy' for auto-init) on first use"
}

TARGET="$(detect_target)"
TAG="$(normalize_tag "$VERSION")"
if [[ -z "$TAG" ]]; then
  TAG="$(resolve_latest_tag)"
fi

ARTIFACT="${BIN_NAME}-${TAG}-${TARGET}.tar.gz"
CHECKSUM="${ARTIFACT}.sha256"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE_PATH="${TMP_DIR}/${ARTIFACT}"
CHECKSUM_PATH="${TMP_DIR}/${CHECKSUM}"

if [[ -n "$FROM_DIST" ]]; then
  [[ -d "$FROM_DIST" ]] || err "--from-dist path '$FROM_DIST' is not a directory"
  [[ -f "${FROM_DIST}/${ARTIFACT}" ]] || err "artifact not found: ${FROM_DIST}/${ARTIFACT}"
  cp "${FROM_DIST}/${ARTIFACT}" "$ARCHIVE_PATH"
  if [[ -f "${FROM_DIST}/${CHECKSUM}" ]]; then
    cp "${FROM_DIST}/${CHECKSUM}" "$CHECKSUM_PATH"
  fi
else
  BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
  log "downloading ${ARTIFACT}"
  curl -fsSL "${BASE_URL}/${ARTIFACT}" -o "$ARCHIVE_PATH" || err "failed to download ${BASE_URL}/${ARTIFACT}"
  if curl -fsSL "${BASE_URL}/${CHECKSUM}" -o "$CHECKSUM_PATH"; then
    :
  else
    warn "checksum file not found (${CHECKSUM}); continuing without checksum verification"
    rm -f "$CHECKSUM_PATH"
  fi
fi

if [[ -f "$CHECKSUM_PATH" ]]; then
  log "verifying checksum"
  verify_sha256 "$ARCHIVE_PATH" "$CHECKSUM_PATH"
fi

log "extracting ${ARTIFACT}"
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR" || err "failed to extract archive ${ARCHIVE_PATH}"
NEW_BIN="${TMP_DIR}/${BIN_NAME}"
[[ -x "$NEW_BIN" ]] || err "archive did not contain executable '${BIN_NAME}'"

mkdir -p "$INSTALL_DIR"
DEST_BIN="${INSTALL_DIR}/${BIN_NAME}"

if [[ -x "$DEST_BIN" ]]; then
  INSTALLED_VER="$("$DEST_BIN" --version 2>/dev/null | awk 'NR==1 { print $2 }' || true)"
  if [[ -n "$INSTALLED_VER" && "v${INSTALLED_VER}" == "$TAG" ]]; then
    log "${BIN_NAME} ${INSTALLED_VER} already installed at ${DEST_BIN}"
    run_post_install_init "$DEST_BIN"
    exit 0
  fi
  if [[ "$FORCE" -ne 1 ]]; then
    err "${DEST_BIN} already exists (installed version: ${INSTALLED_VER:-unknown}); rerun with --force to replace"
  fi
fi

install -m 0755 "$NEW_BIN" "$DEST_BIN" || err "failed to install ${BIN_NAME} into ${INSTALL_DIR}"
log "installed ${BIN_NAME} ${TAG} to ${DEST_BIN}"
run_post_install_init "$DEST_BIN"
