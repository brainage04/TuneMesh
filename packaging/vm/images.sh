#!/bin/sh
# Prints the cloud image of a distribution and its checksum:
#
#   packaging/vm/images.sh <distro>
#   => <url> <sha256|sha512> <checksum>
#
# Distros: ubuntu-24.04 ubuntu-26.04 debian-13 fedora arch.
#
# Release distributions are pinned here, image URL and checksum both, so a
# compromised or wrong download server cannot hand the tests another image.
# To move a pin, take the dated image's checksum from the distribution's
# checksum file and check that file's signature where there is one:
#   ubuntu-24.04  releases/noble/release-<date>/SHA256SUMS, signed (SHA256SUMS.gpg)
#                 by the UEC Image Automatic Signing Key D2EB44626FDDC30B513D5BB71A5D6C4C7DB87C81
#   ubuntu-26.04  releases/resolute/release-<date>/SHA256SUMS, signed by the same key
#   debian-13     images/cloud/trixie/<date>-<n>/SHA512SUMS (Debian does not sign it)
#   fedora        releases/<n>/Cloud/x86_64/images/Fedora-Cloud-<n>-<r>-x86_64-CHECKSUM,
#                 clearsigned with the release key from https://fedoraproject.org/fedora.gpg
#
# Arch is rolling and keeps old images only for a few months, so it is
# resolved to the current snapshot, and its checksum file must carry a good
# signature from the key pinned in packaging/vm/keys/ (checked with gpgv;
# needs gpg and gpgv): the arch-boxes release key
# 1B9A16984A4E8CB448712D2AE0B78BF4326C6F8F (as printed in the arch-boxes README).
set -eu
here=$(cd "$(dirname "$0")" && pwd)

fetch() { curl -fsSL --retry 3 --retry-delay 5 --max-time 60 "$1"; }

# $1 = key file in packaging/vm/keys, $2 = checksum file URL, $3 = its
# signature URL, $4 = image file name: prints the image's checksum once the
# checksum file's signature verifies.
signed_sum() {
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    gpg --batch --quiet --dearmor <"$here/keys/$1" >"$tmp/keyring.gpg"
    fetch "$2" >"$tmp/sums"
    fetch "$3" >"$tmp/sums.sig"
    if ! gpgv --keyring "$tmp/keyring.gpg" "$tmp/sums.sig" "$tmp/sums" 2>"$tmp/gpgv.log"; then
        cat "$tmp/gpgv.log" >&2
        echo "images.sh: $2 has no good signature from packaging/vm/keys/$1" >&2
        exit 1
    fi
    awk -v f="$4" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; found = 1 } END { exit !found }' "$tmp/sums" ||
        { echo "images.sh: no checksum for $4 in $2" >&2; exit 1; }
}

# $1 = directory listing URL, $2 = extended regex: the newest matching name.
newest() {
    fetch "$1" | grep -oE "$2" | sort -uV | tail -n 1 | grep . ||
        { echo "images.sh: nothing matching $2 in $1" >&2; exit 1; }
}

case ${1:-} in
ubuntu-24.04)
    echo "https://cloud-images.ubuntu.com/releases/noble/release-20261017/ubuntu-24.04-server-cloudimg-amd64.img" \
        sha256 04e8f2461a16a44e78a370284471f29616cdd9baf1893df25807b71f985b6c94
    ;;
ubuntu-26.04)
    echo "https://cloud-images.ubuntu.com/releases/resolute/release-20261016/ubuntu-26.04-server-cloudimg-amd64.img" \
        sha256 5645d0318ad21f643ebb1ac4567514620e9893380b07b0201fda163baa5b7eb5
    ;;
debian-13)
    echo "https://cloud.debian.org/images/cloud/trixie/20261017-2634/debian-13-generic-amd64-20261017-2634.qcow2" \
        sha512 62aa92aeff32cbf3fe6c01f410fef4035b1603a512f073c9f8889072168de9e49fa361ba48621359fc7608ad91225e823ea233feb33f612eb39e50aaf21957fe
    ;;
fedora)
    echo "https://download.fedoraproject.org/pub/fedora/linux/releases/44/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44-1.7.x86_64.qcow2" \
        sha256 28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f
    ;;
arch)
    version=$(newest https://geo.mirror.pkgbuild.com/images/latest/ 'Arch-Linux-x86_64-cloudimg-[0-9]+\.[0-9]+\.qcow2' |
        sed 's/^Arch-Linux-x86_64-cloudimg-//; s/\.qcow2$//')
    base=https://geo.mirror.pkgbuild.com/images/v$version
    file=Arch-Linux-x86_64-cloudimg-$version.qcow2
    sum=$(signed_sum arch-boxes.asc "$base/$file.SHA256" "$base/$file.SHA256.sig" "$file")
    echo "$base/$file sha256 $sum"
    ;;
*)
    echo "usage: $0 ubuntu-24.04|ubuntu-26.04|debian-13|fedora|arch" >&2
    exit 2
    ;;
esac
