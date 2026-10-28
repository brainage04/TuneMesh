# Conflict-copy pass test cases

Shared cases for merging several conflict copies of one playlist in passes
(`docs/library-format.md`, "Playlist conflict copies", How). The cases in
`../playlist-merge/` are single three-way merges; these check what a pass
does with the copies, the playlist and the base. Each directory is one case
for the playlist `Mix`:

| File | What it is |
| --- | --- |
| `base.m3u8` | The merging device's base before the first pass |
| `Mix.m3u8` | `Playlists/Mix.m3u8` before the first pass |
| `pass-<n>/` | Copies put into `Playlists/` before pass n (replacing a file of the same name), all due |
| `pass-<n>-expected/Mix.m3u8` | The playlist's bytes after pass n |
| `pass-<n>-expected/base.m3u8` | The base's bytes after pass n |
| `pass-<n>-expected/left.txt` | The copies still in `Playlists/` after pass n, one name per line in byte order |

- `skipped-copy`: base and playlist `A`; copies 0, 1 and 2 add H, J and I,
  and copy 1 has a line over 32 KiB, so it cannot be read. Pass 1 merges
  copies 0 and 2, leaves copy 1, and keeps the base. Before pass 2 copy 1
  becomes readable; pass 2 merges it against the same base, so H and I,
  which it does not have, are not taken for its removals, and the result
  becomes the base.
