# Security

## Reporting a vulnerability

Report vulnerabilities privately through GitHub: open the repository's
**Security** tab and choose **Report a vulnerability**. Please do not open a
public issue for them. You should get a reply within a week.

Only the latest release is supported.

## Threat model

TuneMesh devices share one Syncthing folder and nothing else. Every device that
can write to that folder can already add, change or delete any song or
playlist, so TuneMesh does not try to protect the library from them. It does
try to stop such a device from reaching anything outside the library:

- **Files outside the library.** Symlinks below the library root are never
  followed. Playlists, `.tunemesh/` and every directory the worker writes
  into are opened relative to the root with `O_NOFOLLOW`; symlinked audio
  files and directories are not part of the library.
- **The downloader's network.** Download requests are URLs or search text
  written by any device. `tunemesh-worker` sends all of yt-dlp's traffic, and
  its own, through a proxy inside the worker that refuses loopback, private,
  link-local, CGNAT/Tailscale, unique-local and other not globally reachable
  addresses (IPv6 forms of IPv4 addresses such as NAT64 and 6to4 count as
  the IPv4 address they embed), the host's own
  interface addresses and every address on the host's own networks (their
  on-link prefixes and gateway-less routes, which covers LAN devices with
  global IPv6 addresses).
  Redirects, manifests, fragments and thumbnails go through the same check.
  Not detected: a port forwarded on your router and reached through your
  public IPv4 address (hairpin NAT), and internal networks with globally
  routable addresses that are not directly attached to the downloading host
  (behind another router or a VPN).
- **Resource use.** Requests are limited in items, file size, duration, run
  time, number of pending requests and free disk space (see
  `tunemesh-worker --help`). Every synced file has a size limit, and readers
  keep only a bounded, cut copy of each field and list
  (`docs/library-format.md`, "Limits"), so what an app or the worker holds
  in memory does not grow with what a sharing device writes: the worker's
  status sweep reads one file at a time, and lists of downloads keep counts
  instead of track lists.
- **ffmpeg.** yt-dlp's ffmpeg and ffprobe run through a shim that allows
  only the options, input formats and protocols its postprocessing uses,
  inputs inside the request's scratch directory, and `-vf` filter graphs
  of a few filters that open no files.
- **Playback.** The desktop player only opens local files: mpv's playlist,
  EDL, CUE and archive handling and every network protocol are disabled.

Out of scope: a device you share the folder with deleting or replacing your
music, and anything yt-dlp, ffmpeg, Syncthing or the services you download
from do on their own.
