#!/bin/sh
# Prints the official cloud image of a distribution and its checksum:
#
#   packaging/vm/images.sh <distro>
#   => <url> <sha256|sha512> <checksum>
#
# The checksum comes from the distribution's own checksum file (over HTTPS),
# fetched fresh each time, so a moving "latest" image gets a new checksum
# and with it a new cache entry. Distros: ubuntu-24.04 debian-13 fedora
# opensuse-tumbleweed arch ("fedora" is the newest stable release).
set -eu

fetch() { curl -fsSL --retry 3 --retry-delay 5 --max-time 60 "$1"; }

# $1 = checksum file URL, $2 = image file name: prints the checksum.
sum_for() {
    fetch "$1" | awk -v f="$2" '{ n = $2; sub(/^\*/, "", n) } n == f { print $1; found = 1 } END { exit !found }' ||
        { echo "images.sh: no checksum for $2 in $1" >&2; exit 1; }
}

case ${1:-} in
ubuntu-24.04)
    base=https://cloud-images.ubuntu.com/releases/noble/release
    file=ubuntu-24.04-server-cloudimg-amd64.img
    sum=$(sum_for "$base/SHA256SUMS" "$file")
    echo "$base/$file sha256 $sum"
    ;;
debian-13)
    base=https://cloud.debian.org/images/cloud/trixie/latest
    file=debian-13-generic-amd64.qcow2
    sum=$(sum_for "$base/SHA512SUMS" "$file")
    echo "$base/$file sha512 $sum"
    ;;
fedora)
    # releases.json lists every current image with its SHA-256; take the
    # highest stable (non-Beta) Cloud Base Generic image.
    fetch https://fedoraproject.org/releases.json | python3 -c '
import json, sys
images = [
    r for r in json.load(sys.stdin)
    if r["arch"] == "x86_64" and r["variant"] == "Cloud" and r["subvariant"] == "Cloud_Base"
    and r["version"].isdigit() and r["link"].endswith(".qcow2")
]
if not images:
    sys.exit("images.sh: no Fedora Cloud Base image in releases.json")
best = max(images, key=lambda r: int(r["version"]))
print(best["link"], "sha256", best["sha256"])
'
    ;;
opensuse-tumbleweed)
    base=https://download.opensuse.org/tumbleweed/appliances
    file=openSUSE-Tumbleweed-Minimal-VM.x86_64-Cloud.qcow2
    sum=$(sum_for "$base/$file.sha256" "$file")
    echo "$base/$file sha256 $sum"
    ;;
arch)
    base=https://geo.mirror.pkgbuild.com/images/latest
    file=Arch-Linux-x86_64-cloudimg.qcow2
    sum=$(sum_for "$base/$file.SHA256" "$file")
    echo "$base/$file sha256 $sum"
    ;;
*)
    echo "usage: $0 ubuntu-24.04|debian-13|fedora|opensuse-tumbleweed|arch" >&2
    exit 2
    ;;
esac
