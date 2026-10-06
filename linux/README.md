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
tunemesh [--library <folder>]
```

`--library` uses another folder for this session without changing the saved
setting.

- Config: `~/.config/tunemesh/config.toml`

  ```toml
  library_path = "/home/thomas/Music/TuneMesh"
  ```

  The default is `~/Music/TuneMesh`. The Settings page changes it.
- Per-device state (queue, position, shuffle/repeat, volume):
  `~/.local/state/tunemesh/state.json`. The queue comes back paused at the
  saved position on the next launch.
- Cache (library scan, MPRIS cover art): `~/.cache/tunemesh/`. The scan cache
  makes startup instant; a background rescan follows, and file changes (for
  example songs arriving through Syncthing) are picked up within a second.
- The desktop file and icon install as `io.github.brainage04.TuneMesh`.

Keyboard: Space play/pause, Ctrl+F search, Ctrl+→/← next/previous,
Alt+← back, Ctrl+Alt+Q queue panel, Ctrl+Q quit. Double-click plays a
track; right-click opens the track menu (play next, add to queue, add to
playlist, remove from playlist, like, go to album/artist, delete from
library). Playlist tracks and queue entries reorder by drag and drop.

MPRIS: the app registers `org.mpris.MediaPlayer2.tunemesh`, so media keys and
KDE's media controls work and show title, artist, cover and position.

The application also exports its actions on D-Bus
(`io.github.brainage04.TuneMesh`, object `/io/github/brainage04/TuneMesh`,
interface `org.gtk.Actions`): `play-pause`, `next`, `previous`,
`toggle-queue`, `navigate` (`home`, `search:<text>`, `albums`, `artists`,
`playlists`, `liked`, `downloads`, `settings`, `album:<name>`,
`artist:<name>`, `playlist:<name>`), `play-route` (same strings) and
`seek` (seconds). For example:

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
