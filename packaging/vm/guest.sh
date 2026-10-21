#!/usr/bin/env bash
# Runs inside the test VM as the test user `tm` (passwordless sudo), over an
# SSH login, so the user's systemd runs under a real logind session. Started
# by packaging/vm/run.sh, which copies it to ~/tm with the generated library
# (~/tm/library), check-removed.sh and the packages (~/tm/pkg).
#
#   guest.sh deps <distro>             install the desktop test stack
#   guest.sh test <distro> <package>   install, test and uninstall one package
set -euo pipefail

APP_ID=io.github.brainage04.TuneMesh
MPRIS=org.mpris.MediaPlayer2.tunemesh
LIB=$HOME/Music/TuneMesh
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}

log() { printf '[guest %(%H:%M:%S)T] %s\n' -1 "$*"; }
fail() { log "FAIL: $*"; exit 1; }
# $1 = seconds, rest = command: retries every second until it succeeds.
wait_for() {
    local limit=$1
    shift
    for _ in $(seq "$limit"); do
        if "$@" >/dev/null 2>&1; then return 0; fi
        sleep 1
    done
    return 1
}

deps() {
    local distro=$1
    log "installing the test stack on $distro"
    case $distro in
    ubuntu-24.04 | ubuntu-26.04 | debian-13)
        local apt=(sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=900 -yq)
        "${apt[@]}" update
        "${apt[@]}" install --no-install-recommends sway grim pipewire pipewire-pulse \
            wireplumber dbus-user-session fonts-dejavu-core python3
        ;;
    fedora)
        sudo dnf -y -q --setopt=install_weak_deps=False install sway grim pipewire \
            pipewire-pulseaudio pipewire-utils wireplumber dejavu-sans-fonts python3
        ;;
    arch)
        sudo pacman -Sy --noconfirm --needed archlinux-keyring
        sudo pacman -Su --noconfirm
        sudo pacman -S --noconfirm --needed sway grim pipewire pipewire-pulse wireplumber \
            ttf-dejavu python
        ;;
    *) fail "unknown distro $distro" ;;
    esac

    # A null sink, the only (so the default) audio output of the VM.
    mkdir -p ~/.config/pipewire/pipewire.conf.d
    cat >~/.config/pipewire/pipewire.conf.d/50-tm-null-sink.conf <<'EOF'
context.objects = [
    { factory = adapter
        args = {
            factory.name     = support.null-audio-sink
            node.name        = tm-null
            node.description = "TuneMesh test null sink"
            media.class      = Audio/Sink
            audio.position   = [ FL FR ]
            monitor.channel-volumes = true
        }
    }
]
EOF
    mkdir -p ~/.config/sway
    cat >~/.config/sway/tm.conf <<'EOF'
output HEADLESS-1 mode 1280x800
default_border none
EOF
    systemctl --user daemon-reload
    systemctl --user restart pipewire.service wireplumber.service pipewire-pulse.service
    wait_for 30 sink_id || fail "PipeWire has no tm-null sink"
    log "PipeWire null sink tm-null is node $(sink_id)"
}

# ----------------------------------------------------------------- PipeWire

pw_json() { pw-dump 2>/dev/null; }
sink_id() {
    pw_json | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    p = (o.get("info") or {}).get("props") or {}
    if o.get("type") == "PipeWire:Interface:Node" and p.get("node.name") == "tm-null":
        print(o["id"]); break
else:
    sys.exit(1)'
}
# Succeeds when a TuneMesh playback stream is running and linked to tm-null.
stream_on_null_sink() {
    pw_json | python3 -c '
import json, sys
objs = json.load(sys.stdin)
info = lambda o: o.get("info") or {}
props = lambda o: info(o).get("props") or {}
sink = [o["id"] for o in objs if props(o).get("node.name") == "tm-null"]
streams = {
    o["id"]: info(o).get("state")
    for o in objs
    if o.get("type") == "PipeWire:Interface:Node"
    and props(o).get("media.class") == "Stream/Output/Audio"
    and "tunemesh" in (str(props(o).get("application.name", "")) + str(props(o).get("node.name", ""))).lower()
}
links = [
    o for o in objs
    if o.get("type") == "PipeWire:Interface:Link"
    and info(o).get("output-node-id") in streams and info(o).get("input-node-id") in sink
]
print("streams", streams, "links to tm-null", len(links))
sys.exit(0 if links and "running" in streams.values() else 1)'
}

# ----------------------------------------------------------------- D-Bus

mpris_get() {
    busctl --user get-property "$MPRIS" /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player "$1"
}
position() { mpris_get Position | awk '{ print $2 }'; }
playing() { [ "$(mpris_get PlaybackStatus)" = 's "Playing"' ]; }
not_playing() { ! playing; }
app_on_bus() { busctl --user status "$APP_ID"; }
mpris_on_bus() { busctl --user status "$MPRIS"; }
# $1 = action of the app's GApplication, $2 = its string parameter, if any.
activate() {
    if [ $# -ge 2 ]; then
        busctl --user call "$APP_ID" "/${APP_ID//.//}" org.gtk.Actions Activate 'sava{sv}' "$1" 1 s "$2" 0
    else
        busctl --user call "$APP_ID" "/${APP_ID//.//}" org.gtk.Actions Activate 'sava{sv}' "$1" 0 0
    fi
}
leftover_processes() { pgrep -u "$(id -u)" -x 'tunemesh|tunemesh-worker'; }
not_running() { ! leftover_processes; }
app_gone() { ! pgrep -u "$(id -u)" -x tunemesh; }
worker_running() { pgrep -u "$(id -u)" -x tunemesh-worker; }
no_appimage_mount() { ! grep -q '/tmp/.mount_' /proc/mounts; }

# The program's own scopes: `flatpak run` and `snap run` move it out of the
# service that started it (app-flatpak-<id>-<n>.scope, snap.tunemesh.*.scope),
# so stopping that service alone does not reach it.
scopes=("app-flatpak-$APP_ID-*.scope" 'snap.tunemesh.*.scope')
unit_inactive() { ! systemctl --user is-active --quiet "$1"; }
# Stops a unit and those scopes (SIGTERM) and waits for TuneMesh to exit.
# With $2, that program gets SIGTERM first and the unit a chance to end by
# itself: `systemctl stop` signals every process of the unit at once, and an
# AppImage runtime killed that way leaves its FUSE mount behind (or unmounts
# the image under the still running program). A transient unit whose program
# already exited cleanly is gone, so stop may fail; whether anything still
# runs is what counts.
stop_unit() {
    if [ -n "${2:-}" ]; then
        pkill -TERM -u "$(id -u)" -x "$2" || true
        wait_for 15 unit_inactive "$1" || true
    fi
    systemctl --user stop "$1" "${scopes[@]}" 2>/dev/null || true
    wait_for 15 not_running || fail "TuneMesh still runs 15 s after stopping $1"
}

# ----------------------------------------------------------------- test

# Removes the package under test with its format's tool.
uninstall() {
    case $kind in
    native)
        case $name in
        *.deb) sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=900 -yq purge "$pkgname" ;;
        *.rpm) sudo dnf -y remove "$pkgname" ;;
        *) sudo pacman -Rns --noconfirm "$pkgname" ;;
        esac
        ;;
    appimage) rm -f ~/Applications/TuneMesh.AppImage ;;
    snap) sudo snap remove --purge tunemesh ;;
    flatpak) flatpak uninstall --user -y --noninteractive --delete-data "$APP_ID" ;;
    esac
}

# After a failure: stop what the test started and remove the package, so
# the next package in the same VM starts clean.
# shellcheck disable=SC2329 # run by the EXIT trap
clean_up_failure() {
    [ $? -ne 0 ] || return 0
    log "cleaning up after the failure"
    systemctl --user disable tunemesh-worker.service 2>/dev/null || true
    systemctl --user stop tm-app.service tm-sway.service tunemesh-worker.service "${scopes[@]}" 2>/dev/null || true
    systemctl --user reset-failed 2>/dev/null || true
    [ -z "$kind" ] || uninstall || true
}

distro="" name="" kind="" pkgname=""
test_package() {
    local pkg=$2 app worker unit_mode cfg
    distro=$1
    name=$(basename "$pkg")
    trap clean_up_failure EXIT
    local out=~/tm/logs/$name
    mkdir -p "$out"
    rm -rf ~/.config/tunemesh ~/.local/state/tunemesh ~/.cache/tunemesh "$LIB"

    # ---- install
    case $name in
    *.deb)
        kind=native
        pkgname=$(dpkg-deb -f "$pkg" Package)
        sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=900 -yq install "$pkg"
        dpkg -L "$pkgname" >"$out/installed.txt"
        ;;
    *.rpm)
        kind=native
        pkgname=$(rpm -qp --qf '%{NAME}' "$pkg")
        sudo dnf -y install "$pkg"
        rpm -ql "$pkgname" >"$out/installed.txt"
        ;;
    *.pkg.tar.zst)
        kind=native
        pkgname=$(pacman -Qip "$pkg" | awk -F' *: ' '$1 == "Name" { print $2 }')
        sudo pacman -U --noconfirm "$pkg"
        pacman -Qlq "$pkgname" >"$out/installed.txt"
        ;;
    *.AppImage)
        kind=appimage
        # The libraries an AppImage expects from any desktop (see
        # build-appimage.sh); a server cloud image does not have them.
        if command -v apt-get >/dev/null; then
            sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=900 -yq install \
                --no-install-recommends libgl1 libegl1 libgbm1 libx11-6 libx11-xcb1 libxcb1 \
                libxcb-dri3-0 libdrm2 libwayland-client0 libfontconfig1 libfreetype6 libharfbuzz0b \
                libfribidi0 libasound2t64 libpipewire-0.3-0t64 libusb-1.0-0 shared-mime-info
        else
            sudo dnf -y -q install mesa-libGL mesa-libEGL mesa-libgbm libX11 libX11-xcb libxcb \
                libdrm libwayland-client fontconfig freetype harfbuzz fribidi alsa-lib \
                pipewire-libs libusb1 shared-mime-info
        fi
        mkdir -p ~/Applications
        cp "$pkg" ~/Applications/TuneMesh.AppImage
        chmod +x ~/Applications/TuneMesh.AppImage
        app=~/Applications/TuneMesh.AppImage
        worker="$app worker"
        ;;
    *.snap)
        kind=snap
        sudo snap wait system seed.loaded
        sudo snap install --dangerous "$pkg"
        snap connections tunemesh >"$out/snap-connections.txt"
        app=/snap/bin/tunemesh
        worker=/snap/bin/tunemesh.worker
        ;;
    *.flatpak)
        kind=flatpak
        command -v flatpak >/dev/null || sudo dnf -y -q install flatpak
        flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
        flatpak install --user -y --noninteractive "$pkg"
        app="flatpak run $APP_ID"
        worker="flatpak run --command=tunemesh-worker $APP_ID"
        ;;
    *) fail "unknown package type: $name" ;;
    esac
    if [ "$kind" = native ]; then
        app=tunemesh
        worker=tunemesh-worker
        unit_mode=shipped
    else
        unit_mode=transient
    fi
    log "installed $name ($kind)"

    # Split on purpose: APP and WORKER may be commands with arguments.
    # shellcheck disable=SC2086
    $app --version
    # shellcheck disable=SC2086
    $worker --version

    # The generated library, and the app's settings where this format keeps them.
    mkdir -p "$(dirname "$LIB")"
    cp -a ~/tm/library "$LIB"
    case $kind in
    flatpak) cfg=~/.var/app/$APP_ID/config/tunemesh ;;
    snap) cfg=~/snap/tunemesh/current/.config/tunemesh ;;
    *) cfg=~/.config/tunemesh ;;
    esac
    mkdir -p "$cfg"
    printf 'library_path = "%s"\nsetup_done = true\n' "$LIB" >"$cfg/config.toml"

    # ---- the worker as a systemd user service
    systemctl --user daemon-reload
    if [ "$unit_mode" = shipped ]; then
        systemctl --user cat tunemesh-worker.service >"$out/worker-unit.txt"
        systemctl --user enable --now tunemesh-worker.service
    else
        # No systemd unit in this format: the same command as a transient unit.
        # ExitType=cgroup: the AppImage runtime execs the program in the main
        # process and serves the image from a child that unmounts it after
        # the program exits; by default systemd kills that child as soon as
        # the main process ends, and the mount is left behind.
        # shellcheck disable=SC2086
        systemd-run --user --unit=tunemesh-worker -p Restart=no -p ExitType=cgroup -- \
            $worker --library "$LIB"
    fi
    sleep 15
    journalctl --user -u tunemesh-worker --no-pager >"$out/worker.log" 2>&1 || true
    systemctl --user status tunemesh-worker --no-pager || true
    systemctl --user is-active --quiet tunemesh-worker || fail "tunemesh-worker is not active after 15 s"
    worker_running >/dev/null || fail "no tunemesh-worker process after 15 s"
    [ "$(systemctl --user show -p NRestarts --value tunemesh-worker)" = 0 ] ||
        fail "tunemesh-worker restarted"
    log "tunemesh-worker active as a user service ($unit_mode)"
    # shellcheck disable=SC2086
    $worker --self-test --work-dir "/tmp/tm-self-test.$$" 2>&1 | tee "$out/self-test.log"
    rm -rf "/tmp/tm-self-test.$$"
    log "tunemesh-worker --self-test passed"
    if [ "$unit_mode" = transient ]; then
        stop_unit tunemesh-worker.service tunemesh-worker
    else
        systemctl --user disable tunemesh-worker.service
        stop_unit tunemesh-worker.service
    fi
    systemctl --user reset-failed 2>/dev/null || true

    # ---- the app in headless sway, playing to the null sink
    systemctl --user start pipewire.service wireplumber.service pipewire-pulse.service
    wait_for 30 sink_id || fail "PipeWire has no tm-null sink"
    systemd-run --user --unit=tm-sway -E WLR_BACKENDS=headless -E WLR_RENDERER=pixman \
        -E WLR_LIBINPUT_NO_DEVICES=1 -- sway -c "$HOME/.config/sway/tm.conf"
    local wl=
    for _ in $(seq 30); do
        wl=$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name 'wayland-*' -printf '%f\n' | sort | sed -n 1p)
        [ -n "$wl" ] && break
        sleep 1
    done
    [ -n "$wl" ] || fail "sway did not start (journalctl --user -u tm-sway)"
    log "headless sway on $wl"
    local app_start
    app_start=$(date +%s)
    # shellcheck disable=SC2086
    systemd-run --user --unit=tm-app -p ExitType=cgroup -E WAYLAND_DISPLAY="$wl" \
        -E XDG_SESSION_TYPE=wayland -E XDG_CURRENT_DESKTOP=sway -E GDK_BACKEND=wayland -- \
        $app --library "$LIB"
    wait_for 90 app_on_bus || fail "the app did not take $APP_ID on the session bus"
    wait_for 30 mpris_on_bus || fail "the app did not register $MPRIS"
    log "app on the session bus with MPRIS"

    # The library scan may still be running: ask again until it plays.
    local ok=
    for _ in $(seq 12); do
        activate play-route home >/dev/null
        if wait_for 5 playing; then ok=1; break; fi
    done
    [ -n "$ok" ] || fail "PlaybackStatus never became Playing after app.play-route"
    local p1 p2
    p1=$(position)
    sleep 5
    p2=$(position)
    mpris_get Metadata >"$out/mpris-metadata.txt"
    log "MPRIS position $p1 -> $p2 us"
    [ $((p2 - p1)) -ge 3000000 ] || fail "the MPRIS position did not advance by 3 s in 5 s"
    grep -q 'Tone' "$out/mpris-metadata.txt" || fail "MPRIS metadata has no generated track"
    stream_on_null_sink | tee "$out/pipewire-stream.txt" ||
        fail "no running TuneMesh stream linked to the tm-null sink"
    pw-dump >"$out/pw-dump.json" 2>&1 || true
    activate play-pause >/dev/null
    wait_for 5 not_playing || fail "app.play-pause did not pause playback"
    log "playback confirmed: position advanced, stream on tm-null, play-pause pauses"
    WAYLAND_DISPLAY=$wl grim "$out/screenshot.png" || true

    # SIGTERM to the app itself (as `kill` would): it saves its state and
    # exits cleanly.
    pkill -TERM -u "$(id -u)" -x tunemesh
    wait_for 15 app_gone || fail "the app did not exit within 15 s of SIGTERM"
    stop_unit tm-app.service tunemesh
    journalctl --user --since "@$app_start" -u tm-app.service -u "${scopes[0]}" -u "${scopes[1]}" \
        --no-pager >"$out/app.log" 2>&1 || true
    if systemctl --user is-failed --quiet tm-app.service; then
        fail "the app did not exit cleanly on SIGTERM"
    fi
    if grep -qE 'Failed to load icon|-CRITICAL \*\*|No GSettings schemas' "$out/app.log"; then
        fail "the app logged missing resources (see app.log)"
    fi
    journalctl --user -u tm-sway --no-pager >"$out/sway.log" 2>&1 || true
    systemctl --user stop tm-sway.service
    systemctl --user reset-failed 2>/dev/null || true

    # ---- uninstall
    uninstall
    case $kind in
    native)
        bash ~/tm/check-removed.sh "$out/installed.txt"
        systemctl --user daemon-reload
        if systemctl --user cat tunemesh-worker.service >/dev/null 2>&1; then
            fail "the tunemesh-worker unit is still known to systemd"
        fi
        [ ! -e ~/.config/systemd/user/default.target.wants/tunemesh-worker.service ] ||
            fail "the tunemesh-worker enable link is left behind"
        ;;
    appimage)
        # The runtime unmounts the image after its program exits, a moment
        # later than the program itself.
        wait_for 15 no_appimage_mount || fail "an AppImage mount is left behind"
        ;;
    snap)
        if snap list tunemesh >/dev/null 2>&1; then fail "the snap is still installed"; fi
        [ ! -e ~/snap/tunemesh ] || fail "$HOME/snap/tunemesh is left behind"
        [ ! -e /snap/tunemesh ] || fail "/snap/tunemesh is left behind"
        ;;
    flatpak)
        if flatpak info --user "$APP_ID" >/dev/null 2>&1; then fail "the Flatpak is still installed"; fi
        [ ! -e ~/.var/app/$APP_ID ] || fail "$HOME/.var/app/$APP_ID is left behind"
        ;;
    esac
    if leftover_processes; then fail "TuneMesh processes are left running"; fi
    log "uninstalled $name; nothing left behind"
    rm -rf "$LIB"
    log "PASS $name"
}

case ${1:-} in
deps) deps "$2" ;;
test) test_package "$2" "$3" ;;
*) echo "usage: $0 deps <distro> | test <distro> <package>" >&2; exit 2 ;;
esac
