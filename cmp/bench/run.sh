#!/usr/bin/env bash
# Reproduces the GTK vs Compose comparison on NixOS: tools, test library, both release builds, then bench.py.
# Everything lives under $BENCH_DIR (default /tmp/tunemesh-cmp-bench); builds and app runs go through `heavy`.
set -euo pipefail

B=${BENCH_DIR:-/tmp/tunemesh-cmp-bench}
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$B/bin"

# The main output only: `head -n1` of a multi-output package can be its man pages.
build() { nix build --no-link --print-out-paths --inputs-from "$ROOT" "$1^out"; }
ln -sf "$(build nixpkgs#sway)/bin/sway" "$(build nixpkgs#grim)/bin/grim" "$(nix build --no-link --print-out-paths nixpkgs#glib.bin)/bin/gdbus" "$B/bin/"
LIBGLVND=$(build nixpkgs#libglvnd)
LIBMPV=$(build nixpkgs#mpv-unwrapped)

[ -d "$B/library" ] || python3 "$ROOT/cmp/bench/gen_library.py" "$B/library"

setsid --fork --wait heavy --quick --mem 4G --label cmp-gtk-build -- nix build "$ROOT#tunemesh" -o "$B/gtk-result"

export JAVA_HOME
JAVA_HOME="$(build nixpkgs#jdk25)/lib/openjdk"
(cd "$ROOT/cmp" && setsid --fork --wait heavy --quick --mem 4G --label cmp-build -- \
    ./gradlew --no-daemon -q -PlibmpvDir="$LIBMPV/lib" :shared:test :desktop:createReleaseDistributable)
rm -rf "$B/cmp-app"
cp -a "$ROOT/cmp/desktop/build/compose/binaries/main-release/app/tunemesh-cmp" "$B/cmp-app"

LEAN="-XX:+UseSerialGC -Xmx256m -Xss512k -XX:ReservedCodeCacheSize=48m"
setsid --fork --wait heavy --mem 3G --label cmp-bench -- python3 "$ROOT/cmp/bench/bench.py" \
    --gtk "$B/gtk-result/bin/tunemesh" --cmp "$B/cmp-app/bin/tunemesh-cmp" \
    --cmp-ld-path "$LIBGLVND/lib:/run/opengl-driver/lib" \
    --library "$B/library" --out "$B/results" --tools "$B/bin" --gdbus "$B/bin/gdbus" \
    --screenshots "$ROOT/screenshots" --album "Gold Neon Static" --artist "Kara Öve" --gpu \
    --runs 3 --first-launch-runs 3 --frames-runs 3 \
    --cmp-variant "lean=$LEAN" --cmp-variant "lean-aot=$LEAN -XX:AOTCache={aot}"
