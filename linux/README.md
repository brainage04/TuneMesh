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
warnings.

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
after each request), `tmp/`, `cache/yt-dlp/` and `spotify-queries.json`; in
the library the downloaded tracks, playlists, `.tunemesh/status/*.json` and
`.tunemesh/worker.json`.

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
`allowedAddresses` (systemd cannot restrict these to one port).
