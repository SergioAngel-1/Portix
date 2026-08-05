#!/usr/bin/env bash
# Build and install (or update) portix on this machine.
#
# Installing is a local operation: the CI pipeline only verifies (fmt,
# clippy, tests); nothing deploys from GitHub. This script is the supported
# way to put the binary on the current machine. It ALWAYS overwrites the
# destination, so re-running it after a `git pull` IS "update portix".
#
# Usage:
#   scripts/install.sh                        # release build -> ~/.local/bin
#   scripts/install.sh --bin-dir "$HOME/.cargo/bin"
#   scripts/install.sh --sudo                 # -> /usr/local/bin (prompts for password)
#   scripts/install.sh --profile dev          # debug build (faster compile)
#
# The same settings are honored as environment variables, so the "variable
# via parameter" form works both ways:
#   BIN_DIR=/opt/portix scripts/install.sh
set -euo pipefail

BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
PROFILE="${PROFILE:-release}"
SUDO=""

usage() {
    sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir) BIN_DIR="${2:?--bin-dir needs a value}"; shift 2 ;;
        --profile) PROFILE="${2:?--profile needs a value}"; shift 2 ;;
        --sudo) SUDO="sudo"; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

case "$PROFILE" in
    release) BUILD_ARGS=(--release); SRC="target/release/portix" ;;
    dev) BUILD_ARGS=(); SRC="target/debug/portix" ;;
    *) echo "unknown profile: $PROFILE (use 'release' or 'dev')" >&2; exit 2 ;;
esac

cd "$(dirname "$0")/.."

echo "==> cargo build ${BUILD_ARGS[*]}"
cargo build "${BUILD_ARGS[@]}"

mkdir -p "$BIN_DIR"
if [ -n "$SUDO" ]; then
    $SUDO install -m 0755 "$SRC" "$BIN_DIR/portix"
else
    install -m 0755 "$SRC" "$BIN_DIR/portix"
fi

echo "Installed: $BIN_DIR/portix"
if [ -z "$SUDO" ]; then
    "$BIN_DIR/portix" --version || true
fi
