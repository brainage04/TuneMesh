#!/bin/sh
# Builds the source tarball, the cargo-vendor tarball and the binary RPM from
# this checkout, e.g. in a Fedora container:
#
#   podman run --rm -v "$PWD:/src" -w /src fedora:latest packaging/rpm/build-rpm.sh --deps dist
#
# --deps installs the build dependencies with dnf first (needs root).
# TUNEMESH_VERSION overrides the version taken from linux/Cargo.toml.
set -eu
# shellcheck source=packaging/linux/common.sh
. "$(dirname "$0")/../linux/common.sh"

deps=false
if [ "${1:-}" = --deps ]; then deps=true; shift; fi
out=$(mkdir -p "${1:-dist}" && cd "${1:-dist}" && pwd)

if $deps; then
    dnf install -y --setopt=install_weak_deps=False \
        rpm-build cargo rust gcc tar xz \
        'pkgconfig(gtk4)' 'pkgconfig(libadwaita-1)' 'pkgconfig(mpv)' /usr/bin/glib-compile-resources
fi

version=${TUNEMESH_VERSION:-$(tunemesh_version)}
top=$(mktemp -d)
trap 'rm -rf "$top"' EXIT
mkdir -p "$top/SOURCES" "$top/SPECS"

make_source_tarball "$top/SOURCES/tunemesh-$version.tar.gz" "$version"

# Vendored crates, so the spec builds without network access (mock, COPR, OBS).
vendor=$(mktemp -d)
(cd "$vendor" && cargo vendor --locked --manifest-path "$repo/linux/Cargo.toml" vendor >/dev/null)
tar -C "$vendor" -cJf "$top/SOURCES/tunemesh-$version-vendor.tar.xz" vendor
rm -rf "$vendor"

sed "s/^Version:.*/Version:        $version/" "$repo/packaging/rpm/tunemesh.spec" >"$top/SPECS/tunemesh.spec"
rpmbuild --define "_topdir $top" -bb "$top/SPECS/tunemesh.spec"
find "$top/RPMS" -name '*.rpm' -exec cp {} "$out/" \;
cp "$top/SOURCES/tunemesh-$version.tar.gz" "$top/SOURCES/tunemesh-$version-vendor.tar.xz" "$out/"
ls "$out"
