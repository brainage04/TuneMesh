# TuneMesh

A personal music player for a library that lives in one Syncthing folder,
shared over Tailscale between thomas-pc, thomas-laptop, thomas-server and
thomass-s21. Songs are added by asking the download worker (yt-dlp/spotDL on
thomas-server) for a URL or a search; the result syncs to every device and
plays offline.

| Directory | What |
| --- | --- |
| `linux/` | Rust workspace: the GTK 4 + libmpv desktop app (`tunemesh`) and the download worker (`tunemesh-worker`) |
| `android/` | Kotlin + Jetpack Compose + Media3 Android app |
| `docs/library-format.md` | The folder layout, playlist, request and status formats every component shares |
| `flake.nix` | Nix packages and dev shell for `linux/` |
