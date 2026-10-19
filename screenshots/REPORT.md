# TuneMesh desktop: GTK 4 (Rust) vs Compose Multiplatform – prototype and measurements

Branch `cmp-prototype` in `/home/thomas/agent-work/tunemesh-cmp` (never merged). Base commit `0e57bbd`.

## Recommendation

**Keep the GTK app for Linux; do not migrate now.** The prototype shows the migration is technically easy
(the Android library code, theme and icons compile unchanged for the desktop, and the whole desktop app is
about 2,400 lines, of which about 1,500 are UI code mostly ported from the Android screens), but the desktop result is measurably heavier and less integrated:

- about 2× startup time (1.15 s vs 0.51–0.66 s to a settled window; 0.75 s with a JDK 25 AOT cache),
- about 150 MiB more private memory at idle (PSS 287 vs 122–132 MiB) and about 95 MiB more while playing,
- 3–6× the CPU while scrolling a long list (5.1 vs 0.9 CPU-seconds for the same 5 s of wheel scrolling),
- about 15–20× the install size (134 MiB self-contained app image vs a 6–9 MiB binary on top of system GTK),
- no native Wayland (runs through Xwayland) and **no Linux accessibility at all** (screen readers see nothing),
- MPRIS, desktop integration, Flatpak and Nix packaging all have to be rebuilt on the JVM.

The benefit, one UI codebase for Android and Linux, is real but smaller than it looks: the desktop layout
(sidebar, top bar, table-like track rows, side queue) differs from the phone layout, so roughly half of the
UI code would be shared in practice [INFERENCE], and the Rust `tunemesh-core` stays anyway because the worker uses it.

Revisit when Compose Desktop renders natively on Wayland (Skiko issue
[SKIKO-28](https://youtrack.jetbrains.com/issue/SKIKO-28), still open) and supports Linux accessibility
([docs: "Linux – Not supported"](https://kotlinlang.org/docs/multiplatform/compose-desktop-accessibility.html)).
If a single codebase matters more than these costs, the prototype is a sound starting point; effort below.

The profiling also found a GTK app bug: at idle it re-walked the library and re-read every playlist twice a
second (its file watcher reacted to its own reads), costing 4–8 % of a core. DesktopSlice fixed it on
`pub/desktop` (`89c7c40`); the table shows both builds.

## Setup

- Host: i7-13700K (24 threads), 62 GiB RAM, RTX 4070 Ti SUPER (driver 595.71.05), NixOS, kernel 6.18.
  Other agents' jobs were running on the machine during some runs; app runs were interleaved
  (GTK, CMP, GTK, ...) and CPU figures are the process's own CPU time.
- Isolated session per benchmark: private `dbus-daemon`, headless sway 1.12 rendering with the GPU
  (`WLR_RENDERER=gles2`), Xwayland started up front, 1280×820 output at 60 Hz (both windows tiled to exactly
  that size), a PipeWire null sink (`PIPEWIRE_NODE`/`PULSE_SINK`), and a persistent virtual pointer
  (`zwlr_virtual_pointer_v1`) for wheel input. Every measured playback phase checked that the app's stream was on the null sink (the
harness aborts otherwise).
- Test library: `cmp/bench/gen_library.py`, 3,000 tracks of 20 s (2,700 Opus, 150 AAC `.m4a`, 150 MP3) by
  150 artists on 300 albums (298 distinct album keys), 20 albums with embedded covers, 20 with `cover.jpg`,
  9 playlists (Liked Songs 200, Road Trip 1,000, one with 3 missing entries, one empty), 291 MiB under `/tmp`.
- GTK app: `nix build .#tunemesh` from this worktree (GTK 4.22.4, libadwaita 1.9.3, libmpv 0.41.0; GSK chose
  its Vulkan renderer). Second build: `pub/desktop` at `89c7c40` (idle-loop fix plus that branch's other work),
run with a `config.toml` containing `setup_done = true` and `downloader = false` so its new setup assistant
stays closed and it starts no worker.
- CMP prototype: release build (`createReleaseDistributable`: ProGuard, jlink + jpackage app image) of Compose
  Multiplatform 1.12.2, Material 3 1.9.0, Kotlin 2.4.21 on OpenJDK 25.0.4.1; Skiko used OpenGL through
  Xwayland. Same libmpv 0.41.0 with the same options as the GTK app. JVM variants:
  - *default*: no extra flags (G1, default heap sizing),
  - *lean*: `-XX:+UseSerialGC -Xmx256m -Xss512k -XX:ReservedCodeCacheSize=48m`,
  - *lean + AOT*: lean plus a JDK 25 AOT cache (`-XX:AOTCache`) trained by one browse-and-play session.
- Both apps were driven identically: the GTK app through its `app.navigate`/`app.play-route`/
  `app.toggle-queue`/`app.play-pause`/`app.quit` actions on the private bus, the prototype through the same
  action names on a FIFO.
- Per run: launch → startup measurement → 10 s settle → memory → 60 s idle CPU → visit home, search
  ("lantern"), albums, artists, album, artist, playlist "Road Trip", Liked Songs, playlists (2.5 s each) →
  memory → play "Road Trip", open and close the queue → 60 s playback CPU and memory → pause → 150 wheel
  clicks at 30 Hz over the 1,000-song playlist → quit. 3 runs per app and variant; first launch (no tag cache)
  5 runs; dedicated frame-time runs 3 per app. All figures are medians.
- Startup: *window mapped* = sway's `window new` event; *first paint* = first screenshot with content;
  *settled* = first frame of a screen that then stays unchanged for 1.5 s (grim polling). The OS page cache was
  warm (dropping it needs root); "first launch" means no app tag cache, so the full 3,000-file scan.
- Memory: `Rss`/`Pss` from `/proc/<pid>/smaps_rollup` (whole process tree). GPU memory is not included.

Scratch data (library, app images, raw logs, AOT cache; 558 MiB) is left in `/tmp/tunemesh-cmp-bench`.
Reproduce: `cmp/bench/run.sh` (builds both apps through `heavy`, runs `cmp/bench/bench.py` in a non-quick
`heavy` slot). Raw results: `cmp/bench/results/{main,first-launch,gtk-idle-fix}.json`.

## Results

Medians; memory in MiB; CPU in % of one core.

| Metric | GTK `0e57bbd` | GTK `89c7c40` | CMP default | CMP lean | CMP lean+AOT |
| --- | --- | --- | --- | --- | --- |
| Start: window mapped (s) | 0.44 | 0.58 | 1.07 | 1.03 | 0.67 |
| Start: first paint (s) | 0.47 | 0.62 | 1.10 | 1.07 | 0.71 |
| Start: settled (s) | 0.51 | 0.66 | 1.15 | 1.13 | 0.75 |
| First launch: settled (s) | 0.59 | 0.54 | 1.39 | – | – |
| First launch: scan done (s) | 0.32 | 0.30 | 0.74 | – | – |
| Idle RSS / PSS | 240 / 122 | 237 / 132 | 356 / 287 | 337 / 272 | 307 / 243 |
| After browsing RSS / PSS | 319 / 192 | 316 / 210 | 396 / 327 | 379 / 316 | 348 / 284 |
| Playing RSS / PSS | 354 / 244 | 351 / 242 | 409 / 338 | 390 / 320 | 380 / 315 |
| CPU idle, 60 s | 7.3 % | 0.15 % | 0.02 % | 0.03 % | 0.03 % |
| CPU playing, 60 s | 8.2 % | 1.2 % | 3.6 % | 4.5 % | 2.5 % |
| Scroll: CPU-seconds (6 s window) | 1.84 | 0.91 | 5.12 | 4.38 | 2.95 |
| Scroll: frames/s | 55.4 | 56.0 | 59.0 | – | – |
| Scroll: CPU-seconds (frame runs) | 1.22 | 0.88 | 4.63 | – | – |
| Threads at idle | 44 | 43 | 61 | 38 | 37 |
| Install size | 6.2 MiB¹ | 9.1 MiB¹ | 134 MiB² | 134 MiB² | 134 + 59 MiB³ |

1. Binary only; needs the system's GTK 4, libadwaita and libmpv (preinstalled on most GNOME desktops,
   except libmpv). Compressed (zstd -19): 1.9 MiB.
2. Self-contained jpackage image: 89.7 MiB Java runtime (jlink, 11 modules), 42.6 MiB application jars and
   natives (of which `libskiko-linux-x64.so` 28.3 MiB). 46.1 MiB as `.tar.zst`. Still needs libmpv and libGL.
   Not tuned: jlink `--strip-debug`/compression and fewer modules would cut the runtime.
3. The AOT cache (59 MiB) is specific to the exact JDK build and class path, so it has to be generated
   during packaging or on first run.

Frame notes: GTK frames are its Wayland surface commits (`WAYLAND_DEBUG`); interval p50 16.3 ms, p95 22–24 ms.
CMP frames come from Skiko's FPS counter (it draws through Xwayland, so Wayland logging does not see it);
Skiko flagged a median of 6 long frames per 5 s scroll in the default variant. Both delivered 55–60 frames/s;
the difference is the CPU spent on it. The GTK maximum interval (about 297 ms) is the pause before the
first wheel event, not a stall.

Reading the numbers:

- The base GTK build's idle and playback CPU is its rescan loop, not GTK; compare the `89c7c40` column.
- PSS is the fairer memory figure: the GTK app shares GTK, Vulkan and font libraries with other desktop
  processes, while most JVM memory (heap, metaspace, JIT code) is private.
- JVM tuning helps: Serial GC and a fixed small heap save about 15 MiB PSS; the AOT cache saves about 30 MiB
  more and cuts startup and scroll CPU by about a third, but costs 59 MiB on disk and a packaging step.
- CMP playback CPU (2.5–4.5 % vs 1.2 %) is probably the position poll redrawing the seek bar twice a second
  through the whole Compose frame pipeline [INFERENCE: not profiled].
- The prototype lacks the GTK app's file watcher, MPRIS server, downloads view and state persistence, so a
  full CMP app would use somewhat more [INFERENCE].

## Screenshots

Same 1280×820 window, same library, taken during run 1 of the benchmark (the CMP queue and player shots
show "Road Trip" playing):

- GTK: `/home/thomas/agent-work/tunemesh-cmp/screenshots/gtk/{home,search,albums,artists,album,artist,playlist,liked,playlists,queue}.png`
- CMP: `/home/thomas/agent-work/tunemesh-cmp/screenshots/cmp/{home,search,albums,artists,album,artist,playlist,liked,playlists,songs,queue,player}.png`

The GTK app has no separate full-screen player (`player.png`) and no all-songs list (`songs.png`); its
"Playlists" and "Albums" routes are grids like the CMP library tabs. Visible differences that are prototype
shortcuts, not toolkit limits: playlist and artist tiles use the first track's art (the GTK app picks the
first track that has art, so it shows fewer placeholders), and the GTK app shows album years.

## What the prototype is

`cmp/` in the branch:

- `shared/`: a Kotlin/JVM module whose sources *are* the Android app's
  `android/app/src/main/java/io/github/brainage04/tunemesh/library/` package (649 lines: format rules,
  `LibraryIndex`, `M3u8`, `PlaylistStore`, `Downloads`) and its unit tests; all 28 Android unit tests pass on
  the desktop JVM unchanged. The Android app is untouched.
- `desktop/`: the Compose desktop app (2,414 lines). The Android `AppTheme.kt` and `Icons.kt` are compiled
  unchanged from the Android tree; screens and components are ports of the Android composables with a desktop
  layout: sidebar (Home, Search, Your Library with Liked Songs, Albums, Artists, Songs and playlists),
  back/forward + search bar, home, search, library tabs (playlists, albums, artists, all 3,000 songs), album,
  artist and playlist pages, bottom now-playing bar with seek and volume, full player, side queue panel,
  right-click track menus, playlist dialogs. Resizable window (minimum 900×560).
- Playback: libmpv over JNA (same options as the GTK app's `mpv.rs`). Chosen over vlcj because the GTK app
  already depends on libmpv, so both apps need the same system library, behave the same and can be compared
  directly; vlcj would add VLC (and its plugin tree) as a second media stack.
- Tags: own Ogg (Opus/Vorbis) comment and `METADATA_BLOCK_PICTURE` reader (jaudiotagger has no Opus) plus
  jaudiotagger for M4A/MP3/FLAC; tag cache in `$XDG_CACHE_HOME/tunemesh-cmp/tags.json`. Covers decoded with
  Skia into a 24 MiB LRU like the GTK app.
- Not implemented: downloads, settings/setup, MPRIS, file watching, queue/position persistence, gapless via
  mpv's playlist, track deletion, keyboard shortcuts, single instance.

Run it: `cd cmp && ./gradlew :desktop:createReleaseDistributable -PlibmpvDir=<dir with libmpv.so.2>`, then
`desktop/build/compose/binaries/main-release/app/tunemesh-cmp/bin/tunemesh-cmp --library <folder>`. On NixOS
it also needs `LD_LIBRARY_PATH=<libglvnd>/lib:/run/opengl-driver/lib` (Skiko loads `libGL.so.1`). It only
reads the library, except for liking and playlist edits, which go through the shared `PlaylistStore`.

## Effort estimate for a real migration

Working days for one developer who knows the codebase:

| Work | Days |
| --- | --- |
| Turn `library` into a real shared KMP/JVM module used by Android and desktop; move common composables and the theme into a shared UI module with platform interfaces (player, covers, tag reading, file access, back handling) and a phone vs desktop layout switch; re-test Android | 5–7 |
| Desktop parity with the GTK app: downloads view and worker child process (contract 3), `worker.json` status (contract 4), settings and setup assistant with a folder chooser, file watching, queue/position/volume persistence, gapless playback via mpv's playlist, deleting tracks, drag-and-drop reordering, keyboard shortcuts, single instance, toasts | 12–15 |
| MPRIS on the JVM (dbus-java), including cover art and seeking | 2–3 |
| Packaging: tarball/AppImage/deb/rpm from jpackage, Flatpak with a bundled JRE, a Nix derivation with locked Gradle dependencies, desktop file, icon, AppStream metainfo, AOT cache generation | 4–6 |
| Tests and CI for desktop UI and the shared module | 3–4 |
| **Total** | **26–35 (about 5–7 weeks)** |

Afterwards the Linux UI is a second layout of the Android UI rather than a separate app, so new screens cost
roughly 1.3× one platform instead of 2× [INFERENCE].

## Risks of migrating

- **Wayland**: Compose Desktop draws through AWT's X11 toolkit, so on Wayland desktops it runs under Xwayland
  (verified here: Skiko used OpenGL via GLX on Xwayland). Consequences: blurry text at fractional scales,
  X11 clipboard/drag-and-drop/IME paths, no Wayland-only features. JetBrains Runtime ships a Wayland toolkit
  (WLToolkit) that IntelliJ uses by default since 2026.1, but Skiko's AWT layer does not support it
  ([SKIKO-28](https://youtrack.jetbrains.com/issue/SKIKO-28), [SKIKO-890](https://youtrack.jetbrains.com/issue/SKIKO-890)
  open). The GTK app is native Wayland.
- **Accessibility**: Compose Desktop has no Linux accessibility support; GTK 4 exposes AT-SPI.
- **MPRIS / desktop integration**: no JVM equivalent of GApplication (single instance, D-Bus activation,
  actions) or of the GTK app's `mpris-server`; dbus-java can do MPRIS, but it is new code. Media keys and
  the GNOME/KDE media widgets depend on it.
- **Packaging size and shape**: 134 MiB per install (46 MiB compressed) vs a few MiB; Flatpak needs the
  OpenJDK SDK extension; the Nix build needs a Gradle dependency lock (more work than `buildRustPackage`);
  NixOS needs a wrapper for `libGL.so.1` and libmpv (the prototype bakes `-Djna.library.path` in).
  The stable Material 3 (1.9.0) next to Compose 1.12.2 pulls both androidx and JetBrains copies of some
  runtime/lifecycle artifacts (ProGuard reported 59 duplicate class definitions).
- **Startup and memory**: about +0.5 s and +150 MiB PSS at idle; the AOT cache closes part of the startup gap
  but must be regenerated whenever the JDK or class path changes.
- **Native interop**: libmpv needs `LC_NUMERIC=C` (the JVM sets the user's locale; the prototype calls
  `setlocale` through JNA), JNA needs `--enable-native-access` on recent JDKs, and libmpv must not be destroyed
  while another thread waits for its events (the prototype's first build hung for over 15 s on quit; it exits
  in 0.4 s since the event thread is stopped before `mpv_terminate_destroy`).
- **Look and feel**: Material 3 with the Android theme, not libadwaita; it will not follow GNOME's
  accent/dark settings or window controls without extra work.

## Notes for the integrator

- Nothing from this branch should be merged; it touches no file outside `cmp/` and `screenshots/`.
- For a user-facing guide, this slice adds nothing (prototype only). If the prototype is ever run: it needs
  a JDK 25 to build, libmpv (`libmpv.so.2`) and libGL at run time, and it writes only
  `$XDG_CACHE_HOME/tunemesh-cmp/tags.json` plus liked/playlist edits in the library.
- Contracts: none changed.
