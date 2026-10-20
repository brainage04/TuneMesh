#!/bin/sh
# Prints the Flatpak's THIRD-PARTY-SOURCES.md from its manifest: every module
# built into the app with its exact source (URL plus checksum or git commit).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
manifest=$here/io.github.brainage04.TuneMesh.yml
version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$here/../../linux/Cargo.toml")

"$here/../licenses/sources-header.sh" Flatpak "$version"
cat <<'EOF'

## Modules built into the Flatpak

These modules are built from the sources below, unmodified, with the options
in the manifest (`packaging/flatpak/io.github.brainage04.TuneMesh.yml` in the
source tarball). Their licence files are in
`share/licenses/io.github.brainage04.TuneMesh/<module>/`.

- libass: ISC.
- libplacebo: LGPL-2.1-or-later (built with its git submodules at that commit).
- mpv (libmpv only): built with mpv's default `gpl` option, so libmpv is
  under the GPL-2.0-or-later.
- ffmpeg-cli (`ffmpeg` and `ffprobe` only, statically linked): configured
  without `--enable-gpl` and `--enable-nonfree`, so both programs are under the
  LGPL-2.1-or-later.

GTK, libadwaita, GLib and the FFmpeg libraries libmpv plays through come from
the GNOME runtime (org.gnome.Platform) and its codecs extension, which Flathub
distributes separately with their own sources.
EOF

awk '
    /^  - name: / { module = $3; next }
    module == "" || module == "tunemesh" { next }
    /^ *(- type: |)url: / { sub(/^.*url: /, ""); printf "\n### %s\n\n- %s\n", module, $0; next }
    /^ *sha256: / { sub(/^.*sha256: /, ""); printf "  (SHA-256 %s)\n", $0; next }
    /^ *tag: / { sub(/^.*tag: /, ""); printf "  tag %s\n", $0; next }
    /^ *commit: / { sub(/^.*commit: /, ""); printf "  commit %s\n", $0; next }
' "$manifest"
