#!/bin/sh
# Regenerates THIRD-PARTY-RUST.txt, the licences of every Rust crate linked
# into tunemesh and tunemesh-worker, from linux/Cargo.lock. Run it after
# changing dependencies (CI fails while the file is out of date):
#
#   nix shell --inputs-from . nixpkgs#cargo-about nixpkgs#cargo -c packaging/licenses/update-rust-licenses.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
# Fetch the crates first; cargo-about then reads their licence files offline,
# so the result does not depend on clearlydefined.io.
cargo fetch --locked --manifest-path "$here/../../linux/Cargo.toml" >/dev/null
cargo-about generate --locked --offline \
    --manifest-path "$here/../../linux/Cargo.toml" \
    --config "$here/about.toml" \
    -o "$here/THIRD-PARTY-RUST.txt" "$here/about.hbs"
