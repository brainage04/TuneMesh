#!/usr/bin/env bash
# The desktop app's GUI checks, in a private headless sway with its own
# session bus (never the desktop you are using):
#
#   leaks   every page, panel and dialog is freed once closed (debug build)
#   stalls  the hostile-library walk stays under the main-loop stall budget
#           (release build; generates a 1.5 GB library under /var/tmp)
#   all     both (the default)
#
# Run from the dev shell, with sway and dbus on PATH:
#   nix develop -c nix shell --inputs-from . nixpkgs#sway nixpkgs#dbus -c \
#     linux/crates/tunemesh/scripts/gui-checks.sh [leaks|stalls|all]
#
# Extra arguments go to `cargo test`. Environment: TUNEMESH_STALL_BUDGET_MS
# (default 100), TUNEMESH_STALL_ROUNDS (default 3), TUNEMESH_HOSTILE_LIBRARY
# (reuse a generated library), TUNEMESH_STALL_REPORT (also write the stall
# table to this file), GSK_RENDERER (default cairo).
set -euo pipefail

what=${1:-all}
[ $# -gt 0 ] && shift
case $what in
leaks | stalls | all) ;;
*)
	echo "usage: $0 [leaks|stalls|all] [cargo test arguments]" >&2
	exit 2
	;;
esac

linux=$(cd "$(dirname "$0")/../../.." && pwd)
run=$(mktemp -d "${TMPDIR:-/tmp}/tunemesh-gui.XXXXXX")
sway_pid=
bus_pid=
cleanup() {
	if [ -n "$sway_pid" ]; then
		kill "$sway_pid" 2>/dev/null || true
		wait "$sway_pid" 2>/dev/null || true
	fi
	if [ -n "$bus_pid" ]; then kill "$bus_pid" 2>/dev/null || true; fi
	rm -rf "$run"
}
trap cleanup EXIT

export XDG_RUNTIME_DIR=$run
# A session bus that starts no services (no portals, no keyring), shared by
# sway and the tests. It has to exist before sway starts: nixpkgs' sway
# wrapper starts its own bus from the system's session.conf when it finds
# none, and CI runners have no /etc/dbus-1/session.conf.
cat >"$run/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$run</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
exec 3< <(dbus-daemon --nofork --print-address=1 --print-pid=1 --config-file="$run/bus.conf")
read -r DBUS_SESSION_BUS_ADDRESS <&3
read -r bus_pid <&3
export DBUS_SESSION_BUS_ADDRESS

printf '%s\n' 'output HEADLESS-1 resolution 1920x1080' 'default_border none' >"$run/sway.cfg"
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
	sway --config "$run/sway.cfg" >"$run/sway.log" 2>&1 &
sway_pid=$!
for _ in $(seq 100); do
	[ -S "$run/wayland-1" ] && break
	if ! kill -0 "$sway_pid" 2>/dev/null; then
		cat "$run/sway.log" >&2
		exit 1
	fi
	sleep 0.1
done
[ -S "$run/wayland-1" ] || {
	echo "sway did not start" >&2
	cat "$run/sway.log" >&2
	exit 1
}
export WAYLAND_DISPLAY=wayland-1 GDK_BACKEND=wayland
unset DISPLAY
# None of the desktop's GTK settings or portals. Software rendering with
# cairo: the headless compositor has no GPU, and GL through llvmpipe spends
# most of a frame copying pixels.
export XDG_CONFIG_HOME=$run/config GDK_DEBUG=no-portals GSK_RENDERER=${GSK_RENDERER:-cairo}

cd "$linux"
if [ "$what" != stalls ]; then
	cargo test -p tunemesh "$@" -- --ignored --exact \
		gui_tests::leaks::closed_pages_are_freed
fi
if [ "$what" != leaks ]; then
	cargo test --release -p tunemesh "$@" -- --ignored --exact --nocapture \
		gui_tests::stalls::hostile_walk_stays_responsive
fi
