#!/bin/sh
# floway-cli installer.
#
# Usage:
#   curl -fsSL <release-url>/install.sh | sh
#   curl -fsSL <release-url>/install.sh | sh -s -- --endpoint https://gw.example --api-key KEY --agents all
#
# Downloads the floway binary for this platform from the floway-cli GitHub
# Releases, verifies its sha256, installs it to ~/.local/bin (or ~/bin when
# ~/.local/bin is not on PATH), and, when given --endpoint/--api-key, runs
# `floway install` non-interactively.
#
# Environment (also accepted instead of flags):
#   SETUP_ENDPOINT, SETUP_API_KEY, FLOWAY_AGENTS — matching the Floway
#   agent-setup installer convention.
set -u

REPO="${FLOWAY_CLI_REPO:-AzaContrib/floway-cli}"
BIN_NAME=floway
INSTALL_DIR="${FLOWAY_CLI_INSTALL_DIR:-$HOME/.local/bin}"

error() {
  printf 'floway-cli installer: %s\n' "$1" >&2
  exit 1
}

# --- argument parsing -------------------------------------------------------

ENDPOINT=""
API_KEY=""
AGENTS=""
while [ $# -gt 0 ]; do
  case "$1" in
    --endpoint) ENDPOINT="$2"; shift 2 ;;
    --endpoint=*) ENDPOINT="${1#*=}"; shift ;;
    --api-key|--key) API_KEY="$2"; shift 2 ;;
    --api-key=*|--key=*) API_KEY="${1#*=}"; shift ;;
    --agents) AGENTS="$2"; shift 2 ;;
    --agents=*) AGENTS="${1#*=}"; shift ;;
    --help|-h)
      sed -n '2,14p' "$0" 2>/dev/null || true
      exit 0 ;;
    *) error "unknown argument: $1" ;;
  esac
done

# --- platform detection -----------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Linux) OS_PART='unknown-linux-musl' ;;
  Darwin) OS_PART='apple-darwin' ;;
  *) error "unsupported operating system: $OS (this installer supports macOS and Linux; on Windows use cargo install --git https://github.com/$REPO)" ;;
esac
case "$ARCH" in
  x86_64|amd64) ARCH_PART='x86_64' ;;
  aarch64|arm64) ARCH_PART='aarch64' ;;
  *) error "unsupported architecture: $ARCH" ;;
esac
TARGET="${ARCH_PART}-${OS_PART}"

# --- download ---------------------------------------------------------------

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
  fetch_stdout() { wget -qO- "$1"; }
else
  error 'neither curl nor wget is available; install one and retry'
fi

# The latest release tag; GH exposes it as a stable redirect URL.
LATEST_URL="https://github.com/$REPO/releases/latest"
VERSION="$(fetch_stdout "$LATEST_URL" -I -o /dev/null -w '%{url_effective}' 2>/dev/null | sed 's#.*/tag/##')"
case "$VERSION" in
  v[0-9]*) ;;
  *) VERSION='latest' ;;
esac

BASE="https://github.com/$REPO/releases/download"
if [ "$VERSION" = 'latest' ]; then
  # Resolve the real tag so artifact URLs are stable even when newest-first
  # ordering matters; fall back to the redirect target fetch.
  RELEASE_API="https://api.github.com/repos/$REPO/releases/latest"
  VERSION="$(fetch_stdout "$RELEASE_API" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
  [ -n "$VERSION" ] || error "could not determine the latest release tag from $RELEASE_API"
fi

ARTIFACT="$BIN_NAME-$TARGET.tar.gz"
URL="$BASE/$VERSION/$ARTIFACT"
TMPDIR_FLOWAY="$(mktemp -d "${TMPDIR:-/tmp}/floway-cli.XXXXXX")" || error 'could not create a temporary directory'
trap 'rm -rf "$TMPDIR_FLOWAY"' EXIT
chmod 700 "$TMPDIR_FLOWAY" 2>/dev/null || true

printf 'Downloading floway %s for %s…\n' "$VERSION" "$TARGET"
fetch "$URL" "$TMPDIR_FLOWAY/$ARTIFACT" || error "could not download $URL"

CHECKSUM_URL="$URL.sha256"
fetch "$CHECKSUM_URL" "$TMPDIR_FLOWAY/checksums" 2>/dev/null || true
if [ -s "$TMPDIR_FLOWAY/checksums" ]; then
  EXPECTED="$(sed -n "s#^\([0-9a-f]\{64\}\).*${ARTIFACT//./\\.}.*#\1#p" "$TMPDIR_FLOWAY/checksums" | head -n 1)"
  # The release ships a per-artifact sidecar whose single line is "<sha>  <file>".
  [ -n "$EXPECTED" ] || EXPECTED="$(awk 'NR==1 {print $1}' "$TMPDIR_FLOWAY/checksums")"
  if [ -n "$EXPECTED" ]; then
    ACTUAL="$(sha256sum "$TMPDIR_FLOWAY/$ARTIFACT" 2>/dev/null | awk '{print $1}')"
    [ -n "$ACTUAL" ] || ACTUAL="$(shasum -a 256 "$TMPDIR_FLOWAY/$ARTIFACT" | awk '{print $1}')"
    [ "$ACTUAL" = "$EXPECTED" ] || error "checksum mismatch for $ARTIFACT (expected $EXPECTED, got $ACTUAL)"
    printf 'Checksum verified.\n'
  fi
fi

tar -xzf "$TMPDIR_FLOWAY/$ARTIFACT" -C "$TMPDIR_FLOWAY" || error 'could not extract the archive'
[ -x "$TMPDIR_FLOWAY/$BIN_NAME-$TARGET/$BIN_NAME" ] || error "the archive did not contain a $BIN_NAME binary"

# --- install ----------------------------------------------------------------

mkdir -p "$INSTALL_DIR" 2>/dev/null || true
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf 'NOTE: %s is not on your PATH.\n' "$INSTALL_DIR" >&2 ;;
esac
mv "$TMPDIR_FLOWAY/$BIN_NAME-$TARGET/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME" ||
  error "could not install to $INSTALL_DIR (try FLOWAY_CLI_INSTALL_DIR=~/bin)"
printf 'Installed %s/%s (%s).\n' "$INSTALL_DIR" "$BIN_NAME" "$VERSION"

# --- optional non-interactive setup -----------------------------------------

if [ -n "$ENDPOINT" ] && [ -n "$API_KEY" ]; then
  FLOWAY_AGENTS="${AGENTS:-${FLOWAY_AGENTS:-all}}" \
    "$INSTALL_DIR/$BIN_NAME" install \
    --endpoint "$ENDPOINT" --api-key "$API_KEY" --non-interactive
elif [ -n "${SETUP_ENDPOINT:-}" ] && [ -n "${SETUP_API_KEY:-}" ]; then
  # Agent-setup harness convention: the caller exports the credentials and we
  # consume them, then drop them so they cannot leak into child processes.
  FLOWAY_AGENTS="${AGENTS:-all}" "$INSTALL_DIR/$BIN_NAME" install \
    --endpoint "$SETUP_ENDPOINT" --api-key "$SETUP_API_KEY" --non-interactive
  status=$?
  unset SETUP_API_KEY SETUP_ENDPOINT
  exit $status
else
  printf 'Run `floway install` to configure your agents.\n'
fi
