#!/bin/sh
# Writes packaging/flatpak/cargo-sources.json (the crates from linux/Cargo.lock
# as offline Flatpak sources) with flatpak-builder-tools' cargo generator.
# Needs python3 with aiohttp and tomlkit (pip install aiohttp tomlkit).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
generator=$(mktemp)
trap 'rm -f "$generator"' EXIT
curl -fsSL -o "$generator" \
    https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 "$generator" "$here/../../linux/Cargo.lock" -o "$here/cargo-sources.json"
