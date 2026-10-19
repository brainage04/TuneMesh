#!/bin/sh
# Builds tunemesh_<version>-1_<arch>.deb and the generic binary tarball
# tunemesh-<version>-linux-<machine>.tar.gz (used by the AUR -bin package)
# from this checkout. Run it on the oldest supported base (Ubuntu 24.04) so
# the package also installs on everything newer:
#
#   podman run --rm -v "$PWD:/src" -w /src ubuntu:24.04 packaging/deb/build-deb.sh --deps dist
#
# --deps installs the build dependencies with apt first (needs root).
# TUNEMESH_VERSION overrides the version taken from linux/Cargo.toml.
set -eu
# shellcheck source=packaging/linux/common.sh
. "$(dirname "$0")/../linux/common.sh"

deps=false
if [ "${1:-}" = --deps ]; then deps=true; shift; fi
out=$(mkdir -p "${1:-dist}" && cd "${1:-dist}" && pwd)

if $deps; then
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -q
    apt-get install -yq --no-install-recommends \
        build-essential pkg-config ca-certificates curl dpkg-dev \
        libgtk-4-dev libadwaita-1-dev libmpv-dev libglib2.0-dev-bin
fi

ensure_rust
version=${TUNEMESH_VERSION:-$(tunemesh_version)}
arch=$(dpkg --print-architecture)
build_binaries

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
root=$stage/root
"$repo/packaging/linux/install.sh" --destdir "$root" --prefix /usr
strip --strip-unneeded "$root/usr/bin/tunemesh" "$root/usr/bin/tunemesh-worker"

# Generic tarball: the same files under usr/, for distributions that take
# prebuilt binaries (AUR tunemesh-bin).
tarball=$out/tunemesh-$version-linux-$(uname -m).tar.gz
tar -C "$root" --owner=0 --group=0 --numeric-owner -czf "$tarball" usr

# Debian keeps licences in /usr/share/doc/<package>/copyright, the one file
# there that minimised systems (Docker images, cloud images) still install:
# TuneMesh's licence followed by those of the Rust crates linked into the
# binaries.
doc=$root/usr/share/doc/tunemesh
mkdir -p "$doc"
{
    echo "TuneMesh"
    echo "Source: https://github.com/brainage04/TuneMesh"
    echo
    cat "$root/usr/share/licenses/tunemesh/LICENSE"
    echo
    echo
    cat "$root/usr/share/licenses/tunemesh/THIRD-PARTY-RUST.txt"
} >"$doc/copyright"
rm -r "$root/usr/share/licenses"

# Library dependencies with the minimum versions the binaries link against.
mkdir -p "$stage/shlibs/debian"
printf 'Source: tunemesh\n\nPackage: tunemesh\nArchitecture: any\n' >"$stage/shlibs/debian/control"
shlibs=$(cd "$stage/shlibs" && dpkg-shlibdeps -O \
    -e"$root/usr/bin/tunemesh" -e"$root/usr/bin/tunemesh-worker" |
    sed -n 's/^shlibs:Depends=//p')

mkdir -p "$root/DEBIAN"
cat >"$root/DEBIAN/control" <<EOF
Package: tunemesh
Version: $version-1
Architecture: $arch
Maintainer: brainage04 <brainage04@users.noreply.github.com>
Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)
Depends: $shlibs, libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), ffmpeg
Recommends: syncthing
Section: sound
Priority: optional
Homepage: https://github.com/brainage04/TuneMesh
Description: music player for a Syncthing-shared library
 TuneMesh plays a music library that lives in one Syncthing folder shared
 between your devices, and downloads new songs into it with yt-dlp
 (tunemesh-worker, also available as the user service tunemesh-worker).
EOF

deb=$out/tunemesh_${version}-1_$arch.deb
dpkg-deb --root-owner-group -Zxz --build "$root" "$deb"
echo "built $deb and $tarball"
