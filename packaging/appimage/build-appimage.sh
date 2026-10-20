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
# HarfBuzz, FriBidi, ALSA, PipeWire, shared-mime-info), which any desktop
# installation has.
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
    # deb-src entries let THIRD-PARTY-SOURCES.md list the exact source files
    # of the bundled packages.
    $sudo sed -i 's/^Types: deb$/Types: deb deb-src/' /etc/apt/sources.list.d/ubuntu.sources
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
cd "$out"
# Deploy first, then add the licence notices, then package.
linuxdeploy --appdir "$appdir" --plugin gtk \
    --executable "$appdir/usr/bin/tunemesh" \
    --executable "$appdir/usr/bin/tunemesh-worker" \
    --executable "$appdir/usr/bin/ffmpeg" \
    --executable "$appdir/usr/bin/ffprobe" \
    --desktop-file "$appdir/usr/share/applications/io.github.brainage04.TuneMesh.desktop" \
    --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/io.github.brainage04.TuneMesh.svg" \
    --custom-apprun "$repo/packaging/appimage/AppRun" \
    --library "$(/sbin/ldconfig -p | awk '/libjack\.so\.0 / { print $NF; exit }')"

# The Ubuntu packages every bundled program, library, loader, schema and icon
# comes from (linuxdeploy copies most of their copyright files already).
packages=$(
    find "$appdir/usr/bin" "$appdir/usr/lib" "$appdir/usr/share/glib-2.0" -type f \
        ! -name 'tunemesh*' ! -name '*.cache' ! -name '*.compiled' -printf '%f\n' |
        sort -u | while IFS= read -r name; do
            # "pkg[:arch][, pkg2[:arch]]: /path", keeping only the system
            # locations linuxdeploy copies from: the runner has unrelated
            # copies elsewhere (/opt/google/chrome, ...).
            dpkg -S "*/$name" 2>/dev/null |
                sed -n 's#^\([^ :,]*\)\(:[a-z0-9]*\)\{0,1\}\(, [^:]*\(:[a-z0-9]*\)\{0,1\}\)*: /\(usr/\)\{0,1\}\(lib\|bin\|share/glib-2.0\)/.*#\1#p'
        done
    echo adwaita-icon-theme
)
# shellcheck disable=SC2086 # one package name per word
packages=$(printf '%s\n' $packages | sort -u)
licences=$appdir/usr/share/licenses/tunemesh
sources=$licences/THIRD-PARTY-SOURCES.md
{
    "$repo/packaging/licenses/sources-header.sh" "AppImage ($arch)" "$version"
    cat <<EOF

## Bundled Ubuntu 24.04 packages

Every library, \`ffmpeg\`, \`ffprobe\`, the GDK-Pixbuf loaders, GSettings
schemas and Adwaita icons come unmodified from these Ubuntu 24.04 packages.
Ubuntu builds FFmpeg with \`--enable-gpl\` (and without \`--enable-nonfree\`),
so the bundled FFmpeg libraries and programs are under the GPL-2.0-or-later;
libmpv is GPL-2.0-or-later too. Build configuration of the bundled ffmpeg:

\`\`\`
$(ffmpeg -hide_banner -buildconf 2>&1 | sed -n 's/^ *//; /^--/p' | tr '\n' ' ')
\`\`\`
EOF
} >"$appdir/sources.tmp"
# shellcheck disable=SC2086
"$repo/packaging/licenses/debian-notices.sh" "$appdir" "$appdir/sources.tmp" $packages
{
    cat <<EOF

## AppImage tooling

The AppImage runtime (MIT) and the GTK start-up hook come from these tools,
used as downloaded for this build:

EOF
    for tool in linuxdeploy linuxdeploy-plugin-appimage linuxdeploy-plugin-gtk.sh; do
        echo "- $tool, SHA-256 $(sha256sum "$tools/$tool" | cut -d' ' -f1)"
    done
    echo "  (https://github.com/linuxdeploy, continuous builds; MIT licence)"
} >>"$appdir/sources.tmp"
install -Dm644 "$appdir/sources.tmp" "$sources"
rm "$appdir/sources.tmp"
cp "$sources" "$out/THIRD-PARTY-SOURCES-AppImage-$arch.md"

export LDAI_OUTPUT="$out/TuneMesh-$version-$arch.AppImage"
export LDAI_UPDATE_INFORMATION="gh-releases-zsync|brainage04|TuneMesh|latest|TuneMesh-*-$arch.AppImage.zsync"
linuxdeploy --appdir "$appdir" --output appimage
ls -l "$out"
