#!/bin/sh
# Release APK for GitHub releases, in two steps that run in separate CI jobs,
# so that no Gradle, plugin or app build code ever runs where the signing key
# is (the signing job runs only this script, apksigner and a JDK):
#
#   VERSION=<v> packaging/ci/android-release.sh build
#       Builds the unsigned release APK (no signing secrets needed or read)
#       into dist/TuneMesh-<v>-unsigned.apk, with dist/TuneMesh-<v>-android-NOTICE.txt.
#
#   VERSION=<v> ANDROID_KEYSTORE_B64=... ANDROID_STORE_PASSWORD=... \
#   ANDROID_KEY_ALIAS=... ANDROID_KEY_PASSWORD=... [ANDROID_LINEAGE_B64=...] \
#       packaging/ci/android-release.sh sign
#       Signs that APK with apksigner into dist/TuneMesh-<v>.apk and removes
#       the unsigned one. Fails when a required secret is missing. apksigner
#       comes from Android build-tools downloaded from Google and checked
#       against the SHA-256 pinned below (APKSIGNER=<path> uses another one,
#       e.g. a local SDK's). Needs java on PATH. The
#       signature matches what android/app/build.gradle.kts produces with the
#       same key: APK Signature Scheme v2 only without a lineage; with one,
#       v3 only, carrying the rotation lineage (rotation from Android 9,
#       like RotateApkSignature; minSdk is 30, so v2 is not needed).
set -eu
repo=$(cd "$(dirname "$0")/../.." && pwd)
dist=$repo/dist
[ "${1:-}" = build-tools ] || : "${VERSION:?VERSION is not set}"
unsigned=$dist/TuneMesh-$VERSION-unsigned.apk

build() {
    (cd "$repo/android" && ./gradlew --no-daemon :app:assembleRelease)
    mkdir -p "$dist"
    cp "$repo/android/app/build/outputs/apk/release/app-release-unsigned.apk" "$unsigned"
    # Licence notices of the libraries in the APK (also its assets/NOTICE.txt).
    cp "$repo/android/app/build/generated/notice/NOTICE.txt" "$dist/TuneMesh-$VERSION-android-NOTICE.txt"
    unzip -l "$unsigned" | grep -q ' assets/NOTICE.txt$'
    check_version "$unsigned"
}

# The APK must carry the release version: versionName is VERSION (the
# linux/Cargo.toml version) and versionCode is X*10000 + Y*100 + Z, as
# android/app/build.gradle.kts derives them.
check_version() {
    sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:?ANDROID_HOME is not set}}
    aapt2=$(find -L "$sdk/build-tools" -mindepth 2 -maxdepth 2 -name aapt2 | sort -V | tail -n 1)
    [ -x "$aapt2" ] || { echo "android-release: no aapt2 in $sdk/build-tools" >&2; exit 1; }
    badging=$("$aapt2" dump badging "$1" | sed -n 1p)
    code=$(echo "$VERSION" | awk -F. 'NF == 3 && $2 < 100 && $3 < 100 { print $1 * 10000 + $2 * 100 + $3 }')
    case $badging in
        *" versionCode='$code' versionName='$VERSION' "*) echo "APK version $VERSION ($code)" ;;
        *)
            echo "android-release: the APK's version does not match $VERSION (versionCode $code):" >&2
            echo "  $badging" >&2
            exit 1 ;;
    esac
}

# Android build-tools 36.0.0 for Linux, as listed in Google's SDK repository
# (https://dl.google.com/android/repository/repository2-3.xml gives SHA-1
# b0b6376977657e8ad9b969bacf4093601da2c6fb for the same file).
build_tools_url=https://dl.google.com/android/repository/build-tools_r36_linux.zip
build_tools_sha256=5d9ac77fb6ff43d9da518a337b4fcf8f9097113df531d99ccefe80ef7ce8250b

# Downloads and unpacks the pinned build-tools into $1 (apksigner at $1/apksigner).
fetch_build_tools() {
    mkdir -p "$1"
    curl -fsSL --retry 3 -o "$1.zip" "$build_tools_url"
    echo "$build_tools_sha256  $1.zip" | sha256sum -c --quiet -
    unzip -q "$1.zip" -d "$1.unpacked"
    mv "$1.unpacked"/*/* "$1/"
    rm -rf "$1.zip" "$1.unpacked"
    [ -x "$1/apksigner" ] || { echo "android-release: no apksigner in the build-tools archive" >&2; exit 1; }
}

sign() {
    missing=
    for var in ANDROID_KEYSTORE_B64 ANDROID_STORE_PASSWORD ANDROID_KEY_ALIAS ANDROID_KEY_PASSWORD; do
        eval "value=\${$var:-}"
        [ -n "$value" ] || missing="$missing $var"
    done
    if [ -n "$missing" ]; then
        prefix=
        [ "${GITHUB_ACTIONS:-}" = true ] && prefix=::error::
        echo "${prefix}Android signing secrets missing:$missing (environment 'release')" >&2
        exit 1
    fi
    [ -f "$unsigned" ] || { echo "android-release: $unsigned is missing; run build first" >&2; exit 1; }

    keys=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/tunemesh-signing.XXXXXX")
    trap 'rm -rf "$keys"' EXIT
    apksigner=${APKSIGNER:-}
    if [ -z "$apksigner" ]; then
        fetch_build_tools "$keys/build-tools"
        apksigner=$keys/build-tools/apksigner
    fi
    (
        umask 077
        printf '%s' "$ANDROID_KEYSTORE_B64" | base64 -d >"$keys/release.jks"
        if [ -n "${ANDROID_LINEAGE_B64:-}" ]; then
            printf '%s' "$ANDROID_LINEAGE_B64" | base64 -d >"$keys/lineage"
        fi
    )
    signed=$dist/TuneMesh-$VERSION.apk
    set -- --ks "$keys/release.jks" --ks-key-alias "$ANDROID_KEY_ALIAS" \
        --ks-pass env:ANDROID_STORE_PASSWORD --key-pass env:ANDROID_KEY_PASSWORD \
        --v1-signing-enabled false --v4-signing-enabled false
    if [ -f "$keys/lineage" ]; then
        set -- "$@" --lineage "$keys/lineage" --rotation-min-sdk-version 28 \
            --v2-signing-enabled false --v3-signing-enabled true
    else
        set -- "$@" --v2-signing-enabled true --v3-signing-enabled false
    fi
    "$apksigner" sign "$@" --out "$signed" "$unsigned"
    "$apksigner" verify --print-certs -v "$signed"
    rm "$unsigned"
}

case ${1:-} in
    build) build ;;
    sign) sign ;;
    # build-tools <dir>: only download the pinned build-tools (for tests).
    build-tools) fetch_build_tools "${2:?usage: android-release.sh build-tools <dir>}" ;;
    *) echo "usage: android-release.sh build|sign|build-tools <dir>" >&2; exit 2 ;;
esac
