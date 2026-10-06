# TuneMesh Android

Follows `/home/thomas/01_TM/Coding/Android Apps/AGENTS.md`. Project-specific notes:

- Module `app`, variant `debug`, application ID `io.github.brainage04.tunemesh`, launcher activity
  `.MainActivity`. A second exported activity, `.ShareActivity`, is the `ACTION_SEND text/plain` share target.
- Run the cycle from the repository root with the SDK set explicitly; the login shell's `ANDROID_HOME` and
  `ANDROID_SDK_ROOT` can point at a Nix store SDK and Gradle refuses two different SDK locations:

  ```sh
  env -u ANDROID_HOME ANDROID_SDK_ROOT=/home/thomas/.local/share/android-sdk \
    "/home/thomas/01_TM/Coding/Android Apps/tools/android-cycle" run android
  ```

- `minSdk` is 30 (app and baseline profile module): the app reads and writes the Syncthing folder directly
  with `MANAGE_EXTERNAL_STORAGE`, which does not exist before Android 11.
- Instrumentation tests (`connectedDebugAndroidTest`, run by `android-cycle`) uninstall the app from the
  emulator when they finish. Reinstall the built APK on `emulator-5554` before manual checks. This never
  touches the phone.
- On the emulator, grant all-files access without the settings UI:
  `adb -s emulator-5554 shell appops set --uid io.github.brainage04.tunemesh MANAGE_EXTERNAL_STORAGE allow`.
  On the phone the user grants it through the onboarding screen; never change app ops on the phone.
- Test libraries are generated with ffmpeg (sine/noise audio, `-metadata` tags, embedded covers; Opus covers
  as a `METADATA_BLOCK_PICTURE` tag) under `/tmp` and pushed to `/sdcard/Music/TuneMesh` on the emulator only.
  Never commit audio fixtures.
- JVM unit tests cover the library contract (`app/src/test`): playlists, Liked Songs, sanitising, metadata
  fallbacks, request/status JSON. Keep `library/` free of Android imports so they stay JVM tests.
