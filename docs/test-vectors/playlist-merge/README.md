# Playlist merge test cases

Shared cases for the conflict-copy merge in `docs/library-format.md`
("Playlist conflict copies"). Each directory is one case:

| File | What it is |
| --- | --- |
| `base.m3u8` | The merging device's base (missing: no base kept) |
| `winner.m3u8` | `Playlists/<Name>.m3u8` as Syncthing kept it |
| `copy.m3u8` | The `*.sync-conflict-*` copy |
| `merged.m3u8` | The bytes every implementation must write |

Paths are relative to `Playlists/`, as in a real playlist; the cases do not
depend on the library root or on which tracks exist. Every implementation
parses the three inputs with its playlist reader, merges, writes the result
in the writers' format and compares it byte for byte with `merged.m3u8`.

- `liked-unlike-and-like`: Liked Songs; one device unliked B, the other
  liked D (at the top).
- `remove-and-reorder`, `reorder-in-copy`: one device removed B, the other
  moved D to the top; once with each as the winner (same result).
- `both-add`: each device added a different track.
- `both-add-same`: both devices added the same track; it is there once.
- `no-base`: no base; B, which the winner's device had removed, comes back.
- `duplicates`: one device removed one of two A entries, the other added a
  third.
- `both-reorder`: both devices changed the order; the winner's order wins.
- `labels-and-outside`: entries keep their own `#EXTINF` data; an entry
  without one gets its file name's title; a line outside the root is kept
  as written.
