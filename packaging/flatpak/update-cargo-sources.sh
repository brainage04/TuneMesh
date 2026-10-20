#!/bin/sh
# Writes packaging/flatpak/cargo-sources.json (the crates from linux/Cargo.lock
# as offline Flatpak sources) with flatpak-builder-tools' cargo generator.
# Needs python3 with aiohttp and tomlkit (CI: pip install aiohttp==3.14.5 tomlkit==0.15.2).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
generator=$(mktemp)
trap 'rm -f "$generator"' EXIT
# Pinned generator, checked against its SHA-256. To update: pick a newer
# commit and replace the hash with the output of `curl -fsSL <url> | sha256sum`.
commit=41c20aa10819cdb2a4f3ca171758a96d1955c018
sha=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380
curl -fsSL --retry 3 -o "$generator" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/$commit/cargo/flatpak-cargo-generator.py"
echo "$sha  $generator" | sha256sum -c --quiet -
python3 "$generator" "$here/../../linux/Cargo.lock" -o "$here/cargo-sources.json"
