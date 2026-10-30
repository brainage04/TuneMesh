#!/bin/sh
# Builds the AUR packages from this checkout in an Arch Linux container (as
# root; makepkg runs as an unprivileged "builder" user), and writes ready-to-
# publish PKGBUILD/.SRCINFO pairs with real checksums:
#
#   podman run --rm -v "$PWD:/src" -w /src archlinux packaging/arch/build-arch.sh dist [bin-tarball]
#
# spotless builds from the release's source tarball spotless-<version>.tar.gz:
# SPOTLESS_SOURCE_TARBALL names it (the release workflow passes the one it
# publishes, so the checksum matches); without it, one is made from this
# checkout. With bin-tarball (spotless-<version>-linux-<machine>.tar.gz from
# packaging/deb/build-deb.sh) it also builds spotless-bin from it.
set -eu
# shellcheck source=packaging/linux/common.sh
. "$(dirname "$0")/../linux/common.sh"

out=$(mkdir -p "${1:-dist}" && cd "${1:-dist}" && pwd)
bin_tarball=${2:+$(cd "$(dirname "$2")" && pwd)/$(basename "$2")}
version=${SPOTLESS_VERSION:-$(spotless_version)}

pacman -Syu --noconfirm --needed base-devel cargo gtk4 libadwaita mpv ffmpeg hicolor-icon-theme
id builder >/dev/null 2>&1 || useradd -m builder
work=$(mktemp -d)
chown builder "$work"

# spotless: source build.
mkdir -p "$work/spotless"
if [ -n "${SPOTLESS_SOURCE_TARBALL:-}" ]; then
    cp "$SPOTLESS_SOURCE_TARBALL" "$work/spotless/spotless-$version.tar.gz"
else
    make_source_tarball "$work/spotless/spotless-$version.tar.gz" "$version"
fi
sha=$(sha256sum "$work/spotless/spotless-$version.tar.gz" | cut -d' ' -f1)
sed -e "s/^pkgver=.*/pkgver=$version/" -e "s/^sha256sums=.*/sha256sums=('$sha')/" \
    "$repo/packaging/arch/spotless/PKGBUILD" >"$work/spotless/PKGBUILD"
chown -R builder "$work/spotless"
# The archive is already in place, so makepkg does not download it.
su builder -c "cd '$work/spotless' && makepkg --noconfirm --cleanbuild && makepkg --printsrcinfo > .SRCINFO"
cp "$work"/spotless/spotless-*.pkg.tar.zst "$out/"
mkdir -p "$out/aur/spotless"
cp "$work/spotless/PKGBUILD" "$work/spotless/.SRCINFO" "$out/aur/spotless/"

if [ -n "$bin_tarball" ]; then
    machine=$(uname -m)
    mkdir -p "$work/spotless-bin"
    cp "$bin_tarball" "$work/spotless-bin/spotless-$version-linux-$machine.tar.gz"
    sed "s/^pkgver=.*/pkgver=$version/" "$repo/packaging/arch/spotless-bin/PKGBUILD" >"$work/spotless-bin/PKGBUILD"
    # Real checksums for every architecture whose tarball sits next to this one.
    for m in x86_64 aarch64; do
        f=$(dirname "$bin_tarball")/spotless-$version-linux-$m.tar.gz
        if [ -f "$f" ]; then
            sum=$(sha256sum "$f" | cut -d' ' -f1)
            sed -i "s/^sha256sums_$m=.*/sha256sums_$m=('$sum')/" "$work/spotless-bin/PKGBUILD"
        fi
    done
    chown -R builder "$work/spotless-bin"
    su builder -c "cd '$work/spotless-bin' && makepkg --noconfirm --cleanbuild && makepkg --printsrcinfo > .SRCINFO"
    cp "$work"/spotless-bin/spotless-bin-*.pkg.tar.zst "$out/"
    mkdir -p "$out/aur/spotless-bin"
    cp "$work/spotless-bin/PKGBUILD" "$work/spotless-bin/.SRCINFO" "$out/aur/spotless-bin/"
fi
rm -rf "$work"
ls -R "$out"
