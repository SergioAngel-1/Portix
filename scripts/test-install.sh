#!/usr/bin/env bash
# Tests for install.sh's pure process-matching helper. install.sh only runs
# main() when executed, so sourcing it here is safe and never touches /proc.
#
# Usage: scripts/test-install.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR
# shellcheck source=install.sh
source "$SCRIPT_DIR/install.sh"

failures=0

expect_portix() {
    local description="$1"
    shift
    if ! argv_is_portix "$@"; then
        printf 'FAIL: expected Portix: %s\n' "$description" >&2
        failures=$((failures + 1))
    fi
}

expect_not_portix() {
    local description="$1"
    shift
    if argv_is_portix "$@"; then
        printf 'FAIL: expected not Portix: %s\n' "$description" >&2
        failures=$((failures + 1))
    fi
}

expect_portix 'bare portix' portix
expect_portix 'absolute path with arguments' /x/portix --once
expect_portix 'portix-static' /usr/local/bin/portix-static --once
expect_portix 'sudo portix' sudo portix
expect_portix 'sudo with a value-taking option' sudo -u root /usr/local/bin/portix
expect_portix 'sudo with an attached long option' sudo --user=root /usr/local/bin/portix
expect_portix 'sudo with a separate long option' sudo --user root portix
expect_portix 'sudo with an option terminator' sudo -- portix
expect_portix 'sudo --preserve-env takes no separate value' sudo --preserve-env portix
expect_portix 'sudo -E portix' sudo -E portix

expect_not_portix 'sudo pager on a Portix file' sudo less /x/portix
expect_not_portix 'sudo copy of the binary' sudo cp a/portix b/portix
expect_not_portix 'editor on a Portix source file' vim portix.rs
expect_not_portix 'shell running a Portix command' bash -c portix
expect_not_portix 'empty argv'

# A BIN_DIR containing sed metacharacters must not break or corrupt the
# generated desktop entry.
test_render_desktop() {
    local tmpdir template destination binary launch update autostart

    binary='a&b|c\d'
    launch="$binary --run"
    update="$binary --update"
    autostart="$binary --autostart"
    tmpdir="$(mktemp -d)"
    template="$tmpdir/portix.desktop.in"
    destination="$tmpdir/portix.desktop"
    printf 'Exec=@PORTIX_LAUNCH@\nTryExec=@PORTIX_BIN@\nUpdate=@PORTIX_UPDATE@\nAutostart=@PORTIX_AUTOSTART@\nTerminal=@PORTIX_TERMINAL_FLAG@\n' > "$template"
    render_desktop "$template" "$destination" "$binary" "$launch" "$update" "$autostart" true
    if ! grep -Fxq "TryExec=$binary" "$destination" ||
        ! grep -Fxq "Exec=$launch" "$destination" ||
        ! grep -Fxq "Update=$update" "$destination" ||
        ! grep -Fxq "Autostart=$autostart" "$destination" ||
        ! grep -Fxq "Terminal=true" "$destination"; then
        printf 'FAIL: render_desktop corrupted a path with sed metacharacters\n' >&2
        failures=$((failures + 1))
    fi
    rm -rf -- "$tmpdir"
}

# ghostty is only the default when it is actually installed; otherwise the
# desktop supplies the terminal (Terminal=true).
test_default_terminal() {
    local bindir

    bindir="$(mktemp -d)"
    if [ -n "$(PATH="$bindir" default_terminal)" ]; then
        printf 'FAIL: default_terminal invented a terminal\n' >&2
        failures=$((failures + 1))
    fi
    : > "$bindir/ghostty"
    chmod +x "$bindir/ghostty"
    if [ "$(PATH="$bindir" default_terminal)" != "ghostty" ]; then
        printf 'FAIL: default_terminal did not pick ghostty from PATH\n' >&2
        failures=$((failures + 1))
    fi
    rm -rf -- "$bindir"
}

test_terminal_exec() {
    TERMINAL=""
    if [ "$(terminal_exec /x/portix)" != "/x/portix" ]; then
        printf 'FAIL: terminal_exec added a terminal when none was configured\n' >&2
        failures=$((failures + 1))
    fi
    TERMINAL=foot
    if [ "$(terminal_exec /x/portix)" != "foot -e /x/portix" ]; then
        printf 'FAIL: terminal_exec did not prefix the configured terminal\n' >&2
        failures=$((failures + 1))
    fi
    TERMINAL=""
}

test_render_desktop
test_default_terminal
test_terminal_exec

if [ "$failures" -ne 0 ]; then
    printf '%d test(s) failed\n' "$failures" >&2
    exit 1
fi

echo "install.sh argv tests passed"
