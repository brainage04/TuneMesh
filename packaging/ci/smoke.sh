#!/bin/sh
# Smoke test for an installed TuneMesh package:
#   1. `tunemesh --version` and `tunemesh-worker --version` print a version;
#   2. the worker starts on an empty library and keeps running;
#   3. if xvfb-run and dbus-run-session are available, the app starts
#      headless on an empty library and is still running 15 s later.
#
# APP and WORKER are the commands that run the two binaries (default
# `tunemesh` and `tunemesh-worker`; e.g. APP="flatpak run io.github.brainage04.TuneMesh").
# LIB_PARENT is where the test library is created (default a temp dir; sandboxed
# formats need a folder they can see, such as ~/Music).
set -eu

APP=${APP:-tunemesh}
WORKER=${WORKER:-tunemesh-worker}
parent=${LIB_PARENT:-${TMPDIR:-/tmp}}
mkdir -p "$parent"
lib=$(mktemp -d "$parent/tunemesh-smoke.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/tunemesh-work.XXXXXX")
trap 'rm -rf "$lib" "$work"' EXIT
mkdir -p "$lib/.tunemesh/requests" "$lib/Playlists"

fail() { echo "smoke: FAIL: $*" >&2; exit 1; }

# $1 = label, $2 = command, $3 = expected first word of the version line.
check_version() {
    if out=$($2 --version 2>&1); then
        echo "$1 --version: $out"
        case $out in "$3 "[0-9]*) return 0 ;; esac
        fail "$1 --version printed an unexpected line"
    fi
    # Builds from before --version existed answer it with their usage text.
    if $2 --help 2>&1 | grep -q -- --version; then
        fail "$1 --version failed: $out"
    fi
    echo "::warning::$1 has no --version yet on this branch; checked --help instead"
    $2 --help >/dev/null 2>&1 || fail "$1 --help failed"
}

check_version tunemesh "$APP" tunemesh
check_version tunemesh-worker "$WORKER" tunemesh-worker

# Exit status 124 means timeout had to stop it: it was still running.
set +e
# APP and WORKER are commands with arguments: split them on purpose.
# shellcheck disable=SC2086
timeout 8 $WORKER --library "$lib" --work-dir "$work/worker" >"$work/worker.log" 2>&1
status=$?
set -e
[ $status -eq 124 ] || { cat "$work/worker.log" >&2; fail "worker exited with $status"; }
echo "worker ran for 8 s on an empty library"

if command -v xvfb-run >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1; then
    set +e
    # shellcheck disable=SC2086
    dbus-run-session -- xvfb-run -a env GDK_BACKEND=x11 GSK_RENDERER=cairo NO_AT_BRIDGE=1 \
        timeout 15 $APP --library "$lib" >"$work/app.log" 2>&1
    status=$?
    set -e
    cat "$work/app.log"
    [ $status -eq 124 ] || fail "app exited with $status during the headless launch"
    # Missing icon loaders or GSettings schemas show up only as log lines.
    if grep -qE 'Failed to load icon|-CRITICAL \*\*|No GSettings schemas' "$work/app.log"; then
        fail "the app logged missing resources during the headless launch"
    fi
    echo "app ran headless for 15 s"
else
    echo "xvfb-run or dbus-run-session missing; headless launch skipped"
fi
echo "smoke: OK"
