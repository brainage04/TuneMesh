#!/bin/sh
# Playback test for an installed TuneMesh package: starts the app headless on a
# one-track library, plays the album over D-Bus and requires the MPRIS
# position to advance, which only happens when libmpv really plays the file
# into a sound server (a PulseAudio null sink started here when none runs).
# PlaybackStatus alone is not enough: it can say Playing while mpv is idle.
#
# Run it on a session bus: `dbus-run-session -- playback.sh`, or directly on
# the user's bus where the format needs that (snaps). Needs xvfb-run, gdbus
# (libglib2.0-bin) and, when no sound server runs yet, pulseaudio.
#
# APP is the command that runs the app (default `tunemesh`). LIB_PARENT is
# where the test library is created (default a temp dir). CONFIG_HOME is the
# app's config directory (default a temp dir exported as XDG_CONFIG_HOME;
# set it for formats that choose their own, such as ~/snap/tunemesh/current/.config).
set -eu

APP=${APP:-tunemesh}
parent=${LIB_PARENT:-${TMPDIR:-/tmp}}
mkdir -p "$parent"
lib=$(mktemp -d "$parent/tunemesh-play.XXXXXX")
work=$(mktemp -d "${TMPDIR:-/tmp}/tunemesh-play-work.XXXXXX")
app_pid=
pulse_pid=
cleanup() {
    [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null
    [ -n "$pulse_pid" ] && kill "$pulse_pid" 2>/dev/null
    rm -rf "$lib" "$work"
}
trap cleanup EXIT

fail() {
    echo "playback: FAIL: $*" >&2
    [ -f "$work/app.log" ] && { echo "--- app log" >&2; cat "$work/app.log" >&2; }
    exit 1
}

# One 20 s track of quiet noise as 8 kHz 16-bit mono WAV; folders give it
# artist "Smoke Artist" and album "Smoke Album".
# Little-endian 32-bit integer as raw bytes (the octal escapes are the format).
# shellcheck disable=SC2059
le32() {
    printf "\\$(printf %03o $(($1 & 255)))\\$(printf %03o $(($1 >> 8 & 255)))"
    printf "\\$(printf %03o $(($1 >> 16 & 255)))\\$(printf %03o $(($1 >> 24 & 255)))"
}
mkdir -p "$lib/.tunemesh/requests" "$lib/Playlists" "$lib/Smoke Artist/Smoke Album"
track="$lib/Smoke Artist/Smoke Album/Tone.wav"
bytes=320000
{
    printf RIFF; le32 $((36 + bytes)); printf 'WAVEfmt '
    le32 16; printf '\001\000\001\000'; le32 8000; le32 16000; printf '\002\000\020\000'
    printf data; le32 $bytes
    # Keep the low bits only: quiet noise.
    head -c $bytes /dev/urandom | tr '\000-\377' '\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017\000-\017'
} >"$track"

if [ -z "${CONFIG_HOME:-}" ]; then
    CONFIG_HOME=$work/config
    export XDG_CONFIG_HOME="$CONFIG_HOME"
fi
mkdir -p "$CONFIG_HOME/tunemesh"
# setup_done skips the first-run assistant.
printf 'library_path = "%s"\nsetup_done = true\n' "$lib" >"$CONFIG_HOME/tunemesh/config.toml"

if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    XDG_RUNTIME_DIR=$work/run
    mkdir -m 700 "$XDG_RUNTIME_DIR"
    export XDG_RUNTIME_DIR
fi
if ! pactl info >/dev/null 2>&1; then
    command -v pulseaudio >/dev/null 2>&1 || fail "no sound server and no pulseaudio to start one"
    # A private home: CI containers run as root with a HOME owned by another
    # user, which pulseaudio refuses. Anonymous auth, so clients need no cookie.
    mkdir -m 700 "$work/pulse-home"
    HOME=$work/pulse-home pulseaudio -n --daemonize=no --exit-idle-time=-1 --log-target=stderr \
        --load=module-null-sink --load='module-native-protocol-unix auth-anonymous=1' \
        2>"$work/pulse.log" &
    pulse_pid=$!
    for _ in $(seq 20); do pactl info >/dev/null 2>&1 && break; sleep 0.5; done
    pactl info >/dev/null 2>&1 || { cat "$work/pulse.log" >&2; fail "pulseaudio did not start"; }
fi

# APP is a command with arguments: split it on purpose.
# shellcheck disable=SC2086
xvfb-run -a env GDK_BACKEND=x11 GSK_RENDERER=cairo NO_AT_BRIDGE=1 \
    $APP --library "$lib" >"$work/app.log" 2>&1 &
app_pid=$!

app_bus=io.github.brainage04.TuneMesh
mpris=org.mpris.MediaPlayer2.tunemesh
has_owner() {
    gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus \
        --method org.freedesktop.DBus.NameHasOwner "$1" 2>/dev/null | grep -q true
}
activate() {
    gdbus call --session --dest "$app_bus" --object-path /io/github/brainage04/TuneMesh \
        --method org.gtk.Actions.Activate "$1" "$2" '{}' >/dev/null
}
player_prop() {
    gdbus call --session --dest "$mpris" --object-path /org/mpris/MediaPlayer2 \
        --method org.freedesktop.DBus.Properties.Get org.mpris.MediaPlayer2.Player "$1"
}
position() { player_prop Position | sed -n 's/.*int64 \([0-9-]*\).*/\1/p'; }

for _ in $(seq 60); do
    has_owner "$app_bus" && has_owner "$mpris" && break
    kill -0 "$app_pid" 2>/dev/null || fail "the app exited before it was on the bus"
    sleep 0.5
done
has_owner "$app_bus" || fail "the app did not take $app_bus within 30 s"
has_owner "$mpris" || fail "the app did not take $mpris within 30 s"

# The first scan may still be running: retry until the album is known.
playing=false
for _ in $(seq 10); do
    activate play-route "[<'album:Smoke Album'>]"
    sleep 2
    if player_prop PlaybackStatus | grep -q Playing; then playing=true; break; fi
done
$playing || fail "PlaybackStatus never became Playing"
player_prop Metadata | grep -q 'Tone.wav' || fail "the playing track is not the test file: $(player_prop Metadata)"

first=$(position)
sleep 3
second=$(position)
echo "MPRIS position: ${first:-?} us, 3 s later ${second:-?} us"
if [ -z "$first" ] || [ -z "$second" ]; then fail "no MPRIS position"; fi
[ $((second - first)) -ge 1000000 ] || fail "the position did not advance: libmpv is not playing"

activate quit '[]' || true
for _ in $(seq 20); do kill -0 "$app_pid" 2>/dev/null || break; sleep 0.5; done
if kill -0 "$app_pid" 2>/dev/null; then fail "the app did not quit"; fi
app_pid=
if grep -qE -- '-CRITICAL \*\*' "$work/app.log"; then fail "the app logged critical errors"; fi
echo "playback: OK"
