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
        ffmpeg zsync librsvg2-common adwaita-icon-theme gdk-pixbuf2.0-bin
fi

ensure_rust
version=${TUNEMESH_VERSION:-$(tunemesh_version)}
arch=$(uname -m)
build_binaries

tools=$(mktemp -d)
appdir=$(mktemp -d)/AppDir
trap 'rm -rf "$tools" "$(dirname "$appdir")"' EXIT
# Pinned tool versions, each checked against its SHA-256 before it runs. To
# update: pick the new release or commit and replace its hash with the output
# of `curl -fsSL <url> | sha256sum`.
linuxdeploy_tag=1-alpha-20251107-1
plugin_appimage_tag=1-alpha-20250213-1
plugin_gtk_commit=7a3fbc31a9e5075073ff8790f26effbac5f84453
# The AppImage runtime (the AppImage's executable header); appimagetool would
# otherwise download the moving "continuous" build at packaging time.
runtime_tag=20251108
case $arch in
    x86_64)
        linuxdeploy_sha=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
        plugin_appimage_sha=992d502a248e14ab185448ddf6f6e7d25558cb84d4623c354c3af350c25fccb3
        runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64)
        linuxdeploy_sha=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff
        plugin_appimage_sha=83c292149274965a865dcd44c135cfca8ba28c6b7de3eb628d4b8b5f248af17c
        runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
    *) echo "build-appimage.sh: unsupported architecture $arch" >&2; exit 1 ;;
esac
plugin_gtk_sha=b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171
fetch() {
    curl -fsSL --retry 3 -o "$tools/$1" "$3"
    echo "$2  $tools/$1" | sha256sum -c --quiet -
    chmod +x "$tools/$1"
}
fetch linuxdeploy "$linuxdeploy_sha" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/$linuxdeploy_tag/linuxdeploy-$arch.AppImage"
fetch linuxdeploy-plugin-appimage "$plugin_appimage_sha" \
    "https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/$plugin_appimage_tag/linuxdeploy-plugin-appimage-$arch.AppImage"
fetch linuxdeploy-plugin-gtk.sh "$plugin_gtk_sha" \
    "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/$plugin_gtk_commit/linuxdeploy-plugin-gtk.sh"
fetch runtime "$runtime_sha" \
    "https://github.com/AppImage/type2-runtime/releases/download/$runtime_tag/runtime-$arch"
export LDAI_RUNTIME_FILE="$tools/runtime"
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

# linuxdeploy wraps the custom AppRun in one that sources the GTK hook first.
# The hook pins GDK_BACKEND=x11 (a GTK 3 workaround: GTK 4 runs natively on
# Wayland, and a Wayland-only session has no X server) and the Adwaita GTK
# theme (libadwaita styles itself and warns about GTK_THEME), overriding the
# user's own values; drop both pins.
hook=$appdir/apprun-hooks/linuxdeploy-plugin-gtk.sh
grep -q '^export GDK_BACKEND=' "$hook"
sed -i -e '/^export GDK_BACKEND=/d' -e '/^export GTK_THEME=/d' "$hook"
grep -q 'apprun-hooks/"linuxdeploy-plugin-gtk.sh"' "$appdir/AppRun"

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

The AppImage runtime and the GTK start-up hook come from these tools (MIT
licence), used as downloaded for this build:

EOF
    for tool in runtime linuxdeploy linuxdeploy-plugin-appimage linuxdeploy-plugin-gtk.sh; do
        echo "- $tool, SHA-256 $(sha256sum "$tools/$tool" | cut -d' ' -f1)"
    done
    echo "  (https://github.com/linuxdeploy: linuxdeploy $linuxdeploy_tag, linuxdeploy-plugin-appimage"
    echo "  $plugin_appimage_tag, linuxdeploy-plugin-gtk commit $plugin_gtk_commit;"
    echo "  https://github.com/AppImage/type2-runtime release $runtime_tag)"
} >>"$appdir/sources.tmp"
install -Dm644 "$appdir/sources.tmp" "$sources"
rm "$appdir/sources.tmp"
cp "$sources" "$out/THIRD-PARTY-SOURCES-AppImage-$arch.md"

export LDAI_OUTPUT="$out/TuneMesh-$version-$arch.AppImage"
export LDAI_UPDATE_INFORMATION="gh-releases-zsync|brainage04|TuneMesh|latest|TuneMesh-*-$arch.AppImage.zsync"
linuxdeploy --appdir "$appdir" --output appimage
ls -l "$out"
