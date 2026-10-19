# Shared helpers for the package build scripts (sourced, not executed).

repo=$(cd "$(dirname "$0")/../.." && pwd)

# The workspace version, e.g. 0.1.0.
tunemesh_version() {
    sed -n '/^\[workspace.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$repo/linux/Cargo.toml"
}

# Makes a Rust toolchain new enough for edition 2024 available, installing
# rustup's minimal stable toolchain when the system one is missing or older.
ensure_rust() {
    if command -v cargo >/dev/null 2>&1; then
        minor=$(cargo --version | sed -n 's/^cargo 1\.\([0-9]*\).*/\1/p')
        [ "${minor:-0}" -ge 85 ] && return 0
    fi
    if [ ! -x "$HOME/.cargo/bin/cargo" ]; then
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --default-toolchain stable
    fi
    PATH=$HOME/.cargo/bin:$PATH
    export PATH
}

# Builds both binaries into linux/target/release.
build_binaries() {
    cargo build --release --locked --manifest-path "$repo/linux/Cargo.toml" \
        -p tunemesh -p tunemesh-worker
}

# Writes the source tarball $1 (top directory tunemesh-$2): this checkout
# without git data, build output or the Android app. The release publishes it;
# the RPM spec and the AUR PKGBUILD build from it.
make_source_tarball() {
    tar -C "$repo" --transform "s|^\.|tunemesh-$2|" \
        --exclude=./.git --exclude=./linux/target --exclude=./android --exclude=./dist \
        --exclude='./result*' --exclude=./snap --exclude=./packaging/flatpak/cargo-sources.json \
        --sort=name --owner=0 --group=0 --numeric-owner -czf "$1" .
}
