#!/bin/bash
# Builds TuneMesh-<version>-<arch>.AppImage (plus its .zsync update file) on
# Ubuntu 24.04, the oldest supported base, bundling GTK 4, libadwaita, libmpv,
# ffmpeg and ffprobe:
#
#   packaging/appimage/build-appimage.sh --deps dist    # --deps: apt build deps (sudo)
#
# The AppImage needs glibc 2.39 or newer on the host (Ubuntu 24.04, Debian 13,
# Fedora 40 and later) and, like every AppImage, the host's graphics, font and
# sound libraries (Mesa/libGL/EGL, X11/xcb, Wayland, fontconfig, freetype,
# HarfBuzz, FriBidi, ALSA, PipeWire), which any desktop installation has.
# JACK is bundled, since desktops often lack it. TUNEMESH_VERSION overrides
# the version.
set -euo pipefail
# shellcheck source=packaging/linux/common.sh
. "$(dirname "$0")/../linux/common.sh"

deps=false
if [ "${1:-}" = --deps ]; then deps=true; shift; fi
out=$(mkdir -p "${1:-dist}" && cd "${1:-dist}" && pwd)
sudo=$([ "$(id -u)" -eq 0 ] || echo sudo)

if $deps; then
    $sudo apt-get update -q
    DEBIAN_FRONTEND=noninteractive $sudo apt-get install -yq --no-install-recommends \
        build-essential pkg-config ca-certificates curl file desktop-file-utils \
        libgtk-4-dev libadwaita-1-dev libmpv-dev libglib2.0-dev-bin \
        ffmpeg librsvg2-common adwaita-icon-theme gdk-pixbuf2.0-bin
fi

ensure_rust
version=${TUNEMESH_VERSION:-$(tunemesh_version)}
arch=$(uname -m)
build_binaries

tools=$(mktemp -d)
appdir=$(mktemp -d)/AppDir
trap 'rm -rf "$tools" "$(dirname "$appdir")"' EXIT
fetch() { curl -fsSL --retry 3 -o "$tools/$1" "$2" && chmod +x "$tools/$1"; }
fetch linuxdeploy "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$arch.AppImage"
fetch linuxdeploy-plugin-appimage "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-$arch.AppImage"
fetch linuxdeploy-plugin-gtk.sh "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
export PATH=$tools:$PATH
# CI runners and containers have no FUSE.
export APPIMAGE_EXTRACT_AND_RUN=1

"$repo/packaging/linux/install.sh" --destdir "$appdir" --prefix /usr
# The user unit points at /usr/bin, which does not exist for an AppImage.
rm -r "$appdir/usr/lib/systemd"
install -m755 "$(command -v ffmpeg)" "$(command -v ffprobe)" "$appdir/usr/bin/"
# Symbolic icons for the transport controls and navigation.
mkdir -p "$appdir/usr/share/icons/Adwaita"
cp -a /usr/share/icons/Adwaita/index.theme /usr/share/icons/Adwaita/symbolic "$appdir/usr/share/icons/Adwaita/" 2>/dev/null ||
    cp -a /usr/share/icons/Adwaita/. "$appdir/usr/share/icons/Adwaita/"

export DEPLOY_GTK_VERSION=4
export LDAI_OUTPUT="$out/TuneMesh-$version-$arch.AppImage"
export LDAI_UPDATE_INFORMATION="gh-releases-zsync|brainage04|TuneMesh|latest|TuneMesh-*-$arch.AppImage.zsync"
cd "$out"
linuxdeploy --appdir "$appdir" --plugin gtk \
    --executable "$appdir/usr/bin/tunemesh" \
    --executable "$appdir/usr/bin/tunemesh-worker" \
    --executable "$appdir/usr/bin/ffmpeg" \
    --executable "$appdir/usr/bin/ffprobe" \
    --desktop-file "$appdir/usr/share/applications/io.github.brainage04.TuneMesh.desktop" \
    --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/io.github.brainage04.TuneMesh.svg" \
    --custom-apprun "$repo/packaging/appimage/AppRun" \
    --library "$(/sbin/ldconfig -p | awk '/libjack\.so\.0 / { print $NF; exit }')" \
    --output appimage
ls -l "$out"
