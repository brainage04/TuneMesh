# TuneMesh Android

Project-specific notes for agents working on the Android app. The root `AGENTS.md`, and `AGENTS.local.md` when
it exists, apply as well.

- Module `app`, variant `debug`, application ID `io.github.brainage04.tunemesh`, launcher activity
  `.MainActivity`. A second exported activity, `.ShareActivity`, is the `ACTION_SEND text/plain` share target.
- Build and test with the checked-in Gradle wrapper from this directory. Set the SDK explicitly: if the shell's
  `ANDROID_HOME` and `ANDROID_SDK_ROOT` point at two different SDKs (for example a Nix store SDK), Gradle refuses
  to build:

  ```sh
  env -u ANDROID_HOME ANDROID_SDK_ROOT="$HOME/Android/Sdk" ./gradlew :app:assembleDebug :app:testDebugUnitTest
  ANDROID_SERIAL=emulator-5554 ./gradlew :app:connectedDebugAndroidTest
  ```

  Verify on an emulator. Install on a physical phone only when the task asks for it, and always select the
  device explicitly (`adb -s <serial>`).

- `minSdk` is 30 (app and baseline profile module): the app reads and writes the Syncthing folder directly
  with `MANAGE_EXTERNAL_STORAGE`, which does not exist before Android 11.
- Instrumentation tests (`connectedDebugAndroidTest`) uninstall the app from the device when they finish.
  Reinstall the built APK before manual checks.
- On an emulator, grant all-files access without the settings UI:
  `adb -s emulator-5554 shell appops set --uid io.github.brainage04.tunemesh MANAGE_EXTERNAL_STORAGE allow`.
  On a real phone the user grants it through the onboarding screen; never change app ops there.
- Test libraries are generated with ffmpeg (sine/noise audio, `-metadata` tags, embedded covers; Opus covers
  as a `METADATA_BLOCK_PICTURE` tag) under `/tmp` and pushed to `/sdcard/Music/TuneMesh` on the emulator only.
  Never commit audio fixtures.
- JVM unit tests cover the library contract (`app/src/test`): playlists, Liked Songs, sanitising, metadata
  fallbacks, request/status/worker JSON, pasted download input. Keep the `io.github.brainage04.tunemesh.library`
  package (`app/src/main/java/io/github/brainage04/tunemesh/library/`) free of Android imports so they stay
  JVM tests.
- Limits on everything read from the synced folder live in one object, `library/Limits.kt`, row for row with
  `docs/library-format.md` "Limits". A new synced input gets a row there and a constant here.
- Stress test: `HostileLibraryTest` (`@Stress`) builds the hostile library of the "Limits" section in app
  storage (100,000 audio files with a 40,000-track album, 50 playlists at the entry limit, a playlist over each
  limit, 500 status files of 1 MiB, 1 MiB tags, a 20 MB cover), lets the app scan it and opens Home, Your
  Library, the big album, a big playlist and Downloads. It fails on an OutOfMemoryError, on a main-thread stall
  over 1 s, or when the state does not follow the limits. The normal instrumented run leaves it out; run it
  alone (emulator only; needs about 2 GB free in the emulator's data partition):

  ```sh
  ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :app:connectedDebugAndroidTest -Pstress
  # quicker, 10,000 files:
  ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :app:connectedDebugAndroidTest -Pstress \
    -Pandroid.testInstrumentationRunnerArguments.hostileScale=small
  ```

  Measurements go to logcat (tag `HostileLibrary`) and the test output.
- Signing: `android/.signing/` (git-ignored) holds the app's key, described in `app/build.gradle.kts`. Never
  delete, regenerate or replace it. Without it builds use the machine's debug key and cannot update an install
  signed with the app's key.
- The app version comes from `linux/Cargo.toml` (`[workspace.package]` `version`, read in
  `app/build.gradle.kts`): versionName is that version, versionCode is major * 10000 + minor * 100 + patch.
  `./gradlew -q :app:printVersion` prints both.
- F-Droid/Obtainium metadata lives in `fastlane/metadata/android/en-US/`. Add
  `changelogs/<versionCode>.txt` for every release.
