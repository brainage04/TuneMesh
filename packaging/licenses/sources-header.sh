#!/bin/sh
# Prints the common start of a THIRD-PARTY-SOURCES.md:
#
#   sources-header.sh <artifact> <version>
#
# The build scripts append the artifact's bundled components after it.
set -eu
artifact=$1
version=$2
cat <<EOF
# Third-party components and their sources: $artifact

TuneMesh $version is MIT licensed. This $artifact also contains third-party
software under its own licences. Their licence texts are inside the
$artifact under \`share/licenses/\` and \`share/doc/<package>/copyright\`.

For every component under the GPL or LGPL, the exact corresponding source is
listed below (URLs, versions and checksums). The same files are also
available on request: open an issue at
https://github.com/brainage04/TuneMesh/issues.

## TuneMesh and its Rust crates

- TuneMesh: https://github.com/brainage04/TuneMesh/releases/tag/v$version
  (source tarball \`tunemesh-$version.tar.gz\` on that page). It also contains
  the build recipes for this $artifact under \`packaging/\`.
- Rust crates statically linked into \`tunemesh\` and \`tunemesh-worker\`:
  listed with their licences in \`share/licenses/tunemesh/THIRD-PARTY-RUST.txt\`.
  Their exact versions and SHA-256 checksums are in \`linux/Cargo.lock\` in the
  source tarball; each crate's source is at
  \`https://crates.io/api/v1/crates/<name>/<version>/download\`. The one crate
  under the MPL-2.0, mpris-server, is unmodified.
EOF
