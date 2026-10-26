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
  `docs/library-format.md` "Limits". A new synced input gets a row there and a constant here. Each synced format
  has one `bounded()` projection (`DownloadStore.bounded`, the `M3u8` reader, `Stignore.withRules`), and readers
  keep only what the UI shows (the Downloads list keeps a `DownloadItem` per request, never the files).
  `BoundedModelsTest` lists every property of every parsed model and of what the app keeps of it with its limit,
  and fails on a property (or a new `@Serializable` model in `library/`) without one; `MemoryBudgetTest` asserts
  what each format retains at its limits, and `DownloadsHeapTest` lists the maximal downloads twice in a 192 MB JVM.
- Playlist conflict copies (contract: "Playlist conflict copies") are merged by `PlaylistStore.mergeConflicts`
  against this device's bases in `filesDir/playlist-base/<library>/`; `PlaylistMergeTest` runs the shared cases in
  `docs/test-vectors/playlist-merge/`.
- Stress test: `HostileLibraryTest` (`@Stress`) builds the hostile library of the "Limits" section in app
  storage (100,000 audio files with a 40,000-track album, 50 playlists at the entry limit, a playlist over each
  limit and one whose every label is over its limit, requests and status files of just under 1 MiB with every
  field past its limit, 1 MiB tags, a 20 MB cover), lets the app scan it and opens Home, Your
  Library, the big album, a big playlist and Downloads. It fails on an OutOfMemoryError, on a main-thread stall
  over 1 s, or when the state does not follow the limits. The normal instrumented run leaves it out; run it
  alone (emulator only; needs about 3 GB free in the emulator's data partition):

  ```sh
  ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :app:connectedDebugAndroidTest -Pstress
  # quicker, 10,000 files:
  ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :app:connectedDebugAndroidTest -Pstress \
    -Pandroid.testInstrumentationRunnerArguments.hostileScale=small
  ```

  Measurements go to logcat (tag `HostileLibrary`) and the test output. The generator, `hostile-library/`, is shared
  with the macrobenchmarks (see "Development checks"); its plain-Kotlin part (`hostile-library/common/`,
  `HostileFiles`: synced files with every field at its limit) also serves the JVM unit tests.
- Signing: `android/.signing/` (git-ignored) holds the app's key, described in `app/build.gradle.kts`. Never
  delete, regenerate or replace it. Without it builds use the machine's debug key and cannot update an install
  signed with the app's key.
- The app version comes from `linux/Cargo.toml` (`[workspace.package]` `version`, read in
  `app/build.gradle.kts`): versionName is that version, versionCode is major * 10000 + minor * 100 + patch.
  `./gradlew -q :app:printVersion` prints both.
- F-Droid/Obtainium metadata lives in `fastlane/metadata/android/en-US/`. Add
  `changelogs/<versionCode>.txt` for every release.

## Development checks

Run these from this directory before committing (they also run in CI):

- Formatting: ktfmt (kotlinlang style, 140 columns) through Spotless, for every `.kt` and `.gradle.kts` file.
  `./gradlew spotlessCheck` fails on a difference, `./gradlew spotlessApply` rewrites the files. ktfmt was chosen
  over ktlint because it is a pretty-printer: its output is deterministic and it always fixes what it reports,
  while ktlint leaves long lines to fix by hand and wraps some of them mid-expression.
- Static analysis: detekt with type resolution, `./gradlew detektAll` (the app's main, unit-test and instrumented-test
  sources and the baseline profile module). `config/detekt.yml` lists the deviations from the defaults and the bug-pattern
  rules switched on (coroutines, exceptions, null safety). `app/detekt-baseline-<variant>.xml` holds the style findings
  (sizes, complexity, magic numbers, long strings) of code older than detekt; every other finding fails. Fix a finding,
  or suppress it at the one place with `@Suppress("Rule") // reason`; rewrite a baseline only to drop fixed entries
  (`./gradlew :app:detektBaselineDebug`).
- Fuzzing: Jazzer `@FuzzTest`s in `app/src/test` (`*FuzzTest`: the M3u8 reader and writer, request/status/`worker.json`
  parsing, pasted and shared links, the name sanitiser). `testDebugUnitTest` replays the saved inputs in
  `app/src/test/resources/<package>/<Class>Inputs/<method>/`; `./gradlew --no-daemon :app:fuzz -Pfuzz.seconds=60`
  fuzzes each target in turn for that long (through `heavy`, as any Gradle run). A failing input is written to that
  directory, or to `app/crash-<hash>` when the directory does not exist yet: fix the bug, add a unit test, and commit
  the input under that directory so it stays a regression input. The generated corpus (`app/.cifuzz-corpus/`) is
  git-ignored.
- Leaks: LeakCanary is in debug builds (`debugImplementation`), and every instrumented test class has
  `@get:Rule val leaks = DetectLeaksAfterTestSuccess()`: a test fails when an activity, view model, service or view it
  destroyed is still reachable afterwards (the heap analysis is in the test output). `ActivityLeakTest` opens every tab,
  an album and the share sheet, and recreates them as a rotation does. Give a new instrumented test class the same rule.
- Stalls: `baselineprofile/` holds the macrobenchmarks (`HostileLibraryBenchmark`: start, search, the 40,000-track
  album, a 250,000-entry playlist, Downloads, scrolling Your Library) and the baseline profile generator. Both run on
  the hostile library, which `HostileSetup` generates once in the emulator's `/sdcard/Music/TuneMesh` (it refuses a
  folder holding anything else), against the minified `benchmarkRelease` build. Emulator only, through `heavy`; the
  first run generates the library and scans it, about 45 minutes in all:

  ```sh
  ANDROID_SERIAL=emulator-5554 ./gradlew --no-daemon :baselineprofile:connectedBenchmarkReleaseAndroidTest \
    -Pandroid.injected.androidTest.leaveApksInstalledAfterRun=true \
    -Pandroid.testInstrumentationRunnerArguments.class=io.github.brainage04.tunemesh.baselineprofile.HostileLibraryBenchmark
  ```

  Results (JSON) and a Perfetto trace per iteration land in
  `baselineprofile/build/outputs/connected_android_test_additional_output/`; open a trace at ui.perfetto.dev.
  `MainThreadStallMetric` reports the longest main-thread stall (`mainThreadStallMaxMs`), the same without the time
  the main thread waits for its render thread (`mainThreadWorkMaxMs`: on the emulator's software renderer a frame
  can wait 250 ms there, which no app change removes) and the longest frame. Budget: no stall of the app's own
  work over 100 ms; profile anything over 50 ms. `./gradlew --no-daemon :app:generateBaselineProfile` regenerates
  `app/src/main/generated/baselineProfiles/` after UI changes; commit the result.
