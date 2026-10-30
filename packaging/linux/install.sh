#!/bin/sh
# Installs (or uninstalls) the Spotless desktop app, the download worker and
# their data files in the standard Linux layout. The .deb, .rpm and Arch
# packages call it with --destdir; from a source checkout it is the
# "make install" equivalent:
#
#   cargo build --release --locked --manifest-path linux/Cargo.toml -p spotless -p spotless-worker
#   sudo packaging/linux/install.sh                # installs under /usr/local
#   sudo packaging/linux/install.sh --uninstall    # removes exactly those files
#
# Options:
#   --prefix <dir>    installation prefix (default /usr/local)
#   --destdir <dir>   staging root prepended to every path (packaging)
#   --bin-dir <dir>   where the built binaries are (default linux/target/release)
#   --uninstall       remove the files instead of installing them
set -eu

repo=$(cd "$(dirname "$0")/../.." && pwd)
prefix=/usr/local
destdir=
bin_dir=$repo/linux/target/release
uninstall=false

while [ $# -gt 0 ]; do
    case $1 in
        --prefix) prefix=$2; shift 2 ;;
        --destdir) destdir=$2; shift 2 ;;
        --bin-dir) bin_dir=$2; shift 2 ;;
        --uninstall) uninstall=true; shift ;;
        -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "install.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done

app_id=io.github.brainage04.Spotless
data=$repo/linux/data
bindir=$prefix/bin
datadir=$prefix/share
# systemd looks for user units in <prefix>/lib/systemd/user for /usr and
# /usr/local alike.
userunitdir=$prefix/lib/systemd/user

# Destination paths, one per line, in install order.
files="$bindir/spotless
$bindir/spotless-worker
$datadir/applications/$app_id.desktop
$datadir/icons/hicolor/scalable/apps/$app_id.svg
$datadir/metainfo/$app_id.metainfo.xml
$userunitdir/spotless-worker.service
$datadir/licenses/spotless/LICENSE
$datadir/licenses/spotless/THIRD-PARTY-RUST.txt"

refresh_caches() {
    # Only when installing into the live system; package managers run these
    # through their own triggers.
    [ -z "$destdir" ] || return 0
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database -q "$datadir/applications" 2>/dev/null || true
    fi
    if command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache -q -t "$datadir/icons/hicolor" 2>/dev/null || true
    fi
}

if $uninstall; then
    echo "$files" | while IFS= read -r f; do
        rm -f "$destdir$f"
    done
    rmdir "$destdir$datadir/licenses/spotless" 2>/dev/null || true
    refresh_caches
    exit 0
fi

for bin in spotless spotless-worker; do
    if [ ! -x "$bin_dir/$bin" ]; then
        echo "install.sh: $bin_dir/$bin is missing; build it first with" >&2
        echo "  cargo build --release --locked --manifest-path linux/Cargo.toml -p spotless -p spotless-worker" >&2
        exit 1
    fi
done

# systemd refuses executable paths containing quotes, backslashes or control
# characters however they are escaped, so such prefixes are rejected before
# anything is installed.
case $bindir in
    *'
'*) bad=true ;;
    *) if printf '%s' "$bindir" | LC_ALL=C grep -q "[\"'\\[:cntrl:]]"; then bad=true; else bad=false; fi ;;
esac
if $bad; then
    echo "install.sh: systemd cannot run programs from a path with quotes, backslashes or" >&2
    echo "  control characters: $bindir" >&2
    exit 1
fi

install -Dm755 "$bin_dir/spotless" "$destdir$bindir/spotless"
install -Dm755 "$bin_dir/spotless-worker" "$destdir$bindir/spotless-worker"
install -Dm644 "$data/$app_id.desktop" "$destdir$datadir/applications/$app_id.desktop"
install -Dm644 "$data/$app_id.svg" "$destdir$datadir/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 "$data/$app_id.metainfo.xml" "$destdir$datadir/metainfo/$app_id.metainfo.xml"
mkdir -p "$destdir$userunitdir"
# ExecStart takes the worker as one double-quoted word with % (systemd
# specifiers) doubled; systemd does not expand $ in the executable path. The
# template is filled in with awk's index/substr, so nothing in the prefix acts
# as a pattern or replacement metacharacter.
worker_exec=$(printf '%s/spotless-worker' "$bindir" | sed 's/%/%%/g')
SPOTLESS_WORKER_EXEC="\"$worker_exec\"" awk '
    BEGIN { token = "@bindir@/spotless-worker"; value = ENVIRON["SPOTLESS_WORKER_EXEC"] }
    {
        out = ""
        while ((i = index($0, token)) > 0) {
            out = out substr($0, 1, i - 1) value
            $0 = substr($0, i + length(token))
        }
        print out $0
    }
' "$repo/packaging/systemd/spotless-worker.service.in" >"$destdir$userunitdir/spotless-worker.service"
chmod 644 "$destdir$userunitdir/spotless-worker.service"
# Spotless's licence and those of the Rust crates linked into both binaries.
install -Dm644 "$repo/LICENSE" "$destdir$datadir/licenses/spotless/LICENSE"
install -Dm644 "$repo/packaging/licenses/THIRD-PARTY-RUST.txt" \
    "$destdir$datadir/licenses/spotless/THIRD-PARTY-RUST.txt"
refresh_caches
