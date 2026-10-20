#!/bin/sh
# Builds the release APK into dist/ with its licence notice. Signs it when
# ANDROID_KEYSTORE_B64, ANDROID_STORE_PASSWORD, ANDROID_KEY_ALIAS and
# ANDROID_KEY_PASSWORD are set (plus optional ANDROID_LINEAGE_B64), writing
# android/.signing/ in the format android/app/build.gradle.kts reads and
# deleting it afterwards; otherwise the APK is unsigned.
#
#   VERSION=<version> packaging/ci/android-release.sh
set -eu
repo=$(cd "$(dirname "$0")/../.." && pwd)
signing=$repo/android/.signing
trap 'rm -rf "$signing"' EXIT

if [ -n "${ANDROID_KEYSTORE_B64:-}" ] && [ -n "${ANDROID_STORE_PASSWORD:-}" ] &&
    [ -n "${ANDROID_KEY_ALIAS:-}" ] && [ -n "${ANDROID_KEY_PASSWORD:-}" ]; then
    signed=true
    (
        umask 077
        mkdir -p "$signing"
        printf '%s' "$ANDROID_KEYSTORE_B64" | base64 -d >"$signing/tunemesh.jks"
        {
            echo "storeFile=tunemesh.jks"
            echo "storePassword=$ANDROID_STORE_PASSWORD"
            echo "keyAlias=$ANDROID_KEY_ALIAS"
            echo "keyPassword=$ANDROID_KEY_PASSWORD"
        } >"$signing/signing.properties"
        if [ -n "${ANDROID_LINEAGE_B64:-}" ]; then
            printf '%s' "$ANDROID_LINEAGE_B64" | base64 -d >"$signing/lineage"
            echo "lineageFile=lineage" >>"$signing/signing.properties"
        fi
    )
else
    signed=false
    echo "::notice::Android signing secrets are not available; building an unsigned release APK"
fi

(cd "$repo/android" && ./gradlew --no-daemon :app:assembleRelease)

mkdir -p "$repo/dist"
apks=$repo/android/app/build/outputs/apk/release
if $signed; then
    cp "$apks/app-release.apk" "$repo/dist/TuneMesh-$VERSION.apk"
else
    cp "$apks/app-release-unsigned.apk" "$repo/dist/TuneMesh-$VERSION-unsigned.apk"
fi
# Licence notices of the libraries in the APK (also its assets/NOTICE.txt).
cp "$repo/android/app/build/generated/notice/NOTICE.txt" "$repo/dist/TuneMesh-$VERSION-android-NOTICE.txt"
unzip -l "$repo"/dist/TuneMesh-*.apk | grep -q ' assets/NOTICE.txt$'
