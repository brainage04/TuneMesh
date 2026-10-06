# Library format

Every TuneMesh client and the download worker share one Syncthing folder and
nothing else. This document is the whole contract between them: if two
implementations disagree, this file decides.

## Folder

| | |
| --- | --- |
| Syncthing folder ID | `tunemesh` |
| Label | `TuneMesh` |
| Linux hosts | `/home/thomas/Music/TuneMesh` |
| Android | `/storage/emulated/0/Music/TuneMesh` |

Both apps let the user change the path; these are the defaults.

```text
<root>/
  <Artist>/<Album>/<Title> [<source id>].<ext>   audio downloaded by the worker
  ...any other audio, at any depth...             files the user added by hand
  Playlists/<Name>.m3u8                           playlists, including Liked Songs
  .tunemesh/requests/<id>.json                    download requests (clients write)
  .tunemesh/status/<id>.json                      download status (worker writes)
  .stignore                                       per device, never synced
```

## Audio files

- Audio is any regular file under `<root>` with extension `opus`, `ogg`,
  `oga`, `m4a`, `mp4`, `aac`, `mp3`, `flac`, `wav` or `webm` (case-insensitive),
  excluding everything below `.tunemesh/` and `Playlists/`, hidden files and
  directories (leading `.`), and Syncthing conflict copies (name contains
  `.sync-conflict-`).
- A track's identity is its path relative to `<root>`, with `/` separators.
- Metadata comes from embedded tags. Fallbacks when a tag is missing:
  title = file name without extension and without a trailing ` [<id>]`;
  album = parent directory name (or `Unknown Album` at the root);
  artist = grandparent directory name (or `Unknown Artist`).
- The first artist is the first artist tag value, split on `;` (and NUL),
  trimmed. Albums group by (album artist, falling back to the first artist;
  album). Artists list album artist, falling back to the first artist.
- Cover art: the embedded picture; otherwise `cover.jpg`, `cover.png`,
  `folder.jpg` or `folder.png` in the track's directory; otherwise a generated
  placeholder.
- File and directory names written by TuneMesh never contain
  `/ \ : * ? " < > |` or control characters (Android's shared storage rejects
  them), never start with `.` (readers skip hidden names), never end in `.` or
  space, and each component is at most 120 bytes. Replace offending
  characters with `_`.

## Playlists

- One file per playlist: `Playlists/<Name>.m3u8`, UTF-8, no BOM, LF line
  endings. The playlist name is the file name without `.m3u8`.
- Format:

  ```text
  #EXTM3U
  #EXTINF:213,Artist - Title
  ../Artist/Album/Title [dQw4w9WgXcQ].opus
  ```

  Entry paths are relative to the `Playlists/` directory and are not
  URL-encoded. `#EXTINF` lines are optional for readers and always written by
  writers (duration in whole seconds, `-1` if unknown). Readers ignore other
  `#` lines and blank lines; they also accept absolute paths inside `<root>`.
  Entries whose file is missing stay in the playlist and are shown as
  unavailable; they are never silently dropped when rewriting.
- File order is play order. Duplicates are allowed.
- `Playlists/Liked Songs.m3u8` is Liked Songs. Liking a track inserts it at the
  top; unliking removes every entry for that track. It cannot be renamed or
  deleted from the UI.
- Writers write atomically: write `.~tunemesh-<random>.tmp` in the same
  directory, then rename it over the target. Never edit in place.
- Readers ignore `*.sync-conflict-*` copies.

## Download requests

A client asks for a download by creating
`.tunemesh/requests/<id>.json` (atomically, as above):

```json
{
  "version": 1,
  "id": "1791265297123-3f9c2a1b",
  "input": "https://www.youtube.com/watch?v=jNQXAC9IVRw",
  "playlist": null,
  "playlistFromSource": false,
  "requestedBy": "thomass-s21",
  "requestedAt": "2026-10-06T05:01:37Z"
}
```

- `id`: Unix milliseconds, `-`, 8 lowercase hex random characters. The worker
  orders requests by the numeric prefix.
- `input`: a URL (YouTube, YouTube Music, Spotify track/album/playlist,
  SoundCloud, Bandcamp, anything yt-dlp supports) or free search text.
- `playlist`: optional playlist name; downloaded tracks are appended to it,
  creating it if needed.
- `playlistFromSource`: when true and `playlist` is null, the worker creates or
  appends to a playlist named after the source playlist/album title.
- `requestedBy`: device name, for display only.

A client cancels a request by deleting its request file. The worker treats a
missing request file as cancellation between tracks.

## Download status

Only the worker writes `.tunemesh/status/<id>.json` (atomically):

```json
{
  "version": 1,
  "id": "1791265297123-3f9c2a1b",
  "input": "https://www.youtube.com/watch?v=jNQXAC9IVRw",
  "state": "downloading",
  "title": "Me at the zoo",
  "progress": 0.42,
  "message": null,
  "tracks": ["jawed/Singles/Me at the zoo [jNQXAC9IVRw].opus"],
  "updatedAt": "2026-10-06T05:01:52Z"
}
```

- `state`: `queued` → `downloading` → `done` | `failed` | `cancelled`.
- `title`: resolved title of the item or source playlist, once known.
- `progress`: 0–1 for the whole request, or null.
- `message`: human-readable error or note (for example "2 of 30 tracks
  unavailable").
- `tracks`: library-relative paths of the tracks this request produced or
  matched, in source order.
- After writing a final state the worker deletes the request file.
- Clients show a request with no status file yet as "Waiting for the
  downloader". Clients may delete status files in a final state ("Clear").

## Worker behaviour

The worker runs on exactly one host (thomas-server). It:

1. Processes requests oldest first, one at a time.
2. Downloads into a work directory outside `<root>` and moves finished files
   into place, so Syncthing never sees partial files.
3. Uses `yt-dlp` for URLs other than Spotify, keeping the source audio stream
   without re-encoding, embedding metadata and a square-cropped cover, and
   naming files `<Artist>/<Album>/<Title> [<id>].<ext>` (`Singles` when there
   is no album).
4. Uses `spotdl` for `open.spotify.com` URLs (Spotify is used for metadata only;
   audio comes from YouTube).
5. Treats non-URL input as a search: the first YouTube Music song result, then
   the first YouTube result.
6. Skips a download when a file with the same `[<id>]` already exists and
   reports the existing path in `tracks`.
7. On start, resumes requests whose status is `queued` or `downloading`.

## Local state

Queue, playback position, volume, shuffle/repeat and settings are per device
and never stored in `<root>`.

## `.stignore`

Each device keeps this in `<root>/.stignore` (Syncthing does not sync it; the
NixOS configuration writes it on Linux hosts and the Android app writes it when
missing):

```text
// TuneMesh
(?d).~tunemesh-*
```
