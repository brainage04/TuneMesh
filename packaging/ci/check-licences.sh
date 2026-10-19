#!/bin/sh
# Checks that an installed or unpacked package carries its licence notices:
# TuneMesh's licence, the Rust crate licences and, for formats that bundle
# third-party software, those components' licences and THIRD-PARTY-SOURCES.md.
#
#   check-licences.sh <format> <root>
#
# format: deb, rpm, arch, appimage, flatpak or snap. root: the file-system
# root the package's /usr (or Flatpak's files/) is under, e.g. / or an
# extracted AppImage.
set -eu
format=$1
root=${2%/}
missing=0

need() {
    for f in "$@"; do
        if [ -s "$root/$f" ]; then
            echo "ok      $f"
        else
            echo "MISSING $f" >&2
            missing=1
        fi
    done
}

# $1 = file, $2 = extended regex it must contain.
contains() {
    if grep -Eq "$2" "$root/$1" 2>/dev/null; then
        echo "ok      $1 mentions $2"
    else
        echo "MISSING $1 does not mention $2" >&2
        missing=1
    fi
}

# Every usr/share/doc/<package>/ of a bundling format must hold its copyright.
all_debian_copyrights() {
    for dir in "$root"/usr/share/doc/*/; do
        [ -s "$dir/copyright" ] || { echo "MISSING ${dir#"$root"/}copyright" >&2; missing=1; }
    done
    echo "checked $(find "$root/usr/share/doc" -mindepth 1 -maxdepth 1 | wc -l) bundled packages' copyright files"
}

case $format in
    deb)
        need usr/share/doc/tunemesh/copyright usr/share/doc/tunemesh/THIRD-PARTY-RUST.txt
        rust=usr/share/doc/tunemesh/THIRD-PARTY-RUST.txt
        ;;
    rpm | arch)
        need usr/share/licenses/tunemesh/LICENSE usr/share/licenses/tunemesh/THIRD-PARTY-RUST.txt
        rust=usr/share/licenses/tunemesh/THIRD-PARTY-RUST.txt
        ;;
    appimage | snap)
        lic=usr/share/licenses/tunemesh
        need $lic/LICENSE $lic/THIRD-PARTY-RUST.txt $lic/THIRD-PARTY-SOURCES.md \
            usr/share/doc/libmpv2/copyright usr/share/doc/ffmpeg/copyright
        if [ "$format" = appimage ]; then
            need usr/share/doc/libgtk-4-1/copyright usr/share/doc/libadwaita-1-0/copyright
        fi
        all_debian_copyrights
        contains $lic/THIRD-PARTY-SOURCES.md '^### mpv '
        contains $lic/THIRD-PARTY-SOURCES.md '^### ffmpeg '
        contains $lic/THIRD-PARTY-SOURCES.md 'pool/[^ ]*/ffmpeg_[^ ]*\.orig\.tar'
        rust=$lic/THIRD-PARTY-RUST.txt
        ;;
    flatpak)
        lic=share/licenses/io.github.brainage04.TuneMesh
        need $lic/LICENSE $lic/THIRD-PARTY-RUST.txt $lic/THIRD-PARTY-SOURCES.md \
            share/licenses/libass/COPYING share/licenses/libplacebo/LICENSE \
            share/licenses/mpv/LICENSE.GPL share/licenses/mpv/Copyright \
            share/licenses/ffmpeg/LICENSE.md share/licenses/ffmpeg/COPYING.LGPLv2.1
        for module in libass libplacebo mpv ffmpeg-cli; do
            contains $lic/THIRD-PARTY-SOURCES.md "^### $module\$"
        done
        rust=$lic/THIRD-PARTY-RUST.txt
        ;;
    *)
        echo "check-licences: unknown format $format" >&2
        exit 2
        ;;
esac
contains "$rust" '^  mpris-server '

if [ $missing -ne 0 ]; then
    echo "check-licences: licence notices missing from the $format package" >&2
    exit 1
fi
echo "check-licences: $format package carries its licence notices"
