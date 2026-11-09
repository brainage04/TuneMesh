#!/usr/bin/env bash
# Retakes the desktop screenshots docs/screenshots/{home,album,downloads,
# setup-syncthing}.png (1440x900):
# generates a library of sine and noise tracks with placeholder names, its
# download requests and a downloader heartbeat, and a throwaway Syncthing
# that shares it, then runs gui_tests::screenshots in a private headless
# sway. Everything runs in user, mount, UTS, PID and network namespaces: the
# home folder is a scratch folder shown as /home/user, the host name is
# desktop-pc, Syncthing and the app reach nothing but each other, and
# whatever the script started ends with it. Never touches your library,
# your Syncthing or your desktop.
#
# Two runs make the same pictures: the covers are fixed gradients, the app's
# clock is set to 2026-10-19 14:30 UTC with faketime, and Syncthing uses the
# throwaway key in screenshots-syncthing/ (so the same device ID and QR code).
#
# Run from the dev shell with the tools on PATH:
#   nix develop -c nix shell --inputs-from . nixpkgs#sway nixpkgs#grim \
#     nixpkgs#dbus nixpkgs#syncthing nixpkgs#ffmpeg nixpkgs#curl \
#     nixpkgs#iproute2 nixpkgs#util-linux nixpkgs#libfaketime -c \
#     linux/crates/spotless/scripts/screenshots.sh [OUT-DIR]
#
# OUT-DIR defaults to docs/screenshots.
set -euo pipefail

scripts=$(cd "$(dirname "$0")" && pwd)
linux=$(cd "$scripts/../../.." && pwd)
out=$(realpath -m "${1:-$linux/../docs/screenshots}")
mkdir -p "$out"
base=/var/tmp
[ -d "$base" ] || base=${TMPDIR:-/tmp}
run=$(mktemp -d "$base/spotless-shots.XXXXXX")
trap 'rm -rf "$run"' EXIT

# The test binary, copied out of the checkout: /home is replaced below.
cd "$linux"
cargo test -p spotless --locked --no-run >"$run/build.log" 2>&1 || {
	cat "$run/build.log" >&2
	exit 1
}
bin=$(sed -n 's/^ *Executable unittests src\/main.rs (\(.*\))$/\1/p' "$run/build.log")
[ -n "$bin" ] || {
	cat "$run/build.log" >&2
	echo "no test binary for src/main.rs" >&2
	exit 1
}
cp "$bin" "$run/spotless-tests"

# ---- the library ----------------------------------------------------------
home=$run/home
lib=$home/Music/Spotless
covers=$run/covers
mkdir -p "$lib/Playlists" "$covers"

cover() { # name color0 color1
	# The end point must lie inside the frame (0..499) and the seed be set:
	# otherwise the gradients source picks random ones on every run.
	ffmpeg -loglevel error -y -f lavfi \
		-i "gradients=s=500x500:c0=$2:c1=$3:x0=0:y0=0:x1=499:y1=499:speed=0:d=1:seed=1" -frames:v 1 "$covers/$1.png"
}

track() { # artist album title n total ext source seconds cover id
	local artist=$1 album=$2 title=$3 n=$4 total=$5 ext=$6 src=$7 dur=$8 cov=$9 id=${10}
	local dir="$lib/$artist/$album" codec art=()
	mkdir -p "$dir"
	case $ext in
	mp3) codec=(-c:a libmp3lame -b:a 96k) art=(-i "$covers/$cov.png" -map 0:a -map 1:v -c:v png -disposition:v attached_pic -id3v2_version 3) ;;
	flac | m4a)
		codec=(-c:a flac)
		[ "$ext" = m4a ] && codec=(-c:a aac -b:a 96k)
		art=(-i "$covers/$cov.png" -map 0:a -map 1:v -c:v png -disposition:v attached_pic)
		;;
	opus | webm) codec=(-c:a libopus -b:a 64k) ;;
	esac
	ffmpeg -loglevel error -y -f lavfi -i "$src:d=$dur" "${art[@]}" "${codec[@]}" \
		-metadata title="$title" -metadata artist="$artist" -metadata album_artist="$artist" \
		-metadata album="$album" -metadata track="$n/$total" -metadata date=2026 "$dir/$title [$id].$ext"
	if [ "$ext" = opus ] || [ "$ext" = webm ]; then cp "$covers/$cov.png" "$dir/cover.png"; fi
}

cover pure 0x1db954 0x0b3d20
cover square 0xe8115b 0x3d0b1c
cover noise 0x509bf5 0x0b1f3d
cover echo 0xf59b23 0x3d250b
cover dusk 0x8d67ab 0x1f0b3d

track "Aurora Lane" "Pure Tones" "Morning A" 1 4 mp3 "sine=f=440" 148 pure pt01
track "Aurora Lane" "Pure Tones" "Second Harmonic" 2 4 mp3 "sine=f=880" 173 pure pt02
track "Aurora Lane" "Pure Tones" "Low Hum" 3 4 mp3 "sine=f=110" 201 pure pt03
track "Aurora Lane" "Pure Tones" "Fifth Above" 4 4 mp3 "sine=f=660" 132 pure pt04
track "The Oscillators" "Square Days" "Duty Cycle" 1 3 opus "sine=f=330" 187 square sq01
track "The Oscillators" "Square Days" "Edge Rider" 2 3 opus "sine=f=495" 164 square sq02
track "The Oscillators" "Square Days" "Clocked Out" 3 3 opus "sine=f=247" 222 square sq03
track "Nova Field" "White Noise Sessions" "Static Bloom" 1 4 flac "anoisesrc=c=white:a=0.1" 95 noise nf01
track "Nova Field" "White Noise Sessions" "Pink Hours" 2 4 flac "anoisesrc=c=pink:a=0.1" 120 noise nf02
track "Nova Field" "White Noise Sessions" "Brown Study" 3 4 flac "anoisesrc=c=brown:a=0.1" 110 noise nf03
track "Nova Field" "White Noise Sessions" "Blue Shift" 4 4 m4a "anoisesrc=c=blue:a=0.1" 140 noise nf04
track "Juniper Echo" "Long Shadows" "Lantern" 1 2 webm "sine=f=523" 205 echo je01
track "Juniper Echo" "Long Shadows" "Paper Boats" 2 2 webm "sine=f=392" 191 echo je02
track "Velvet Static" "Dusk Signals" "Night Bus" 1 3 m4a "sine=f=294" 176 dusk vs01
track "Velvet Static" "Dusk Signals" "Neon Rain" 2 3 m4a "sine=f=370" 158 dusk vs02
track "Velvet Static" "Dusk Signals" "Last Train" 3 3 m4a "sine=f=587" 199 dusk vs03

printf '#EXTM3U\n../Velvet Static/Dusk Signals/Night Bus [vs01].m4a\n../Aurora Lane/Pure Tones/Morning A [pt01].mp3\n../Juniper Echo/Long Shadows/Lantern [je01].webm\n../The Oscillators/Square Days/Edge Rider [sq02].opus\n' >"$lib/Playlists/Road Trip.m3u8"
printf '#EXTM3U\n../Nova Field/White Noise Sessions/Pink Hours [nf02].flac\n../Nova Field/White Noise Sessions/Brown Study [nf03].flac\n../Aurora Lane/Pure Tones/Low Hum [pt03].mp3\n' >"$lib/Playlists/Focus.m3u8"
printf '#EXTM3U\n../The Oscillators/Square Days/Duty Cycle [sq01].opus\n../Aurora Lane/Pure Tones/Second Harmonic [pt02].mp3\n../Velvet Static/Dusk Signals/Neon Rain [vs02].m4a\n' >"$lib/Playlists/Liked Songs.m3u8"
# Recently added, newest first: Pure Tones, Long Shadows, White Noise
# Sessions, Square Days, Dusk Signals.
i=0
for album in "Velvet Static/Dusk Signals" "The Oscillators/Square Days" "Nova Field/White Noise Sessions" \
	"Juniper Echo/Long Shadows" "Aurora Lane/Pure Tones"; do
	i=$((i + 1))
	find "$lib/$album" -type f -exec touch -d "2026-10-1$i 12:00" {} +
done

# ---- a throwaway Syncthing ------------------------------------------------
# Its key is a fixed throwaway one, kept with this script, so the device ID
# (and the QR code in the setup picture) is the same on every run; generate
# keeps an existing key.
st=$home/.local/state/syncthing
mkdir -p "$st"
cp "$scripts/screenshots-syncthing/cert.pem" "$scripts/screenshots-syncthing/key.pem" "$st/"
chmod 600 "$st/key.pem"
syncthing generate --home "$st" --no-port-probing >/dev/null 2>&1
sed -i -e 's#<address>127.0.0.1:8384</address>#<address>127.0.0.1:18384</address>#' \
	-e 's#<listenAddress>default</listenAddress>#<listenAddress>tcp://127.0.0.1:22999</listenAddress>#' \
	-e 's#AnnounceEnabled>true#AnnounceEnabled>false#g' -e 's#<relaysEnabled>true#<relaysEnabled>false#' \
	-e 's#<startBrowser>true#<startBrowser>false#' -e 's#<natEnabled>true#<natEnabled>false#' \
	-e 's#<urAccepted>0#<urAccepted>-1#' -e 's#<crashReportingEnabled>true#<crashReportingEnabled>false#' \
	"$st/config.xml"
grep -q '127.0.0.1:18384' "$st/config.xml"

# ---- fonts: the same on every machine -----------------------------------
repo=$(cd "$linux/.." && pwd)
noto=$(nix build --inputs-from "$repo" --no-link --print-out-paths nixpkgs#noto-fonts-lgc-plus)
dejavu=$(nix build --inputs-from "$repo" --no-link --print-out-paths nixpkgs#dejavu_fonts)
cat >"$run/fonts.conf" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$noto/share/fonts</dir>
  <dir>$dejavu/share/fonts</dir>
  <cachedir>$run/fontconfig</cachedir>
  <!-- GTK's and libadwaita's default families included: every text in Noto
       Sans, monospace in DejaVu Sans Mono. -->
  <alias binding="same"><family>sans-serif</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>Sans</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>serif</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>system-ui</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>Adwaita Sans</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>Cantarell</family><prefer><family>Noto Sans</family></prefer></alias>
  <alias binding="same"><family>monospace</family><prefer><family>DejaVu Sans Mono</family></prefer></alias>
  <alias binding="same"><family>Monospace</family><prefer><family>DejaVu Sans Mono</family></prefer></alias>
  <alias binding="same"><family>Adwaita Mono</family><prefer><family>DejaVu Sans Mono</family></prefer></alias>
  <match target="pattern"><edit name="family" mode="append_last"><string>Noto Sans</string></edit></match>
</fontconfig>
EOF

# ---- inside the namespaces ------------------------------------------------
cat >"$run/inside.sh" <<'EOF'
#!/usr/bin/env bash
# Runs as the namespace's root: $1 is the scratch folder, $2 the output.
set -euo pipefail
run=$1 out=$2
# This shell is PID 1 of its own PID namespace: everything it starts ends
# when it exits. Stop Syncthing first anyway, so it can close its database,
# and show the logs when something failed. As PID 1 the shell gets no
# default handlers for INT and TERM, hence the explicit traps.
st_pid=
finish() {
	local rc=$?
	trap - EXIT
	if [ -n "$st_pid" ]; then
		kill "$st_pid" 2>/dev/null || true
		wait "$st_pid" 2>/dev/null || true
	fi
	if [ "$rc" != 0 ]; then
		echo "screenshots.sh: failed (exit $rc); the last lines of the logs:" >&2
		tail -n 30 "$run/sway.log" "$run/syncthing.log" >&2 2>/dev/null || true
	fi
	exit "$rc"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
hostname desktop-pc
ip link set lo up
mount -t tmpfs tmpfs /home
mkdir -p /home/user
mount --bind "$run/home" /home/user
export HOME=/home/user
export XDG_STATE_HOME=$HOME/.local/state XDG_CACHE_HOME=$HOME/.cache XDG_DATA_HOME=$HOME/.local/share
export XDG_CONFIG_HOME=$run/config XDG_RUNTIME_DIR=$run/xdg FONTCONFIG_FILE=$run/fonts.conf
mkdir -p "$XDG_CONFIG_HOME/gtk-4.0" "$XDG_RUNTIME_DIR" "$run/shots"
# A close button only, as GNOME shows it.
printf '[Settings]\ngtk-decoration-layout=:close\n' >"$XDG_CONFIG_HOME/gtk-4.0/settings.ini"
chmod 700 "$XDG_RUNTIME_DIR"
lib=$HOME/Music/Spotless

STNODEFAULTFOLDER=1 STNOUPGRADE=1 syncthing serve --home "$XDG_STATE_HOME/syncthing" \
	--no-browser --no-restart --no-upgrade >"$run/syncthing.log" 2>&1 &
st_pid=$!
key=$(sed -n 's#.*<apikey>\(.*\)</apikey>.*#\1#p' "$XDG_STATE_HOME/syncthing/config.xml")
api() { curl -sf -H "X-API-Key: $key" "$@"; }
status=
for _ in $(seq 100); do
	status=$(api http://127.0.0.1:18384/rest/system/status 2>/dev/null) && break
	status=
	sleep 0.2
done
if [ -z "$status" ]; then
	echo "screenshots.sh: the throwaway Syncthing's API at http://127.0.0.1:18384 did not answer within 20 s" >&2
	exit 1
fi
my_id=$(printf '%s' "$status" | sed -n 's/.*"myID": *"\([^"]*\)".*/\1/p')
api -X PUT -H 'Content-Type: application/json' \
	-d "{\"id\":\"spotless\",\"label\":\"Spotless\",\"path\":\"$lib\",\"type\":\"sendreceive\",\"devices\":[{\"deviceID\":\"$my_id\"}]}" \
	http://127.0.0.1:18384/rest/config/folders/spotless >/dev/null

# Requests from two devices, their statuses, and a fresh heartbeat from the
# downloader on home-server.
state=$lib/.spotless
mkdir -p "$state/requests" "$state/status"
# The app's clock (faketime, below) starts at this time too.
now=2026-10-19T14:30:00Z
printf '{"version":1,"device":"home-server","machine":"0f3c9a1b2d4e5f600f3c9a1b2d4e5f60","instance":"0f3c9a1b2d4e5f60","pid":4242,"workerVersion":"0.1.0","heartbeatAt":"%s"}\n' "$now" >"$state/worker.json"
request() { # id input requestedBy playlistFromSource
	printf '{"version":1,"id":"%s","input":"%s","playlist":null,"playlistFromSource":%s,"requestedBy":"%s","requestedAt":"%s"}\n' \
		"$1" "$2" "$4" "$3" "$now" >"$state/requests/$1.json"
}
request 1792410060000-0000aa02 "https://open.spotify.com/playlist/0aExamplePlaylist0000" pixel-8 true
request 1792410120000-0000aa03 "velvet static last train" pixel-8 false
request 1792410180000-0000aa04 "https://www.youtube.com/watch?v=aExampleId0" desktop-pc false
cat >"$state/status/1792410000000-0000aa01.json" <<JSON
{"version":1,"id":"1792410000000-0000aa01","input":"https://music.youtube.com/playlist?list=OLAK5uy_mExampleAlbum0000000000000","state":"done","title":"Dusk Signals","progress":1.0,"message":null,"tracks":["Velvet Static/Dusk Signals/Night Bus [vs01].m4a","Velvet Static/Dusk Signals/Neon Rain [vs02].m4a","Velvet Static/Dusk Signals/Last Train [vs03].m4a"],"itemsDone":3,"itemsTotal":3,"updatedAt":"$now"}
JSON
cat >"$state/status/1792410060000-0000aa02.json" <<JSON
{"version":1,"id":"1792410060000-0000aa02","input":"https://open.spotify.com/playlist/0aExamplePlaylist0000","state":"downloading","title":"Evening Drive","progress":0.45,"message":null,"tracks":[],"itemsDone":7,"itemsTotal":12,"failures":[{"item":"Nova Field - Static Bloom (Live)","error":"no matching song found on YouTube Music"}],"warnings":["2 songs are longer than the 4 h limit and were skipped"],"updatedAt":"$now"}
JSON
cat >"$state/status/1792410120000-0000aa03.json" <<JSON
{"version":1,"id":"1792410120000-0000aa03","input":"velvet static last train","state":"queued","title":null,"progress":null,"message":null,"tracks":[],"updatedAt":"$now"}
JSON

# The headless compositor and a session bus that starts no services.
cat >"$run/bus.conf" <<CONF
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
CONF
printf '%s\n' 'output HEADLESS-1 resolution 1440x900 scale 1' 'default_border none' 'xwayland disable' \
	'seat seat0 hide_cursor 100' >"$run/sway.cfg"
dbus-run-session --config-file="$run/bus.conf" -- bash -c '
	set -euo pipefail
	run=$1 out=$2
	WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
		sway --config "$run/sway.cfg" >"$run/sway.log" 2>&1 &
	sway_pid=$!
	for _ in $(seq 100); do [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break; sleep 0.1; done
	export WAYLAND_DISPLAY=wayland-1 GDK_BACKEND=wayland GDK_DEBUG=no-portals GSK_RENDERER=cairo GTK_A11Y=none
	unset DISPLAY
	rc=0
	# A fixed wall clock for the app (the Home greeting, the times on the
	# Downloads page); the monotonic clock and file times stay real.
	TZ=UTC DONT_FAKE_MONOTONIC=1 NO_FAKE_STAT=1 \
		SPOTLESS_SHOTS_LIBRARY=$HOME/Music/Spotless SPOTLESS_SHOTS_OUT=$run/shots \
		faketime -f "@2026-10-19 14:30:00" \
		"$run/spotless-tests" --ignored --exact --nocapture gui_tests::screenshots::docs_screenshots || rc=$?
	kill "$sway_pid"; wait "$sway_pid" 2>/dev/null || true
	exit "$rc"
' bash "$run" "$out"
EOF
chmod +x "$run/inside.sh"
unshare --map-root-user --mount --uts --net --pid --fork --kill-child --mount-proc \
	"$run/inside.sh" "$run" "$out"
for name in home album downloads setup-syncthing; do cp "$run/shots/$name.png" "$out/"; done
echo "wrote home, album, downloads and setup-syncthing.png to $out"
