# TuneMesh Android

Project-specific notes for agents working on the Android app. A workspace-level `AGENTS.md` above this
repository, when there is one, applies as well.

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
- Signing: `android/.signing/` (git-ignored) holds the app's key, described in `app/build.gradle.kts`. Never
  delete, regenerate or replace it. Without it builds use the machine's debug key and cannot update an install
  signed with the app's key.
- F-Droid/Obtainium metadata lives in `fastlane/metadata/android/en-US/`. Add
  `changelogs/<versionCode>.txt` for every release.
