#!/bin/sh
# Starts the app in a Wayland-only session (headless sway, no X server) and
# requires it to be still running 10 s later.
#
# APP is the command that runs the app (default `tunemesh`). Needs sway and
# dbus-run-session.
set -eu

APP=${APP:-tunemesh}
work=$(mktemp -d "${TMPDIR:-/tmp}/tunemesh-wayland.XXXXXX")
sway_pid=
cleanup() {
    [ -n "$sway_pid" ] && kill "$sway_pid" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT
fail() {
    echo "wayland-launch: FAIL: $*" >&2
    [ -f "$work/app.log" ] && cat "$work/app.log" >&2
    exit 1
}

XDG_RUNTIME_DIR=$work/run
mkdir -m 700 "$XDG_RUNTIME_DIR" "$work/lib"
export XDG_RUNTIME_DIR
unset DISPLAY WAYLAND_DISPLAY
printf 'output HEADLESS-1 resolution 1280x800\n' >"$work/sway.cfg"
# Headless and software rendered; --unsupported-gpu only silences sway's
# refusal when the host has the proprietary NVIDIA driver loaded.
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    sway --unsupported-gpu -c "$work/sway.cfg" >"$work/sway.log" 2>&1 &
sway_pid=$!
display=
for _ in $(seq 40); do
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        case $socket in *.lock) ;; *) [ -S "$socket" ] && display=${socket##*/} ;; esac
    done
    [ -n "$display" ] && break
    sleep 0.5
done
[ -n "$display" ] || { cat "$work/sway.log" >&2; fail "sway did not start"; }

set +e
# APP is a command with arguments: split it on purpose.
# shellcheck disable=SC2086
WAYLAND_DISPLAY=$display dbus-run-session -- timeout 10 $APP --library "$work/lib" >"$work/app.log" 2>&1
status=$?
set -e
[ $status -eq 124 ] || fail "the app exited with $status in a Wayland-only session"
if grep -qE 'Failed to open display|-CRITICAL \*\*' "$work/app.log"; then fail "the app logged display errors"; fi
echo "wayland-launch: OK (ran 10 s on $display without X)"
