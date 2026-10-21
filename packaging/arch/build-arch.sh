#!/bin/sh
# Builds the AUR packages from this checkout in an Arch Linux container (as
# root; makepkg runs as an unprivileged "builder" user), and writes ready-to-
# publish PKGBUILD/.SRCINFO pairs with real checksums:
#
#   podman run --rm -v "$PWD:/src" -w /src archlinux packaging/arch/build-arch.sh dist [bin-tarball]
#
# tunemesh builds from the release's source tarball tunemesh-<version>.tar.gz:
# TUNEMESH_SOURCE_TARBALL names it (the release workflow passes the one it
# publishes, so the checksum matches); without it, one is made from this
# checkout. With bin-tarball (tunemesh-<version>-linux-<machine>.tar.gz from
# packaging/deb/build-deb.sh) it also builds tunemesh-bin from it.
set -eu
# shellcheck source=packaging/linux/common.sh
. "$(dirname "$0")/../linux/common.sh"

out=$(mkdir -p "${1:-dist}" && cd "${1:-dist}" && pwd)
bin_tarball=${2:+$(cd "$(dirname "$2")" && pwd)/$(basename "$2")}
version=${TUNEMESH_VERSION:-$(tunemesh_version)}

pacman -Syu --noconfirm --needed base-devel cargo gtk4 libadwaita mpv ffmpeg hicolor-icon-theme
id builder >/dev/null 2>&1 || useradd -m builder
work=$(mktemp -d)
chown builder "$work"

# tunemesh: source build.
mkdir -p "$work/tunemesh"
if [ -n "${TUNEMESH_SOURCE_TARBALL:-}" ]; then
    cp "$TUNEMESH_SOURCE_TARBALL" "$work/tunemesh/tunemesh-$version.tar.gz"
else
    make_source_tarball "$work/tunemesh/tunemesh-$version.tar.gz" "$version"
fi
sha=$(sha256sum "$work/tunemesh/tunemesh-$version.tar.gz" | cut -d' ' -f1)
sed -e "s/^pkgver=.*/pkgver=$version/" -e "s/^sha256sums=.*/sha256sums=('$sha')/" \
    "$repo/packaging/arch/tunemesh/PKGBUILD" >"$work/tunemesh/PKGBUILD"
chown -R builder "$work/tunemesh"
# The archive is already in place, so makepkg does not download it.
su builder -c "cd '$work/tunemesh' && makepkg --noconfirm --cleanbuild && makepkg --printsrcinfo > .SRCINFO"
cp "$work"/tunemesh/tunemesh-*.pkg.tar.zst "$out/"
mkdir -p "$out/aur/tunemesh"
cp "$work/tunemesh/PKGBUILD" "$work/tunemesh/.SRCINFO" "$out/aur/tunemesh/"

if [ -n "$bin_tarball" ]; then
    machine=$(uname -m)
    mkdir -p "$work/tunemesh-bin"
    cp "$bin_tarball" "$work/tunemesh-bin/tunemesh-$version-linux-$machine.tar.gz"
    sed "s/^pkgver=.*/pkgver=$version/" "$repo/packaging/arch/tunemesh-bin/PKGBUILD" >"$work/tunemesh-bin/PKGBUILD"
    # Real checksums for every architecture whose tarball sits next to this one.
    for m in x86_64 aarch64; do
        f=$(dirname "$bin_tarball")/tunemesh-$version-linux-$m.tar.gz
        if [ -f "$f" ]; then
            sum=$(sha256sum "$f" | cut -d' ' -f1)
            sed -i "s/^sha256sums_$m=.*/sha256sums_$m=('$sum')/" "$work/tunemesh-bin/PKGBUILD"
        fi
    done
    chown -R builder "$work/tunemesh-bin"
    su builder -c "cd '$work/tunemesh-bin' && makepkg --noconfirm --cleanbuild && makepkg --printsrcinfo > .SRCINFO"
    cp "$work"/tunemesh-bin/tunemesh-bin-*.pkg.tar.zst "$out/"
    mkdir -p "$out/aur/tunemesh-bin"
    cp "$work/tunemesh-bin/PKGBUILD" "$work/tunemesh-bin/.SRCINFO" "$out/aur/tunemesh-bin/"
fi
rm -rf "$work"
ls -R "$out"
