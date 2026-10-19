# TuneMesh for Linux

A Cargo workspace with three crates:

| Crate | What |
| --- | --- |
| `crates/tunemesh-core` | The library format from `../docs/library-format.md`: scanning (tags and covers via `lofty`), metadata fallbacks, playlists, atomic writes, request/status JSON, file-name sanitising, search folding |
| `crates/tunemesh` | The desktop app: GTK 4 + libadwaita, playback through libmpv in-process, MPRIS |
| `crates/tunemesh-worker` | The headless download worker (yt-dlp / spotDL) that runs on thomas-server |

## Build

Everything builds through the flake in the repository root.

```sh
nix build .#tunemesh          # the app (default package)
nix build .#tunemesh-worker   # the worker, wrapped with yt-dlp, ffmpeg and spotdl on PATH
nix flake check               # builds both and runs the unit tests
```

For development use the dev shell, which has Cargo, GTK, libadwaita, libmpv,
yt-dlp, ffmpeg and spotdl:

```sh
nix develop
cd linux
cargo test
cargo run -p tunemesh
cargo run -p tunemesh-worker -- --library ~/Music/TuneMesh --work-dir /tmp/tunemesh-work
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
tunemesh-worker --library <root> --work-dir <dir> [--poll-seconds <n>]
```

It implements "Worker behaviour" from `docs/library-format.md`:

- Polls `<root>/.tunemesh/requests/` (every 2 s by default), gives every new
  request a `queued` status, and processes them oldest first by the id's
  millisecond prefix, one at a time. Requests still unfinished after a restart
  are processed again; finished tracks are found by id, so nothing downloads
  twice.
- Downloads into `<work-dir>/<request id>/` and moves finished files into
  `<Artist>/<Album or Singles>/<Title> [<id>].<ext>`, re-sanitised to the
  contract's naming rules. When the work directory is on another filesystem the
  move goes through a `.~tunemesh-*.tmp` file next to the target.
- Non-Spotify URLs use yt-dlp: the source audio stream is kept (`-x` without
  re-encoding), metadata and a square-cropped thumbnail are embedded, file
  names use YouTube Music's artist/album/track with uploader/title fallbacks,
  and the primary artist is tagged as album artist.
- Search text resolves to the first YouTube Music song
  (`music.youtube.com/search?q=…#songs`), falling back to `ytsearch1:`.
- `open.spotify.com` URLs use `spotdl` (metadata from Spotify, audio from
  YouTube Music) with `--format opus --bitrate disable`, which copies YouTube's
  Opus stream instead of re-encoding.
- A file whose name already carries `[<id>]` is not downloaded again; its path
  is reported in `tracks`.
- `playlist` appends the tracks to that playlist (Liked Songs: inserted at the
  top); `playlistFromSource` creates or extends a playlist named after the
  source playlist or album.
- Progress comes from yt-dlp's download progress per track; status files are
  written at most once a second.
- Deleting the request file cancels: the running downloader's process group is
  stopped within about a second and the status becomes `cancelled`. Status
  files left unfinished by a request deleted while the worker was down become
  `cancelled` too.
- Logs go to stderr (the journal when run as a service).

### NixOS service

The flake exports `nixosModules.worker`:

```nix
{
  inputs.tunemesh.url = "git+ssh://git@github.com/brainage04/TuneMesh";
  inputs.tunemesh.inputs.nixpkgs.follows = "nixpkgs";

  # in the thomas-server configuration:
  imports = [ inputs.tunemesh.nixosModules.worker ];
  services.tunemesh-worker = {
    enable = true;
    user = "thomas";
    libraryPath = "/home/thomas/Music/TuneMesh";
    workDir = "/var/cache/tunemesh-worker"; # default; created for `user`
  };
}
```

This runs `tunemesh-worker.service` as `user` with `Restart=on-failure`;
`journalctl -u tunemesh-worker` shows its log. spotDL keeps its settings and
cache in the user's `~/.spotdl`.
