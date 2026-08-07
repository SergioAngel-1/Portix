#!/usr/bin/env bash
# Build, install, and update Portix on this machine.
#
# The binary is installed separately for the user and for the privileged
# context. Runtime caches are in-memory today, but this script also removes the
# well-known persistent cache/state directories so future cache versions cannot
# leave stale data behind. It never removes config.toml, Cargo's registry, or
# the project's target directory: those are not Portix runtime caches.
#
# Usage:
#   scripts/install.sh                         # user release install
#   scripts/install.sh --profile dev           # user debug install
#   scripts/install.sh --sudo                  # /usr/local/bin + system desktop entry
#   scripts/install.sh --all-contexts          # user install, then privileged install
#   scripts/install.sh --bin-dir "$HOME/.cargo/bin"
#
# Environment variables:
#   BIN_DIR=/opt/portix PROFILE=release scripts/install.sh
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
PROFILE="${PROFILE:-release}"
BIN_DIR_EXPLICIT=0
SUDO_MODE=0
ALL_CONTEXTS=0
NO_RECURSE="${PORTIX_NO_RECURSE:-0}"

usage() {
    sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir)
            [ $# -ge 2 ] || { echo "--bin-dir needs a value" >&2; exit 2; }
            BIN_DIR="$2"
            BIN_DIR_EXPLICIT=1
            shift 2
            ;;
        --profile)
            [ $# -ge 2 ] || { echo "--profile needs a value" >&2; exit 2; }
            PROFILE="$2"
            shift 2
            ;;
        --sudo)
            SUDO_MODE=1
            shift
            ;;
        --all-contexts)
            ALL_CONTEXTS=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

case "$PROFILE" in
    release)
        BUILD_ARGS=(--release)
        SRC_REL="target/release/portix"
        ;;
    dev)
        BUILD_ARGS=()
        SRC_REL="target/debug/portix"
        ;;
    *)
        echo "unknown profile: $PROFILE (use 'release' or 'dev')" >&2
        exit 2
        ;;
esac

# --sudo means the system installation unless the caller explicitly selected a
# destination. Running the script from `sudo portix --update` is also treated
# as a system installation, even when --sudo is not present.
if { [ "$SUDO_MODE" -eq 1 ] || [ "${EUID:-$(id -u)}" -eq 0 ]; } && [ "$BIN_DIR_EXPLICIT" -eq 0 ]; then
    BIN_DIR="/usr/local/bin"
fi

if [ "$ALL_CONTEXTS" -eq 1 ] && [ "$SUDO_MODE" -eq 1 ]; then
    echo "--all-contexts must be run as the normal user, not together with --sudo" >&2
    exit 2
fi

if [ "$SUDO_MODE" -eq 1 ] && [ "${EUID:-$(id -u)}" -ne 0 ]; then
    PRIV=(sudo)
else
    PRIV=()
fi

if [ "$NO_RECURSE" != "0" ] && [ "$NO_RECURSE" != "1" ]; then
    echo "PORTIX_NO_RECURSE must be 0 or 1" >&2
    exit 2
fi

run_priv() {
    if [ "${#PRIV[@]}" -gt 0 ]; then
        "${PRIV[@]}" "$@"
    else
        "$@"
    fi
}

# Build as the invoking user when a root process was started through sudo. This
# avoids leaving root-owned target artifacts in the user's checkout and keeps
# Cargo/Rustup resolution tied to the user's toolchain.
run_build() {
    if [ "${EUID:-$(id -u)}" -eq 0 ] && [ -n "${SUDO_USER:-}" ] && command -v runuser >/dev/null 2>&1; then
        BUILD_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6 || true)"
        if [ -n "$BUILD_HOME" ]; then
            echo "==> cargo build ${BUILD_ARGS[*]} as $SUDO_USER"
            runuser -u "$SUDO_USER" -- env HOME="$BUILD_HOME" \
                CARGO_HOME="${BUILD_HOME}/.cargo" \
                PATH="${BUILD_HOME}/.cargo/bin:/usr/local/bin:/usr/bin:/bin:$PATH" \
                cargo build "${BUILD_ARGS[@]}"
            return
        fi
    fi

    echo "==> cargo build ${BUILD_ARGS[*]}"
    cargo build "${BUILD_ARGS[@]}"
}

# Remove only exact Portix cache/state directories. Configuration is explicitly
# preserved: deleting ~/.config/portix would reset user preferences, not clear
# a runtime cache.
remove_exact_dir() {
    local path="$1"
    case "$path" in
        */portix)
            if [ -e "$path" ] || [ -L "$path" ]; then
                echo "==> clearing cache/state: $path"
                rm -rf -- "$path"
            fi
            ;;
        *)
            echo "refusing to remove non-Portix path: $path" >&2
            return 1
            ;;
    esac
}

clear_runtime_caches() {
    local home="$1"
    [ -n "$home" ] || return 0
    remove_exact_dir "$home/.cache/portix"
    remove_exact_dir "$home/.local/state/portix"
}

clear_all_runtime_caches() {
    clear_runtime_caches "$HOME"

    # `scripts/install.sh --sudo` is normally started as the unprivileged user
    # with PRIV=(sudo), so the root namespace must be cleared through sudo too.
    if [ "${#PRIV[@]}" -gt 0 ]; then
        for path in /root/.cache/portix /root/.local/state/portix /var/cache/portix /var/lib/portix; do
            if run_priv test -e "$path" || run_priv test -L "$path"; then
                echo "==> clearing cache/state: $path"
                run_priv rm -rf -- "$path"
            fi
        done
    fi

    # Under sudo, clear both the root context and the invoking user's context.
    # This is intentionally limited to exact ~/.cache/portix and state paths.
    if [ "${EUID:-$(id -u)}" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
        INVOKING_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6 || true)"
        clear_runtime_caches "$INVOKING_HOME"
    fi

    if [ "${EUID:-$(id -u)}" -eq 0 ]; then
        remove_exact_dir /var/cache/portix
        remove_exact_dir /var/lib/portix
    fi
}

render_desktop() {
    local template="$1"
    local destination="$2"
    local binary="$3"
    local launch="$4"
    local update="$5"
    local autostart="$6"
    sed -e "s|@PORTIX_BIN@|$binary|g" \
        -e "s|@PORTIX_LAUNCH@|$launch|g" \
        -e "s|@PORTIX_UPDATE@|$update|g" \
        -e "s|@PORTIX_AUTOSTART@|$autostart|g" \
        "$template" > "$destination"
}

install_user_integration() {
    local data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
    local config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
    local applications="$data_home/applications"
    local autostart_dir="$config_home/autostart"
    local icons="$data_home/icons/hicolor"
    local binary="$BIN_DIR/portix"
    local launch="ghostty -e $binary"
    local update="ghostty -e $binary --update"
    # Autostart follows the user installation. The system installation has its
    # own launcher and must be selected explicitly with --all-contexts.
    local autostart_cmd="$launch"

    mkdir -p "$applications" "$autostart_dir" \
        "$icons/256x256/apps" "$icons/512x512/apps"
    render_desktop "$PROJECT_ROOT/packaging/desktop/portix.desktop.in" \
        "$applications/portix.desktop" "$binary" "$launch" "$update" "$autostart_cmd"
    render_desktop "$PROJECT_ROOT/packaging/desktop/portix-autostart.desktop.in" \
        "$autostart_dir/portix.desktop" "$binary" "$launch" "$update" "$autostart_cmd"
    install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/256x256/apps/portix.png" \
        "$icons/256x256/apps/portix.png"
    install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/512x512/apps/portix.png" \
        "$icons/512x512/apps/portix.png"
    install -m 0644 "$PROJECT_ROOT/packaging/desktop/index.theme" "$icons/index.theme"

    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -f -t "$icons" >/dev/null
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$applications" >/dev/null 2>&1 || true
    fi
    if command -v systemctl >/dev/null 2>&1 && [ -n "${XDG_RUNTIME_DIR:-}" ]; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
    fi
}

install_system_integration() {
    local applications="/usr/share/applications"
    local icons="/usr/share/icons/hicolor"
    local binary="$BIN_DIR/portix"
    local launch="ghostty -e sudo $binary"
    local update="ghostty -e sudo $binary --update --all-contexts"
    local autostart_cmd="$launch"

    run_priv install -d -m 0755 "$applications" \
        "$icons/256x256/apps" "$icons/512x512/apps"
    if [ "${#PRIV[@]}" -gt 0 ]; then
        tmp_desktop="$(mktemp)"
        tmp_icon_index="$(mktemp)"
        render_desktop "$PROJECT_ROOT/packaging/desktop/portix.desktop.in" "$tmp_desktop" "$binary" "$launch" "$update" "$autostart_cmd"
        cp "$PROJECT_ROOT/packaging/desktop/index.theme" "$tmp_icon_index"
        run_priv install -m 0644 "$tmp_desktop" "$applications/portix.desktop"
        run_priv install -m 0644 "$tmp_icon_index" "$icons/index.theme"
        run_priv install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/256x256/apps/portix.png" "$icons/256x256/apps/portix.png"
        run_priv install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/512x512/apps/portix.png" "$icons/512x512/apps/portix.png"
        rm -f "$tmp_desktop" "$tmp_icon_index"
    else
        render_desktop "$PROJECT_ROOT/packaging/desktop/portix.desktop.in" "$applications/portix.desktop" "$binary" "$launch" "$update" "$autostart_cmd"
        install -m 0644 "$PROJECT_ROOT/packaging/desktop/index.theme" "$icons/index.theme"
        install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/256x256/apps/portix.png" "$icons/256x256/apps/portix.png"
        install -m 0644 "$PROJECT_ROOT/packaging/desktop/icons/hicolor/512x512/apps/portix.png" "$icons/512x512/apps/portix.png"
    fi

    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        run_priv gtk-update-icon-cache -f -t "$icons" >/dev/null
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        run_priv update-desktop-database "$applications" >/dev/null 2>&1 || true
    fi
}

cd "$PROJECT_ROOT"
run_build

SRC="$PROJECT_ROOT/$SRC_REL"
[ -x "$SRC" ] || { echo "build did not produce executable: $SRC" >&2; exit 1; }

run_priv install -d -m 0755 "$BIN_DIR"
run_priv install -m 0755 "$SRC" "$BIN_DIR/portix"
clear_all_runtime_caches

if [ "$SUDO_MODE" -eq 1 ] || [ "${EUID:-$(id -u)}" -eq 0 ]; then
    install_system_integration
else
    install_user_integration
fi

if [ "$NO_RECURSE" -eq 0 ] && { [ "$ALL_CONTEXTS" -eq 1 ] || { [ "${EUID:-$(id -u)}" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; }; }; then
    if [ "${EUID:-$(id -u)}" -eq 0 ] && [ -n "${SUDO_USER:-}" ]; then
        # `sudo portix --update` starts here as root. Update the invoking
        # user's binary and desktop/autostart integration without asking sudo
        # to recurse back into the root context.
        INVOKING_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6 || true)"
        if [ -n "$INVOKING_HOME" ]; then
            echo "==> updating invoking user context: $SUDO_USER"
            runuser -u "$SUDO_USER" -- env HOME="$INVOKING_HOME" \
                PORTIX_NO_RECURSE=1 BIN_DIR="$INVOKING_HOME/.local/bin" \
                "$PROJECT_ROOT/scripts/install.sh" --profile "$PROFILE"
        fi
    else
        echo "==> updating privileged context via sudo"
        sudo --preserve-env=SUDO_USER env PORTIX_NO_RECURSE=1 \
            "$PROJECT_ROOT/scripts/install.sh" --sudo --profile "$PROFILE"
    fi
fi

echo "Installed: $BIN_DIR/portix"
"$BIN_DIR/portix" --version 2>/dev/null || true
