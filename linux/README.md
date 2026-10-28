# TuneMesh for Linux

A Cargo workspace with three crates:

| Crate | What |
| --- | --- |
| `crates/tunemesh-core` | The library format from `../docs/library-format.md`: scanning (tags and covers via `lofty`), metadata fallbacks, playlists, atomic writes, request/status JSON, file-name sanitising, search folding |
| `crates/tunemesh` | The desktop app: GTK 4 + libadwaita, playback through libmpv in-process, MPRIS |
| `crates/tunemesh-worker` | The headless download worker (yt-dlp, Spotify metadata from Spotify's public pages) |

## Build

Everything builds through the flake in the repository root.

```sh
nix build .#tunemesh          # the app (default package)
nix build .#tunemesh-worker   # the worker, wrapped with nixpkgs' yt-dlp, deno and ffmpeg
nix flake check               # builds both and runs the unit tests
```

For development use the dev shell, which has Cargo, GTK, libadwaita, libmpv,
yt-dlp and ffmpeg:

```sh
nix develop
cd linux
cargo test
cargo run -p tunemesh
cargo run -p tunemesh-worker -- --library ~/Music/TuneMesh   # work dir: ~/.cache/tunemesh/worker
```

### Desktop GUI checks

Two tests in `crates/tunemesh/src/gui_tests` drive the real window from inside
the test process. Both need a display, so they are `#[ignore]`d in a plain
`cargo test`; `crates/tunemesh/scripts/gui-checks.sh` runs them in a private
headless sway with a session bus of its own (no portals, none of the
desktop's GTK settings):

```sh
# from the repository root
nix develop -c nix shell --inputs-from . nixpkgs#sway nixpkgs#dbus -c \
  linux/crates/tunemesh/scripts/gui-checks.sh leaks    # or: stalls, all
```

- `leaks` (debug build, about a minute, also in CI): opens every page three
  times on a small generated library, with the keyboard focus inside it, and
  checks through weak references that each page's widgets, list models and
  items are freed once it is left; likewise the queue panel's rows, the
  now-playing bar and the setup assistant. A failure lists what is still
  alive by type and page.
- `stalls` (release build, about 5 minutes plus the build): generates the
  hostile library of `docs/library-format.md` "Limits" (1.5 GB under
  `/var/tmp`), walks every page and action three times and prints the longest
  main-loop stall each caused. It fails when an action exceeds
  `TUNEMESH_STALL_BUDGET_MS` (100 ms) in more than one round (a single late
  round is the machine, not the app). The script draws with
  `GSK_RENDERER=cairo`, so stalls include software rendering; in perf
  profiles most of the 40-50 ms on the grid pages is cairo drawing.
  `TUNEMESH_HOSTILE_LIBRARY=<dir>` reuses a library made with
  `cargo run --release -p tunemesh-core --features hostile --example hostile_library -- <dir>`,
  `TUNEMESH_STALL_ROUNDS=1` shortens the walk and `TUNEMESH_STALL_REPORT=<file>`
  writes the table to a file.

On a shared machine run them through `heavy` (`--mem 4G` for `leaks`, `--mem
8G` for `stalls`). To profile a slow action, record with perf enabled only
around it:

```sh
mkfifo /var/tmp/ctl /var/tmp/ack
CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="perf record -g --call-graph dwarf \
  --control fifo:/var/tmp/ctl,/var/tmp/ack --delay=-1 --" \
TUNEMESH_PERF_CTL=/var/tmp/ctl,/var/tmp/ack TUNEMESH_STALL_PROFILE='downloads,search' \
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
  linux/crates/tunemesh/scripts/gui-checks.sh stalls
```

The same runner variable takes heaptrack (`nix shell nixpkgs#heaptrack`):
`CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER="heaptrack --record-only -o /var/tmp/walk"`
with `TUNEMESH_STALL_BUDGET_MS=1000000`, as heaptrack slows the walk down
(without `--record-only` it opens heaptrack_gui inside the headless session
and waits there). Read the result with `heaptrack_print -f /var/tmp/walk.zst`.
The heap in use after each round, printed under the table, should stay
level from the second round on.

Both checks also run under AddressSanitizer with a nightly toolchain
(`nix shell nixpkgs#rustup`, then `rustup toolchain install nightly`):

```sh
PATH=$(rustup +nightly which rustc | xargs dirname):$PATH \
RUSTFLAGS=-Zsanitizer=address CARGO_TARGET_DIR=/var/tmp/target-asan \
ASAN_OPTIONS=detect_leaks=1:log_path=/var/tmp/asan \
  linux/crates/tunemesh/scripts/gui-checks.sh leaks --target x86_64-unknown-linux-gnu
```

`--target` keeps the sanitizer out of build scripts and proc macros.
LeakSanitizer then lists about 0.9 MB that fontconfig keeps for pango's
font map until exit; anything with a `tunemesh` frame in its stack is a
real leak. For symbol names set `ASAN_SYMBOLIZER_PATH` to
`llvm-symbolizer` (`nixpkgs#llvm`).

## The app

```sh
tunemesh [--library <folder>] [--setup]
tunemesh --version
```

`--library` uses another folder for this session without changing the saved
setting. `--setup` opens the setup assistant, which also opens by itself on
the first start and from Settings.

Requirements: GTK 4.14 and libadwaita 1.5 or newer, and libmpv. The built-in
downloader needs `tunemesh-worker` in the same directory as `tunemesh` (or on
`PATH`) and ffmpeg.

- Config: `~/.config/tunemesh/config.toml`

  ```toml
  library_path = "/home/user/Music/TuneMesh"
  downloader = false   # Settings: "This device downloads songs"
  setup_done = true    # the setup assistant was finished or skipped
  ```

  The default library is `~/Music/TuneMesh`. The Settings page changes it.
- Per-device state (queue, position, shuffle/repeat, volume):
  `~/.local/state/tunemesh/state.json`. The queue comes back paused at the
  saved position on the next launch.
- Playlist bases: `~/.local/state/tunemesh/playlist-base/`, this device's
  copy of each playlist as it was before its own latest edits, shared with a
  worker of the same user and never synced. When two devices change a
  playlist before Syncthing syncs either change, Syncthing keeps one version
  and a `*.sync-conflict-*` copy of the other; the app merges the copy back
  against the base and deletes it, so both edits are kept and an unlike or a
  removal is not undone. A copy that names this device's Syncthing ID is
  merged at once (the app asks its Syncthing for the ID); any other copy
  once it is 10 minutes old, by whichever device sees it then (phones and
  the worker do not know their ID and always wait). Up to 32 copies of one
  playlist are merged at a time, with one write of the playlist; more wait
  for the next pass.
- Playlist edits are written on a background thread; quitting or closing
  the window waits up to 5 seconds for the ones still queued.
- Cache (library scan, MPRIS cover art): `~/.cache/tunemesh/`. The scan cache
  makes startup instant; a background rescan follows, and file changes (for
  example songs arriving through Syncthing) are picked up within a second.
- The built-in downloader's work directory: `~/.cache/tunemesh/worker/`
  (mode 0700).
- The desktop file, icon and AppStream metainfo install as
  `io.github.brainage04.TuneMesh`.

### Setup assistant

1. Library folder: `~/Music/TuneMesh` by default; created if missing.
2. Syncthing: the app reads the GUI address and API key from Syncthing's
   `config.xml` (`~/.config/syncthing/` or `~/.local/state/syncthing/`), or
   asks for them (Syncthing web interface, Actions → Settings → General).
   It then adds the folder with ID `tunemesh` at the library path (or offers
   to use the folder Syncthing already has under that ID), writes the
   library's `.stignore` when missing, shows this device's ID with a QR code,
   and shares the folder with other device IDs you enter. When Syncthing is
   not installed or not running it shows how to install and start it.
3. Downloads: the "This device downloads songs" switch.

Every step can be skipped.

### Downloads

The Downloads page takes links or search text, one per line: songs, albums
and playlists from YouTube, YouTube Music and Spotify, and other sites
yt-dlp supports. Each line becomes one request. When a line is a playlist
link, "Save playlists and albums as TuneMesh playlists" turns on by default.
Each request shows its progress (songs done of the total), failed songs and
warnings. A line longer than 8 KiB is refused and stays in the box.

One device per library downloads. The page and Settings show which device
that is (from `.tunemesh/worker.json`). With "This device downloads songs"
on, the app runs `tunemesh-worker --library <library> --work-dir
~/.cache/tunemesh/worker` as a child process while it is open, restarts it
with a growing delay (up to a minute) if it stops unexpectedly, and stops it
when the app quits. Closing the window while this device still has requests
asks whether to keep downloading in the background; TuneMesh then quits once
the queue is empty (a notification offers "Stop and Quit").

### Playback

Library files arrive from every synced device, so mpv only plays the local
file itself: libavformat is the only demuxer (no playlist, EDL or CUE
handling), references inside files are not followed, nested opens are
limited to local files and nothing next to a file is loaded automatically. A
synced `.mp3` that is really a playlist of URLs fails to play instead of
fetching them.

Keyboard: Space play/pause, Ctrl+F search, Ctrl+→/← next/previous,
Alt+← back, Ctrl+Alt+Q queue panel, Ctrl+Q quit. Double-click plays a
track; right-click opens the track menu (play next, add to queue, add to
playlist, remove from playlist, like, go to album/artist, delete from
library). Playlist tracks and queue entries reorder by drag and drop.
Ctrl+Enter in the Downloads box requests the downloads.

MPRIS: the app registers `org.mpris.MediaPlayer2.tunemesh`, so media keys and
desktop media controls work and show title, artist, cover and position.

Memory: GTK draws with the GPU (Vulkan) by default; while playing, the app
uses about 130 MiB PSS on an NVIDIA desktop, most of it the GPU driver.
`GSK_RENDERER=cairo tunemesh` draws in software instead: about 80 MiB PSS,
at roughly twice the CPU time for redraws.

The application also exports its actions on D-Bus
(`io.github.brainage04.TuneMesh`, object `/io/github/brainage04/TuneMesh`,
interface `org.gtk.Actions`): `play-pause`, `next`, `previous`,
`toggle-queue`, `setup`, `quit`, `navigate` (`home`, `search:<text>`,
`albums`, `artists`, `playlists`, `liked`, `downloads`, `settings`,
`album:<name>`, `artist:<name>`, `playlist:<name>`), `play-route` (same
strings) and `seek` (seconds). For example:

```sh
gdbus call --session --dest io.github.brainage04.TuneMesh \
  --object-path /io/github/brainage04/TuneMesh \
  --method org.gtk.Actions.Activate navigate "[<'album:Pure Tones'>]" "{}"
```

## The worker

```sh
tunemesh-worker [--library <root>] [--work-dir <dir>] [--yt-dlp <path>] [options]
tunemesh-worker --help | --version
```

The desktop app runs it as a child process; it also runs as a service. It
implements "One downloader per library" and "Worker behaviour" from
`docs/library-format.md`.

| Option | Default |
| --- | --- |
| `--library <dir>` | `library_path` from `$XDG_CONFIG_HOME/tunemesh/config.toml`, else `~/Music/TuneMesh` |
| `--work-dir <dir>` | `$XDG_CACHE_HOME/tunemesh/worker`, created with mode 0700 |
| `--yt-dlp <path>` | `$TUNEMESH_YTDLP`, else a managed copy (below) |
| `--device <name>` | the host name; shown to other devices in `worker.json` |
| `--max-items <n>` | 5000 items per request |
| `--max-file-size <size>` | `500M` per download |
| `--max-duration <time>` | `4h` per track; live streams are skipped |
| `--child-timeout <time>` | `30m` per yt-dlp run |
| `--max-pending <n>` | 500: only the oldest requests are read on each scan |
| `--min-free-space <size>` | `2G` kept free on the library's and the work directory's filesystems |
| `--poll-seconds <n>` | 2 |

Exit codes: 0 after SIGTERM/SIGINT (the running request goes back to
`queued`, downloaders are stopped, `worker.json` is removed), 2 for a bad
command line or configuration (including a work directory it refuses), 1 for
other fatal errors.

Programs it runs:

- yt-dlp: `--yt-dlp`, else `$TUNEMESH_YTDLP`, else a managed copy in
  `$XDG_DATA_HOME/tunemesh/yt-dlp/` (`yt-dlp`, `version`, `checked`): the
  official standalone release for x86_64 or aarch64 (glibc or musl), verified
  against the release's `SHA2-256SUMS`, checked for a new release at most once
  a day and replaced atomically. Other CPUs need `TUNEMESH_YTDLP`.
- deno, yt-dlp's JavaScript runtime for YouTube: `$TUNEMESH_DENO`, else `deno`
  on `PATH`, else a managed copy in `$XDG_DATA_HOME/tunemesh/deno/` from the
  official release zip (x86_64 or aarch64, glibc), verified against its
  `.sha256sum` and checked weekly. Without deno, YouTube downloads may fail.
- ffmpeg and ffprobe: `$TUNEMESH_FFMPEG_DIR`, else `PATH`. Required.

Files it creates: in the work directory `jobs/<request id>/` (scratch, removed
after each request), `tmp/`, `cache/yt-dlp/`, `spotify-queries.json` and
`unwritten/` (final statuses it could not write into the library yet, so
their requests are not run again); in the library the downloaded tracks,
playlists, `.tunemesh/status/*.json` and `.tunemesh/worker.json`.

How it works:

- Polls `<root>/.tunemesh/requests/`, gives every new request a `queued`
  status, and processes them oldest first by the id's millisecond prefix, one
  at a time, while it is the library's downloader. Requests left unfinished
  are processed again; finished tracks are found by id, so nothing downloads
  twice.
- Downloads into `<work-dir>/jobs/<request id>/` and moves finished files into
  `<Artist>/<Album or Singles>/<Title> [<id>].<ext>`, re-sanitised to the
  contract's naming rules, never replacing an existing file. When the work
  directory is on another filesystem the move goes through a
  `.~tunemesh-*.tmp` file next to the target.
- Never follows a symlink below the library root: artist and album
  directories, `Playlists/`, `.tunemesh/` and the files in them are opened with
  `O_NOFOLLOW` relative to the root, and anything else is refused.
- All network traffic of yt-dlp, its ffmpeg and the worker's own HTTP client
  goes through an egress proxy inside the worker (127.0.0.1, random port,
  per-run password). For every connection it resolves the host once, refuses
  loopback, private, link-local, CGNAT/Tailscale, unique-local and other
  non-public addresses as well as the host's own interface addresses, and
  connects to the address it checked. yt-dlp only gets formats it downloads
  itself over HTTP(S), HLS or DASH. The host's public IPv4 address behind NAT
  is not covered (only a router firewall can block hairpin NAT).
- Non-Spotify URLs use yt-dlp: the source audio stream is kept (`-x` without
  re-encoding), metadata and a square-cropped thumbnail are embedded, file
  names use YouTube Music's artist/album/track with uploader/title fallbacks,
  and the primary artist is tagged as album artist. Playlists download item by
  item; failed items are listed in the status's `failures`.
- Search text resolves to the first YouTube Music song
  (`music.youtube.com/search?q=…#songs`), falling back to `ytsearch1:`.
- `open.spotify.com` track, album and playlist links need no Spotify account.
  The track list comes from the public embed page
  (`open.spotify.com/embed/<kind>/<id>`, at most 100 tracks, plus an
  anonymous token) and, for the full list with album names and track numbers,
  from the web player's GraphQL endpoint with that token. That endpoint only
  takes query hashes that change with Spotify's web player; the worker knows
  recent ones and otherwise reads the current ones from the web player's
  JavaScript, caching them in `spotify-queries.json`. If it still fails, the
  request goes on with the embed's tracks and a warning says the list was cut
  at 100. Each track is matched on YouTube Music by title and artists, the
  duration must be within 10 % (at least 10 s), and plain YouTube search is
  the fallback. Files are named with the Spotify track id and tagged with
  Spotify's metadata. None of this is a documented Spotify API.
- `playlist` appends the tracks to that playlist (Liked Songs: inserted at the
  top); `playlistFromSource` creates or extends a playlist named after the
  source playlist or album.
- Progress (`progress`, `itemsDone`, `itemsTotal`) is written at most once a
  second.
- Deleting the request file cancels: the running downloader's process group is
  stopped within about a second and the status becomes `cancelled`. Status
  files left unfinished by a request deleted while no worker ran become
  `cancelled` too.
- Every minute the worker looks at the status files whose request file is
  gone, oldest first, one file at a time and reading only their `state`;
  finished ones that are not among the newest 200 status files are deleted,
  since no client lists them. It changes at most 128 status files per pass,
  so a large pile is worked off a minute at a time without holding up
  downloads for long. It then merges playlist conflict copies that are 10
  minutes old (it does not know its Syncthing device ID).
- Playlist edits (downloads added to a playlist) take the same `Playlists/`
  lock as the desktop app, so neither loses the other's edit.
- Logs go to stderr (the journal when run as a service).

### NixOS service

The flake exports `nixosModules.worker`:

```nix
{
  inputs.tunemesh.url = "github:brainage04/TuneMesh";
  inputs.tunemesh.inputs.nixpkgs.follows = "nixpkgs";

  # in the host configuration:
  imports = [ inputs.tunemesh.nixosModules.worker ];
  services.tunemesh-worker = {
    enable = true;
    user = "alice";
    libraryPath = "/home/alice/Music/TuneMesh";
    workDir = "/var/cache/tunemesh-worker"; # default; created 0700 for `user`
    # extraArgs = [ "--max-items" "1000" ];
  };
}
```

This runs `tunemesh-worker.service` as `user` with `Restart=on-failure`
(except after exit code 2); `journalctl -u tunemesh-worker` shows its log.
The unit adds systemd sandboxing on top of the worker's own checks: the
filesystem is read-only except the library and the work directory, and
`IPAddressDeny` blocks private ranges for the whole service, with exceptions
for 127.0.0.1 (the worker's proxy) and the DNS resolvers in
`allowedAddresses` (systemd cannot restrict these to one port). The worker's
caches and its playlist bases (`XDG_CACHE_HOME`, `XDG_STATE_HOME` and the
other XDG directories) are in the work directory, so it keeps its bases in
`<workDir>/.local/state/tunemesh/playlist-base/`, not in the user's home.

## Development checks

Besides `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
`cargo test`, CI runs these on `tunemesh-core` and `tunemesh-worker`. The
nightly ones use the flake's `nightly` dev shell (a pinned nightly Rust with
Miri, plus cargo-fuzz). Run them from the repository root:

```sh
# RustSec advisories, licences, duplicate crates, sources (linux/deny.toml)
nix develop -c bash -c 'cd linux && cargo deny check'

# Miri: undefined behaviour in the pure logic
nix develop .#nightly -c bash -c 'cd linux && cargo miri test -p tunemesh-core --lib'

# AddressSanitizer and LeakSanitizer over the core and worker tests
nix develop .#nightly -c bash -c 'cd linux &&
  RUSTFLAGS=-Zsanitizer=address RUSTDOCFLAGS=-Zsanitizer=address \
  cargo test -Zbuild-std --target x86_64-unknown-linux-gnu \
    -p tunemesh-core -p tunemesh-worker --lib --bins --tests -- \
    --skip huge_album_builds_quickly --skip hostile_library_stays_bounded'

# Every fuzz target for 60 s (CI), or one target for longer
nix develop .#nightly -c linux/fuzz/smoke.sh 60
nix develop .#nightly -c bash -c 'cd linux/fuzz && cargo fuzz run -O playlist_parse -- -max_total_time=900'

# Mutation testing (slow): tests that would not notice a changed line
nix develop -c bash -c 'cd linux && cargo mutants -p tunemesh-core -f src/limits.rs -f src/playlist.rs'
```

Tests that use the file system or child processes are marked
`#[cfg_attr(miri, ignore = "...")]`; every other new core test runs under
Miri as well. The two tests skipped under AddressSanitizer check time and
memory bounds, which the sanitizer's overhead breaks.

The fuzz targets (`linux/fuzz/fuzz_targets/`, a separate Cargo workspace)
cover everything the core and the worker parse from the synced folder or
the network. The worker's parsers are public only with its `fuzzing`
feature, which `linux/fuzz` turns on:

| Target | Input |
| --- | --- |
| `playlist_parse` | A playlist file: limits, and written entries read back the same |
| `playlist_edits` | Like, unlike, append, remove and move, against a plain list |
| `json_files` | Request, status and `worker.json` files |
| `status_limits` | Status files the worker writes stay under 1 MiB |
| `links` | Request input: classification, Spotify and YouTube links |
| `netguard` | The worker's address and URL checks, against `IpAddr::is_global` |
| `ffshim_args` | The ffmpeg and ffprobe shim's argument filter, read as ffmpeg reads it |
| `sanitize` | File and playlist names |
| `stignore` | `.stignore` upkeep |
| `text_limits` | Text cutting and clipping, cover headers, search |
| `remote_metadata` | Spotify embed pages, yt-dlp JSON and progress lines |
| `worker_args` | The worker's command line |

A crash leaves its input in `linux/fuzz/artifacts/<target>/`; `cargo fuzz run
-O <target> <file>` replays it. After the fix, add the input as
`linux/fuzz/corpus/<target>/regression-<what>` (the corpus is replayed on
every run) next to a unit test.
