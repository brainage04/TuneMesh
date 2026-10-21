#!/bin/sh
# Licence notices and source references for Ubuntu/Debian packages bundled
# into an AppImage or a snap.
#
#   debian-notices.sh <root> <sources.md> <package>...
#
# For each binary package: copies /usr/share/doc/<package>/copyright into
# <root>/usr/share/doc/<package>/ (unless the root has it already), then
# appends to <sources.md> one entry per source package with its version and
# the exact source files (URL, size, checksum) from `apt-get source
# --print-uris`. The packages must be installed (dpkg) or be .deb files.
# deb-src entries must be enabled for the checksums; without them the entry
# carries the version and a Launchpad link only.
set -eu
root=$1
out=$2
shift 2

list=$(mktemp)
trap 'rm -f "$list"' EXIT
for pkg in "$@"; do
    case $pkg in
        *.deb)
            name=$(dpkg-deb -f "$pkg" Package)
            version=$(dpkg-deb -f "$pkg" Version)
            source=$(dpkg-deb -f "$pkg" Source)
            ;;
        *)
            name=$pkg
            version=$(dpkg-query -W -f '${Version}' "$pkg")
            source=$(dpkg-query -W -f '${Source}' "$pkg")
            ;;
    esac
    # Source is "name" or "name (version)" and absent when it equals Package.
    src_name=${source%% *}
    src_name=${src_name:-$name}
    case $source in
        *"("*")") src_version=${source#*(}; src_version=${src_version%)} ;;
        *) src_version=$version ;;
    esac
    # Binary-only rebuilds append +bN, which the source version lacks.
    src_version=$(printf '%s' "$src_version" | sed 's/+b[0-9]*$//')

    doc=$root/usr/share/doc/$name
    if [ ! -f "$doc/copyright" ]; then
        if [ -f "/usr/share/doc/$name/copyright" ]; then
            install -Dm644 "/usr/share/doc/$name/copyright" "$doc/copyright"
        elif [ "${pkg%.deb}" != "$pkg" ]; then
            tmp=$(mktemp -d)
            dpkg-deb -x "$pkg" "$tmp"
            install -Dm644 "$tmp/usr/share/doc/$name/copyright" "$doc/copyright"
            rm -rf "$tmp"
        else
            echo "debian-notices: no copyright file for $name" >&2
            exit 1
        fi
    fi
    printf '%s %s %s\n' "$src_name" "$src_version" "$name" >>"$list"
done

sort -u "$list" | awk '
    { key = $1 " " $2; if (!(key in bins)) order[n++] = key; bins[key] = bins[key] (bins[key] ? ", " : "") $3 }
    END { for (i = 0; i < n; i++) print order[i] "\t" bins[order[i]] }
' | sort -u | while IFS='	' read -r key bins; do
    src=${key% *}
    ver=${key#* }
    {
        echo
        echo "### $src $ver"
        echo
        echo "Binary packages: $bins. Licence: \`usr/share/doc/<package>/copyright\`."
        echo "Launchpad: https://launchpad.net/ubuntu/+source/$src/$ver"
        echo
        # apt-get source writes nothing with --print-uris, but runs in /tmp anyway.
        uris=$(cd /tmp || exit; apt-get source -qq --print-uris --only-source "$src=$ver" 2>/dev/null |
            sed -n "s/^'\([^']*\)' [^ ]* \([0-9]*\) \([A-Z0-9]*\):\([0-9a-f]*\)$/- \1 (\2 bytes, \3 \4)/p" |
            # Name the main archive rather than whichever mirror the build used
            # (GitHub's runners use mirror+file:/etc/apt/apt-mirrors.txt).
            sed 's#^- [^ ]*/pool/#- http://archive.ubuntu.com/ubuntu/pool/#')
        if [ -n "$uris" ]; then
            echo "$uris"
        else
            echo "- (source file list unavailable when this was built; see Launchpad)"
        fi
    } >>"$out"
done
