# Library format

Every TuneMesh client and the download worker share one Syncthing folder and
nothing else. This document is the whole contract between them: if two
implementations disagree, this file decides.

## Folder

| | |
| --- | --- |
| Syncthing folder ID | `tunemesh` |
| Label | `TuneMesh` |
| Linux | `~/Music/TuneMesh` |
| Android | `/storage/emulated/0/Music/TuneMesh` |

Both apps let the user change the path; these are the defaults. On Android the
folder must have Syncthing's "Ignore permissions" set: shared storage reports
every file as 0660, which Syncthing otherwise treats as a local change and
turns into conflict copies of request and status files.

```text
<root>/
  <Artist>/<Album>/<Title> [<source id>].<ext>   audio downloaded by the worker
  ...any other audio, at any depth...             files the user added by hand
  Playlists/<Name>.m3u8                           playlists, including Liked Songs
  .tunemesh/requests/<id>.json                    download requests (clients write)
  .tunemesh/status/<id>.json                      download status (worker writes)
  .tunemesh/worker.json                           which device downloads (worker writes)
  .stignore                                       per device, never synced
```

Symbolic links: any device can put a symlink into the shared folder, so
implementations never follow one below `<root>`. Playlists, `.tunemesh/` and
everything in it, and the directories the worker creates or writes into must
be real directories and regular files; a symlink in their place is refused
(read as missing or invalid, never written through). Symlinked audio files and
directories are not part of the library. Desktop and worker open these paths
with `O_NOFOLLOW` relative to the root (`tunemesh_core::fsx`). Android shared
storage cannot hold symlinks.

## Audio files

- Audio is any regular file under `<root>` with extension `opus`, `ogg`,
  `oga`, `m4a`, `mp4`, `aac`, `mp3`, `flac`, `wav` or `webm` (case-insensitive),
  excluding everything below `.tunemesh/` and `Playlists/`, hidden files and
  directories (leading `.`), Syncthing conflict copies (name contains
  `.sync-conflict-`), and any file or directory whose name contains a line
  break (LF or CR; such a path cannot be written as a playlist line), with
  everything below such a directory. Readers scan at most 64 directory
  levels below `<root>` (see "Limits").
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
- Android's shared storage is case-insensitive, so two names in one directory
  must never differ only in letter case (Syncthing refuses to sync the
  second). A writer creating a directory reuses an existing one whose name
  matches case-insensitively.

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
- Edits on one machine: a writer re-reads the playlist, changes it and writes
  it while holding an exclusive `flock` on an open handle of the
  `Playlists/` directory. The atomic rename replaces the playlist file, so a
  lock on the file itself would not serialise anything. Creating, renaming,
  deleting and merging playlists take the same lock, so the desktop app and
  a worker on the same machine never lose each other's edit. Android has a
  single process and serialises its edits inside it.
- Readers ignore `*.sync-conflict-*` copies when listing and reading; the
  apps merge playlist conflict copies back (see "Playlist conflict copies").
- Writers never write an entry whose path contains LF or CR: adding such a
  track to a playlist skips that entry and adds the rest.
- Size limits, in all implementations: a playlist larger than 32 MiB, with
  more than 250,000 entries, or with a line longer than 32 KiB (without its
  line ending) is too large. Such a playlist is listed as too large, is not
  opened for playing or editing, and is never rewritten or truncated; the
  worker does not add downloads to it and says so in the request's status
  (`warnings`). Writers never write a playlist over these limits (adding to
  a full playlist fails), cut `#EXTINF` labels to 1 KiB and trim Unicode
  White_Space from both ends of a label, as readers do, so rewriting a
  playlist does not change it.

### Playlist conflict copies

When two devices change one playlist before Syncthing has synced either
change, Syncthing keeps one version as `Playlists/<Name>.m3u8` (the winner)
and renames the other to
`Playlists/<Name>.sync-conflict-<YYYYMMDD>-<HHMMSS>-<ID7>.m3u8` (the copy),
where `<ID7>` is the first 7 characters of a Syncthing device ID (see "Who
merges"). The apps merge each copy into the playlist and
delete it, so neither device's edit is lost, and a removal (an unlike, a
removed entry) is never undone by the other device's unchanged entries.

Base. Each device keeps, per library and per playlist, a base: the
playlist's entries as they were before this device's own edits since it
last saw another device's version. When a device edits a playlist (re-read,
change, write, under the lock above), it first sets the base to the version
it re-read, unless that version is exactly the one this device wrote last
(then the base stays, so several own edits in a row keep the version they
started from). After a merge the base is the merged playlist. Bases are
never inside `<root>`, never synced and not backed up: the desktop app and
the worker keep them in `$XDG_STATE_HOME/tunemesh/playlist-base/` (default
`~/.local/state/tunemesh/playlist-base/`), shared by the app and a worker of
the same user and changed under the lock; Android keeps them in app-private
storage (`filesDir/playlist-base/`). A base follows the playlist limits (a
too-large playlist has none); renaming a playlist moves its base, deleting
one removes it.

Who merges. `<ID7>` names the device that made the winning version (tested
with Syncthing 2.1.3: of two concurrent edits the later one wins, and both
devices name the copy after the winner's device). That device merges the
copy as soon as it sees it, if it knows its own Syncthing device ID (the
desktop app asks its Syncthing). Every other device merges a copy only once
the copy's modification time is at least 10 minutes in the past; a device
that cannot know its own ID (Android, which never talks to Syncthing, and
the worker) is always such an other device.

The merge is a pure function of the playlist file (the winner), the copy and
the merging device's base, so two devices with the same base write the same
bytes, and Syncthing does not turn two identical concurrent writes into a
conflict (tested with Syncthing 2.1.3). Two devices with different bases
can write different results (the `different-bases-*` test cases); Syncthing
then keeps the later one and makes the other a new conflict copy, which is
merged in turn. On one machine the merge runs under the `Playlists/` lock
and re-reads the playlist and the copy there, so a second merger finds the
copy gone instead of overwriting the first merger's result.

How. A copy is merged only if its name is exactly as above (`<Name>` not
empty, 8 digits, `-`, 6 digits, `-`, 7 characters of `A`-`Z` and `2`-`7`)
and the playlist `<Name>.m3u8` exists; the winner and the copy are read with
the playlist rules. A too-large or unreadable winner leaves the playlist and
its copies alone. A too-large or unreadable copy, or one whose merge would
be over the limits (step 5), is skipped: it stays until it can be merged or
someone deletes it, and the other copies are merged without it. Copies of
one playlist are merged one at a time, in byte order of their names, each
into the playlist as the one before left it (that is the winner of the
next), all with the same base; the base becomes the merged playlist only
after the last one, and stays while any due copy is left (skipped, or
waiting for a later pass), so a copy merged later is merged against the
base the others were. A copy that is not due yet (see "Who merges") does
not hold the base: it is merged later against the base as it is then. A
copy that can never be merged (over the limits) holds its playlist's base
until someone deletes it, so every implementation reports it in its log.
A device merges the copies of one playlist in memory and
writes the playlist once; the bytes are the same as writing it after each
copy, because entries writers leave out (a line break inside an entry's
line) are left out after each copy in memory too. It merges at most 32
copies of one playlist per pass; the rest wait for its next pass.
An entry's key is its root-relative track path, or its line as written when
it points outside the root. With base B, winner W and copy C, and b, w and c
the number of entries with key k in each:

1. How many entries with key k the result has: when neither side removed
   one (w ≥ b and c ≥ b), r = max(w, c), so a track both devices added is
   there once; when neither side added one (w ≤ b and c ≤ b), r = min(w,
   c); otherwise r = max(0, w + c - b). So an entry one side removed stays
   removed even where the other side kept it, and what either side added is
   kept. Without a base (none kept yet, or unreadable) b counts as 0, so
   r = max(w, c): removals made on another device before this device's
   first edit of the playlist can come back.
2. Order: the result starts from W's entries, or from C's when C changed the
   order and W did not. A side changed the order when its keys in order,
   leaving out keys whose count differs from B's, differ from B's keys in
   order leaving out the same keys. Without a base it starts from W. Of that
   side's entries the first r with each key are kept, in order, and the rest
   dropped.
3. Keys still short of r get entries from the other side, in its order:
   going through it, its j-th entry with key k is matched to the j-th kept
   entry with key k, if there is one, and becomes the anchor. An unmatched
   entry whose key is still short of r is inserted directly after the anchor
   (and after entries inserted there before it), or at the top while there
   is no anchor yet, and becomes the anchor.
4. Every entry keeps the `#EXTINF` duration and label of the file it came
   from. The result is written in the writers' format (`#EXTM3U`, then per
   entry `#EXTINF:<seconds, or -1>,<label>` and its line, LF endings; an
   entry without a label gets its file name's title, see "Audio files")
   without looking anything up in the library, so every device writes the
   same bytes. Liked Songs is merged the same way: a like at the top of the
   copy stays at the top.
5. The merged playlist is written only if it is within the limits, then the
   copy is deleted. A copy whose merge would be too large is skipped (see
   How).

Shared test cases (base, winner, copy and the expected bytes) are in
`docs/test-vectors/playlist-merge/`, and cases for passes over several
copies in `docs/test-vectors/playlist-copies/`; every implementation runs
them.

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
  "requestedBy": "pixel-8",
  "requestedAt": "2026-10-06T05:01:37Z"
}
```

- `id`: Unix milliseconds, `-`, 8 lowercase hex random characters. The worker
  orders requests by the numeric prefix.
- `input`: a URL (YouTube, YouTube Music, Spotify track/album/playlist,
  SoundCloud, Bandcamp, anything yt-dlp supports) or free search text.
- `playlist`: optional playlist name; downloaded tracks are appended to it,
  creating it if needed. An existing playlist whose name differs only in
  letter case is that playlist (`liked songs` is Liked Songs), as for every
  name in one directory (see "Audio files").
- `playlistFromSource`: when true and `playlist` is null, the worker creates or
  appends to a playlist named after the source playlist/album title.
- `requestedBy`: device name, for display only.
- `requestedAt`: RFC 3339 UTC time the request was made, for display only.

The JSON `id` of a request or status file is ignored: the file name decides.
Clients do not write a request whose `input` (after trimming) is longer than
8 KiB; they tell the user it is too long instead (see "Limits").

A client cancels a request by deleting its request file. The worker treats a
missing request file as cancellation between tracks.

### YouTube links

Clients and the worker classify a YouTube or YouTube Music link the same
way (clients use it to preselect `playlistFromSource`, the worker to decide
what to download):

1. The host, lowercased, with one leading `www.` and then one leading `m.`
   removed, is `youtube.com`, `music.youtube.com` or `youtu.be`. Other
   hosts are not YouTube links.
2. The list is the value of the first `list=` parameter in the query
   (before any `#`), case-sensitive and not percent-decoded.
3. `OLAK5uy_...`: an album. `RDCLAK5uy_...`: a playlist (YouTube Music's
   editorial playlists). Any other `RD...`: a single video (an
   auto-generated mix). Any other non-empty value: a playlist. No `list=` or
   an empty value: a single video.

For an album or playlist the worker downloads the whole list from
`https://www.youtube.com/playlist?list=<list>` (`music.youtube.com` for
YouTube Music links), even when the link also names a video, and
`playlistFromSource` names the playlist after it. A single video is
downloaded alone.

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
  "itemsDone": 0,
  "itemsTotal": 1,
  "updatedAt": "2026-10-06T05:01:52Z"
}
```

- `state`: `queued` → `downloading` → `done` | `failed` | `cancelled`. A
  worker that stops mid-request (shutdown, or another device took over)
  sets `queued` again and keeps the request file; the next worker resumes it.
  A status file with any other `state` is invalid and skipped.
- `title`: resolved title of the item or source playlist, once known.
- `progress`: 0–1 for the whole request, or null.
- `message`: human-readable error or note (for example "2 of 30 tracks
  unavailable").
- `tracks`: library-relative paths of the tracks this request produced or
  matched, in source order.
- `itemsDone`, `itemsTotal` (optional integers): items of the request
  finished so far (downloaded, already present or failed) and items it
  covers, once the source is listed. Absent until known.
- `failures` (optional, default empty): items that could not be downloaded,
  as `[{"item": "Artist - Title", "error": "..."}]`, at most 200 entries
  (`itemsDone` and `message` still count all of them).
- `warnings` (optional, default empty): strings the user should see although
  the request went on, for example "Imported the first 100 tracks: ..." when
  a source was cut short. Final `message`s repeat them.
- The worker keeps every status file within the 1 MiB file limit (see
  "Limits"): when one would be larger, it drops entries from the end of
  `tracks`, then `failures`, then `warnings` until it fits.
- After writing a final state the worker deletes the request file. When it
  cannot write the final state (for example a full disk, or a directory
  another device made at the status path), it keeps the status in its
  per-device state directory outside `<root>` (next to its playlist bases,
  so it survives a restart and a cleared work directory) and the request
  file in place, never runs that request again (also after a restart), and
  tries the status again at every poll until it is written (then deletes
  the request file) or the request file is deleted. A request whose status
  is already final is never run again either: when its request file cannot
  be deleted, the worker tries again at every poll and logs the id once a
  minute, and a kept status stays kept until the request file is gone. A
  request whose status cannot be written when it would start is not run;
  the worker goes on with the next one and logs the id once a minute.
- Clients show a request with no status file yet as "Waiting for the
  downloader". Clients may delete status files in a final state ("Clear");
  "Clear" deletes every status file in a final state whose request file is
  gone, not only the listed ones, reading one status file at a time.
- Clients listing downloads keep only what the list shows of each file (see
  "Limits"): of a status's `tracks` only their number. A client that needs
  the paths, to play a finished request, reads that one status file again.
- Every 60 seconds the worker looks at the status files whose request file
  is gone, oldest first, one file at a time and reading only their `state`:
  one that is not final is set to `cancelled` (its request was cancelled
  while no worker was watching); a final one, or a regular file that is not
  a valid status (over the limit, not JSON, no `state`), whose id is not
  among the newest 200 status ids is deleted, since no client lists it (see
  "Limits"). It changes at most 128 of them per pass; the rest wait for the
  next pass.

## One downloader per library

Any number of devices may run the worker; one downloads at a time. The
active worker writes `.tunemesh/worker.json` (atomically) when it starts
downloading and every 60 seconds after that:

```json
{
  "version": 1,
  "device": "desktop",
  "machine": "9b1f04c2a7d35e8860c1f2ab34d5e6f7",
  "instance": "5f0c1d2e3a4b6c7d",
  "pid": 41234,
  "workerVersion": "0.1.0",
  "heartbeatAt": "2026-10-06T05:01:52Z"
}
```

- `device`: device name (the host name unless `--device` says otherwise),
  for display; not unique. `machine`: 32 hex characters identifying the
  machine and its process-id namespace (a SHA-256 hash of `/etc/machine-id`,
  or of a random id kept in `$XDG_DATA_HOME/tunemesh/worker-id`, plus the
  namespace), so a sandboxed worker differs from one on the host; may be
  missing in files from older workers. `instance`: random per worker
  process. `pid`: its process id in that namespace. `workerVersion`: the
  worker's version. Readers ignore fields they do not know.
- A heartbeat is fresh when `heartbeatAt` is less than 3 minutes from now (in
  either direction; device clocks must be roughly in sync).
- A worker does not process requests while the file holds a fresh heartbeat
  of another worker. It keeps running and takes over once that heartbeat is
  stale, or at once when the other heartbeat has the same non-empty
  `machine` as its own and that `pid` no longer exists. Device names never
  decide this.
- An active worker that finds another worker's fresh heartbeat in the file
  (two started at once and Syncthing kept the other write) hands its current
  request back (`queued`) and stops downloading.
- On a clean exit the worker deletes the file if it still holds its own
  heartbeat.
- Clients show "Downloads are handled by <device>" while the heartbeat is
  fresh, otherwise "No device is downloading right now".

## Worker behaviour

The worker (`tunemesh-worker`) runs on any Linux device that holds the
library, as a service or as a child process of the desktop app. It:

1. Processes requests oldest first, one at a time, looking at the oldest 500
   request files only (`--max-pending`).
2. Downloads into a private work directory outside `<root>` (owned by the
   worker's user, mode 0700, refused otherwise), in a run directory of its
   own (`run/<id>/`, kept locked with `flock` while the worker runs, so
   several workers can share one work directory and each removes only run
   directories nobody holds), and moves finished files
   into place, so Syncthing never sees partial files. A file is moved only
   from inside the request's own scratch directory, and only while the
   library's filesystem keeps 2 GiB free (`--min-free-space`).
3. Uses `yt-dlp` for URLs other than Spotify, keeping the source audio stream
   without re-encoding, embedding metadata and a square-cropped cover, and
   naming files `<Artist>/<Album>/<Title> [<id>].<ext>` (`Singles` when there
   is no album). YouTube and YouTube Music playlists and albums (see
   "YouTube links") are downloaded whole; items that fail are skipped and
   listed in `failures`.
4. For `open.spotify.com` track, album and playlist URLs, reads the track list
   from Spotify's public web pages without any account, then finds each track
   on YouTube Music (title and artists; the duration must match within 10 %
   or 10 s), falling back to YouTube search. Files are named with the Spotify
   track id (`<Title> [<Spotify id>].<ext>`) and tagged with Spotify's title,
   artists, album and track number. When only part of a Spotify list could be
   read, `warnings` says so; a list is never cut silently.
5. Treats non-URL input as a search: the first YouTube Music song result, then
   the first YouTube result.
6. Skips a download when a file with the same `[<id>]` already exists and
   reports the existing path in `tracks`.
7. On start, resumes requests whose status is `queued` or `downloading`.
8. Limits each request: at most 5000 items (`--max-items`; the rest is
   reported in `warnings`), files up to 500 MiB (`--max-file-size`), tracks
   up to 4 hours and no live streams (`--max-duration`), 30 minutes per
   yt-dlp run (`--child-timeout`).
9. Refuses (state `failed`, or a `failures` entry for a playlist item) URLs
   that are not http(s) or whose host is or resolves to a loopback, private,
   link-local, CGNAT/Tailscale (`100.64.0.0/10`), IPv6 unique-local or other
   non-public address (IPv6: every block the IANA special-purpose registry
   marks as not globally reachable, including local-use NAT64
   `64:ff9b:1::/48`, `::/96` and Teredo), or to an address on one of the host's own networks
   (an interface address with its on-link prefix, a /64 for a /128 IPv6
   address, and every route without a gateway, so global IPv6 addresses of
   LAN neighbours such as the router or a NAS are refused too). Addresses
   that reach an IPv4 address (IPv4-mapped, NAT64 `64:ff9b::/96`, SIIT
   `::ffff:0:0:0/96`, 6to4 `2002::/16`) count as that IPv4 address. The
   message in the status file is the same for refused and unresolvable hosts
   and names no address; details go to the worker's local log only. All
   network traffic of yt-dlp, its ffmpeg and the worker itself goes through
   the worker's filtering proxy, which applies this check to every
   connection, redirects included, and connects only to the address it
   checked. This does not cover the host's public IPv4 address behind NAT
   (reachable through the router's hairpin NAT), other internal networks
   that use globally routable addresses but are not on-link here (behind
   another router, a VPN), or services that other hosts on the internet can
   reach anyway; the NixOS module adds a firewall layer.
10. Runs ffmpeg and ffprobe (yt-dlp's postprocessing) only on files inside
    the request's scratch directory and only with demuxers for plain audio,
    video and image containers (`-format_whitelist`, `-protocol_whitelist
    file`, placed after the caller's options so they win), and only with
    the options yt-dlp's postprocessing uses (no other file-taking options
    such as `-filter_script` or `-progress`), so a download that is really
    a playlist or script referring to other files (HLS, ffconcat, DASH)
    fails instead of copying local files into the library. A `-vf` filter
    graph may name only the filters `crop`, `scale`, `pad`, `format` and
    `setsar` (filters such as `movie` or `subtitles` open files of their
    own).

## Limits

Every file in `<root>` can come from any device that shares the folder, so
every reader bounds what it reads, keeps and shows. These are the limits of
all implementations (Rust: `tunemesh_core::limits`; Android: `Limits`).
Real libraries stay far below them. Sizes are bytes of UTF-8; text is cut
at a character boundary.

Every field of a synced JSON file is listed here; fields a reader does not
know are ignored and not kept. `version`, `pid`, `itemsDone` and
`itemsTotal` are integers from 0 to 2^32 - 1 and `progress` a number;
another value makes the file invalid (read as described in the first row).

| Input | Limit | Past the limit |
| --- | --- | --- |
| A request, status or `worker.json` file | 1 MiB | Not read: a request is reported as an invalid request file, a status file is skipped, `worker.json` counts as absent |
| `<root>/.stignore` | 1 MiB | Left unchanged (not read or rewritten); the reader logs it |
| Request `input` | 8 KiB | Clients do not write it; the worker fails the request ("too long"); readers cut it and keep only that it was too long |
| Request `playlist` | 1 KiB | Cut |
| Request `requestedBy`; `worker.json` `device` | 1 KiB | Cut |
| Request `requestedAt`; status `updatedAt`; `worker.json` `heartbeatAt` | 64 B | Cut |
| The JSON `id` of a request or status file | 64 B | Cut; ignored (the file name is the id) |
| `worker.json` `machine`, `instance` and `workerVersion` | 64 B each | Cut |
| Request `playlistFromSource` | a boolean | Otherwise invalid |
| Request files the worker considers | the oldest 500 (`--max-pending`) | The others wait their turn |
| Request and status files clients read | the newest 200 ids (a request with its status file counts once) | Older ones are counted but not read or shown |
| Status files the worker reads | one at a time: the `state` of those whose request file is gone; at most 128 set to `cancelled` or deleted per pass, oldest first | Final and invalid ones past the newest 200 status ids are deleted (see "Download status"); the rest of the changes wait for the next pass |
| Status `input` | 8 KiB | Cut |
| Status `title`; each warning; each failure's `item` and `error` | 1 KiB | Cut |
| Status `message` | 4 KiB | Cut |
| Status `warnings` | 50 entries | The rest is dropped |
| Status `failures` | 200 entries | The rest is dropped (`itemsDone` and `message` still count all) |
| Status `tracks` | 10,000 paths of 4 KiB each | The rest is dropped and longer paths are cut. Clients listing downloads keep only the number of paths (at most 10,000); a client reading one status to play it keeps the paths |
| A playlist file | 32 MiB, 250,000 entries, lines of 32 KiB (without the line ending) | Too large: listed as too large, not opened for playing or editing, never rewritten or truncated (see "Playlists") |
| Playlists listed | 1,000 (by name, case-insensitively), Liked Songs not counted | The others are not listed; Liked Songs is always shown |
| An `#EXTINF` label | 1 KiB | Cut when read and when written |
| A tag text field (title, artist, album, album artist) | 1 KiB | Cut |
| Artists in one tag | 32 | The rest is dropped |
| A single tag item (a picture or text frame) | 16 MiB | Not used (a reader may have to load it once to see its size); the track keeps its other tags and its fallbacks |
| An embedded or folder cover | 16 MiB, and 5,000 pixels per side as stated in the image header (JPEG, PNG, WebP or GIF) | Not shown: the placeholder is drawn. A cover whose size cannot be read from its header is not shown either |
| Folder depth below `<root>` | 64 levels | Deeper folders are not scanned |
| Entries read from one folder | 100,000 | The rest of that folder is not scanned |
| Audio files in the library | 250,000 | Files past the limit (in scan order) are left out |
| Tracks in one album or by one artist | no limit | Grouping and sorting are O(n log n), so a 40,000-track album costs tens of milliseconds |

Display limits (what a UI shows of text it read):

| Text | Shown |
| --- | --- |
| A single-line text (titles, names, labels, paths) | at most 300 characters, one line |
| A message or warning | at most 2,000 characters and 20 lines |
| A list read from a synced file (warnings, failures, tracks of a status, status files) | the first 200 rows, then "and N more" |

Cut text ends in `…`.

## Local state

Queue, playback position, volume, shuffle/repeat and settings are per device
and never stored in `<root>`.

## `.stignore`

Each device keeps this in `<root>/.stignore` (Syncthing does not sync it; the
desktop app, the Android app and the worker add these rules when missing,
keeping the user's other lines):

```text
// TuneMesh
(?d).~tunemesh-*
```
