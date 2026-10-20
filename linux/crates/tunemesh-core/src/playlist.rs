//! `Playlists/<Name>.m3u8` (contract: "Playlists").

use std::io;
use std::path::{Path, PathBuf};

use crate::fsx::{Dir, Kind};
use crate::library::{has_line_break, is_conflict_copy, title_from_file_name};
use crate::{PLAYLISTS_DIR, sanitize};

pub const LIKED_SONGS: &str = "Liked Songs";
const EXT: &str = ".m3u8";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistEntry {
    /// Root-relative track path; `None` when the line points outside the root.
    pub track: Option<String>,
    /// The path line as read, written back verbatim when `track` is `None`.
    pub raw: String,
    /// `#EXTINF` duration in seconds, when known.
    pub duration: Option<u64>,
    /// `#EXTINF` display text.
    pub label: Option<String>,
}

impl PlaylistEntry {
    pub fn for_track(track: &str) -> Self {
        Self {
            track: Some(track.to_owned()),
            raw: format!("../{track}"),
            duration: None,
            label: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
}

/// A playlist's size and first tracks, read without holding its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub name: String,
    /// Number of entries, as `parse` would produce.
    pub count: usize,
    /// Root-relative paths of the first entries that resolve inside the
    /// root, in order.
    pub first_tracks: Vec<String>,
}

/// What writers need to know about a track for its `#EXTINF` line.
pub struct TrackInfo {
    pub duration_secs: Option<u64>,
    /// `Artist - Title`.
    pub label: String,
}

/// Resolves a playlist line to a root-relative path.
fn resolve(line: &str, root: &Path) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    let rest = if line.starts_with('/') {
        let root_str = root.to_str()?.trim_end_matches('/');

        line.strip_prefix(root_str)?.strip_prefix('/')?
    } else {
        parts.push(PLAYLISTS_DIR);
        line
    };
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The non-blank lines of playlist text, BOM and CR removed.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
}

/// Parses playlist text. Tolerates a BOM and CRLF line endings.
pub fn parse(text: &str, root: &Path) -> Vec<PlaylistEntry> {
    let mut entries = Vec::new();
    let mut pending: Option<(Option<u64>, Option<String>)> = None;
    for line in lines(text) {
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            let (dur, label) = info.split_once(',').unwrap_or((info, ""));
            let dur = dur
                .trim()
                .parse::<i64>()
                .ok()
                .filter(|d| *d >= 0)
                .map(|d| d as u64);
            let label = Some(label.trim().to_owned()).filter(|l| !l.is_empty());
            pending = Some((dur, label));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (duration, label) = pending.take().unwrap_or((None, None));
        entries.push(PlaylistEntry {
            track: resolve(line, root),
            raw: line.to_owned(),
            duration,
            label,
        });
    }
    entries
}

/// Serialises entries: `#EXTM3U`, then `#EXTINF` + path per entry, LF endings.
pub fn serialize(entries: &[PlaylistEntry], info: &dyn Fn(&str) -> Option<TrackInfo>) -> String {
    let mut out = String::from("#EXTM3U\n");
    // An entry with a line break would turn into two lines.
    for e in entries
        .iter()
        .filter(|e| !has_line_break(&e.raw) && !e.track.as_deref().is_some_and(has_line_break))
    {
        let known = e.track.as_deref().and_then(info);
        let duration = known.as_ref().and_then(|k| k.duration_secs).or(e.duration);
        let label = known
            .map(|k| k.label)
            .or_else(|| e.label.clone())
            .unwrap_or_else(|| {
                let path = e.track.as_deref().unwrap_or(&e.raw);
                title_from_file_name(path.rsplit('/').next().unwrap_or(path))
            });
        let label = label.replace(['\n', '\r'], " ");
        match duration {
            Some(d) => out.push_str(&format!("#EXTINF:{d},{label}\n")),
            None => out.push_str(&format!("#EXTINF:-1,{label}\n")),
        }
        match &e.track {
            Some(t) => {
                out.push_str("../");
                out.push_str(t);
            }
            None => out.push_str(&e.raw),
        }
        out.push('\n');
    }
    out
}

pub fn playlists_dir(root: &Path) -> PathBuf {
    root.join(PLAYLISTS_DIR)
}

pub fn playlist_path(root: &Path, name: &str) -> PathBuf {
    playlists_dir(root).join(file_name(name))
}

fn file_name(name: &str) -> String {
    format!("{name}{EXT}")
}

/// Largest playlist file read (a 100,000-entry playlist is about 15 MB).
const MAX_PLAYLIST_BYTES: u64 = 32 * 1024 * 1024;

/// The `Playlists` directory, refusing a symlink (see `fsx`).
fn open_dir(root: &Path, create: bool) -> io::Result<Dir> {
    Dir::open(root)?.sub(PLAYLISTS_DIR, create)
}

/// The file-system-safe form of a playlist name.
pub fn sanitize_name(name: &str) -> String {
    let file = sanitize::file_name(name, EXT);
    file[..file.len() - EXT.len()].to_owned()
}

/// Whether a directory entry name is a playlist file readers use.
fn playlist_stem(name: &str) -> Option<&str> {
    if name.starts_with('.') || is_conflict_copy(name) {
        return None;
    }
    name.strip_suffix(EXT).filter(|stem| !stem.is_empty())
}

/// Stems of the regular playlist files (symlinks and directories skipped).
fn stems(root: &Path) -> Vec<String> {
    let Ok(entries) = open_dir(root, false).and_then(|d| d.entries()) else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::File)
        .filter_map(|(name, _)| playlist_stem(&name).map(str::to_owned))
        .collect()
}

/// Names of all playlists, sorted case-insensitively, Liked Songs excluded.
pub fn list_names(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = stems(root)
        .into_iter()
        .filter(|stem| stem != LIKED_SONGS)
        .collect();
    names.sort_by_cached_key(|n| crate::search::fold(n));
    names
}

/// The playlist whose name equals `name` ignoring letter case (Liked Songs
/// included, whether or not its file exists yet). Android's shared storage is
/// case-insensitive, so such a playlist is the same file there.
pub fn existing_name(root: &Path, name: &str) -> Option<String> {
    let folded = name.to_lowercase();
    if folded == LIKED_SONGS.to_lowercase() {
        return Some(LIKED_SONGS.to_owned());
    }
    stems(root)
        .into_iter()
        .find(|stem| stem.to_lowercase() == folded)
}

/// A playlist file's bytes (empty for a missing Liked Songs), refusing a
/// symlink and files over `MAX_PLAYLIST_BYTES`.
fn read_bytes(root: &Path, name: &str) -> io::Result<Vec<u8>> {
    match open_dir(root, false).and_then(|d| d.read(&file_name(name), MAX_PLAYLIST_BYTES)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound && name == LIKED_SONGS => Ok(Vec::new()),
        r => r,
    }
}

impl Playlist {
    /// Reads a playlist; a missing Liked Songs file reads as empty. A
    /// playlist that is a symlink (or below a symlinked `Playlists`) is
    /// refused, so a synced link cannot leak the file it points to.
    pub fn read(root: &Path, name: &str) -> io::Result<Self> {
        let bytes = read_bytes(root, name)?;
        Ok(Self {
            name: name.to_owned(),
            entries: parse(&String::from_utf8_lossy(&bytes), root),
        })
    }

    /// The entry count and the first `first` resolvable tracks, read with
    /// `read`'s rules but without building the entries: memory stays at
    /// the file's bytes however many entries it has.
    pub fn summary(root: &Path, name: &str, first: usize) -> io::Result<Summary> {
        let bytes = read_bytes(root, name)?;
        let text = String::from_utf8_lossy(&bytes);
        let mut count = 0;
        let mut first_tracks = Vec::new();
        for line in lines(&text).filter(|l| !l.starts_with('#')) {
            count += 1;
            if first_tracks.len() < first
                && let Some(track) = resolve(line, root)
            {
                first_tracks.push(track);
            }
        }
        Ok(Summary {
            name: name.to_owned(),
            count,
            first_tracks,
        })
    }

    pub fn write(&self, root: &Path, info: &dyn Fn(&str) -> Option<TrackInfo>) -> io::Result<()> {
        open_dir(root, true)?.write_atomic(
            &file_name(&self.name),
            serialize(&self.entries, info).as_bytes(),
        )
    }

    /// Creates an empty playlist with the sanitised `name`; fails if a
    /// playlist with that name, in any letter case, exists.
    pub fn create(root: &Path, name: &str) -> io::Result<Self> {
        let name = sanitize_name(name);
        if let Some(existing) = existing_name(root, &name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("“{existing}” already exists"),
            ));
        }
        let p = Self {
            name,
            entries: Vec::new(),
        };
        p.write(root, &|_| None)?;
        Ok(p)
    }

    /// Renames a playlist file; returns the sanitised new name. Changing only
    /// the letter case of a name is allowed.
    pub fn rename(root: &Path, old: &str, new: &str) -> io::Result<String> {
        if old == LIKED_SONGS {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Liked Songs cannot be renamed",
            ));
        }
        let new = sanitize_name(new);
        if new == old {
            return Ok(new);
        }
        if let Some(existing) = existing_name(root, &new).filter(|e| e != old) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("“{existing}” already exists"),
            ));
        }
        open_dir(root, false)?.rename(&file_name(old), &file_name(&new))?;
        Ok(new)
    }

    pub fn delete(root: &Path, name: &str) -> io::Result<()> {
        if name == LIKED_SONGS {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Liked Songs cannot be deleted",
            ));
        }
        open_dir(root, false)?.remove_file(&file_name(name))
    }

    pub fn contains(&self, track: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.track.as_deref() == Some(track))
    }

    /// Liked Songs: liking inserts at the top (no-op when already liked).
    /// Tracks whose path has a line break are skipped (contract: they are
    /// never library tracks, and no playlist line can hold them).
    pub fn like(&mut self, track: &str) {
        if !has_line_break(track) && !self.contains(track) {
            self.entries.insert(0, PlaylistEntry::for_track(track));
        }
    }

    /// Liked Songs: unliking removes every entry for the track.
    pub fn unlike(&mut self, track: &str) {
        self.entries.retain(|e| e.track.as_deref() != Some(track));
    }

    pub fn append<'a>(&mut self, tracks: impl IntoIterator<Item = &'a str>) {
        self.entries.extend(
            tracks
                .into_iter()
                .filter(|t| !has_line_break(t))
                .map(PlaylistEntry::for_track),
        );
    }

    /// Removes the entries at `positions` (any order, duplicates ignored).
    pub fn remove_positions(&mut self, positions: &[usize]) {
        let mut i = 0;
        self.entries.retain(|_| {
            let keep = !positions.contains(&i);
            i += 1;
            keep
        });
    }

    /// Moves the entry at `from` so it ends up at index `to`.
    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from < self.entries.len() && to < self.entries.len() && from != to {
            let e = self.entries.remove(from);
            self.entries.insert(to, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    /// Audit finding 1: a synced symlink must not turn a secret file into
    /// playlist entries, and writing must replace the link, not the target.
    #[test]
    fn symlinked_playlists_are_refused() {
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("id_ed25519");
        fs::write(&secret, "-----BEGIN KEY-----\nAAAA\n").unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        fs::create_dir(playlists_dir(root)).unwrap();
        symlink(&secret, playlist_path(root, LIKED_SONGS)).unwrap();
        symlink(&secret, playlist_path(root, "x")).unwrap();
        assert!(Playlist::read(root, LIKED_SONGS).is_err());
        assert!(Playlist::read(root, "x").is_err());
        // Symlinks are not playlists, so a request naming "X" gets a new file.
        assert_eq!(existing_name(root, "X"), None);
        assert!(list_names(root).is_empty());

        // A symlinked Playlists directory is refused for reads and writes.
        fs::remove_dir_all(playlists_dir(root)).unwrap();
        fs::write(outside.path().join("y.m3u8"), "#EXTM3U\n../a.mp3\n").unwrap();
        symlink(outside.path(), playlists_dir(root)).unwrap();
        assert!(Playlist::read(root, "y").is_err());
        assert!(list_names(root).is_empty());
        let p = Playlist {
            name: "z".into(),
            entries: vec![],
        };
        assert!(p.write(root, &|_| None).is_err());
        assert!(!outside.path().join("z.m3u8").exists());
        assert!(Playlist::delete(root, "y").is_err());
        assert!(outside.path().join("y.m3u8").exists());
        assert_eq!(
            fs::read_to_string(&secret).unwrap(),
            "-----BEGIN KEY-----\nAAAA\n"
        );
    }

    fn info(path: &str) -> Option<TrackInfo> {
        (path != "Gone/Missing.mp3").then(|| TrackInfo {
            duration_secs: Some(213),
            label: format!("A - {path}"),
        })
    }

    #[test]
    fn round_trip_with_missing_and_unicode_entries() {
        let root = Path::new("/home/u/Music/TuneMesh");
        let text = "\u{feff}#EXTM3U\r\n#EXTINF:100,Old label\r\n../Björk/Homogenic/Jóga [abc].opus\r\n\r\n# comment\n\
                    ../Gone/Missing.mp3\n/home/u/Music/TuneMesh/Abs/Track.flac\n/elsewhere/x.mp3\n#EXTINF:7,Esc\n../../outside.mp3\n";
        let entries = parse(text, root);
        let tracks: Vec<_> = entries.iter().map(|e| e.track.as_deref()).collect();
        assert_eq!(
            tracks,
            vec![
                Some("Björk/Homogenic/Jóga [abc].opus"),
                Some("Gone/Missing.mp3"),
                Some("Abs/Track.flac"),
                None,
                None
            ]
        );
        assert_eq!(entries[0].duration, Some(100));
        assert_eq!(entries[0].label.as_deref(), Some("Old label"));

        let out = serialize(&entries, &info);
        assert_eq!(
            out,
            "#EXTM3U\n\
             #EXTINF:213,A - Björk/Homogenic/Jóga [abc].opus\n../Björk/Homogenic/Jóga [abc].opus\n\
             #EXTINF:-1,Missing\n../Gone/Missing.mp3\n\
             #EXTINF:213,A - Abs/Track.flac\n../Abs/Track.flac\n\
             #EXTINF:-1,x\n/elsewhere/x.mp3\n\
             #EXTINF:7,Esc\n../../outside.mp3\n"
        );
        // Reading what we wrote gives the same entries (missing ones kept).
        let again = parse(&out, root);
        assert_eq!(again.len(), 5);
        assert_eq!(
            again.iter().map(|e| e.track.clone()).collect::<Vec<_>>(),
            tracks
                .iter()
                .map(|t| t.map(str::to_owned))
                .collect::<Vec<_>>()
        );
        assert_eq!(serialize(&again, &info), out);
    }

    #[test]
    fn liked_songs_insert_at_top_and_unlike_removes_all() {
        let mut p = Playlist {
            name: LIKED_SONGS.into(),
            entries: vec![],
        };
        p.like("a.mp3");
        p.like("b.mp3");
        p.like("a.mp3");
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["b.mp3", "a.mp3"]
        );
        p.append(["c.mp3", "b.mp3"]);
        p.unlike("b.mp3");
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["a.mp3", "c.mp3"]
        );
    }

    #[test]
    fn file_operations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = Playlist::create(root, "Road: Trip?").unwrap();
        assert_eq!(p.name, "Road_ Trip_");
        assert!(Playlist::create(root, "Road: Trip?").is_err());
        let mut p = Playlist::read(root, "Road_ Trip_").unwrap();
        p.append(["Ä/B/c.opus", "Ä/B/c.opus"]);
        p.write(root, &|_| None).unwrap();
        let bytes = fs::read(playlist_path(root, "Road_ Trip_")).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "#EXTM3U\n#EXTINF:-1,c\n../Ä/B/c.opus\n#EXTINF:-1,c\n../Ä/B/c.opus\n"
        );
        fs::write(
            playlists_dir(root).join("Road_ Trip_.sync-conflict-20260101-000000-X.m3u8"),
            "",
        )
        .unwrap();
        fs::write(playlist_path(root, LIKED_SONGS), "#EXTM3U\n").unwrap();
        assert_eq!(list_names(root), vec!["Road_ Trip_"]);
        assert_eq!(
            Playlist::rename(root, "Road_ Trip_", "Drive").unwrap(),
            "Drive"
        );
        assert!(Playlist::rename(root, LIKED_SONGS, "x").is_err());
        assert!(Playlist::delete(root, LIKED_SONGS).is_err());
        Playlist::delete(root, "Drive").unwrap();
        assert!(list_names(root).is_empty());
        assert!(
            Playlist::read(root, LIKED_SONGS)
                .unwrap()
                .entries
                .is_empty()
        );
    }

    #[test]
    fn names_never_differ_only_in_case() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Playlist::create(root, "Road Trip").unwrap();
        assert_eq!(
            existing_name(root, "road TRIP").as_deref(),
            Some("Road Trip")
        );
        assert_eq!(existing_name(root, "Drive"), None);
        // Liked Songs is reserved even before its file exists.
        assert_eq!(
            existing_name(root, "liked songs").as_deref(),
            Some(LIKED_SONGS)
        );
        assert_eq!(
            Playlist::create(root, "road trip").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            Playlist::create(root, "LIKED SONGS").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );

        Playlist::create(root, "Drive").unwrap();
        assert_eq!(
            Playlist::rename(root, "Drive", "ROAD trip")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            Playlist::rename(root, "Drive", "liked songs")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        // Changing only the case of its own name is a rename, not a clash.
        assert_eq!(
            Playlist::rename(root, "Road Trip", "road trip").unwrap(),
            "road trip"
        );
        assert_eq!(list_names(root), vec!["Drive", "road trip"]);
    }

    /// Review item 12: a path with a line break never corrupts a playlist.
    #[test]
    fn line_breaks_never_reach_a_playlist() {
        let mut p = Playlist {
            name: "x".into(),
            entries: vec![],
        };
        p.append(["A/B/one\ntwo.mp3", "A/B/ok.mp3", "A/B/cr\r.mp3"]);
        p.like("L/x\ny.mp3");
        assert_eq!(p.entries.len(), 1);
        p.entries.push(PlaylistEntry::for_track("A/B/sneak\ny.mp3"));
        let out = serialize(&p.entries, &|_| None);
        assert_eq!(out, "#EXTM3U\n#EXTINF:-1,ok\n../A/B/ok.mp3\n");
    }

    #[test]
    fn remove_and_move_by_position() {
        let mut p = Playlist {
            name: "x".into(),
            entries: vec![],
        };
        p.append(["a", "b", "a", "c"]);
        p.remove_positions(&[2]);
        p.move_entry(2, 0);
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
    }

    /// `summary` agrees with `parse` on count and first resolvable tracks,
    /// and follows `read`'s rules.
    #[test]
    fn summary_matches_parse() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        let text = format!(
            "\u{feff}#EXTM3U\r\n#EXTINF:12,A - B\r\n../A/b.mp3\r\n# comment\r\n\r\n   \n\
             ../../outside.mp3\n/etc/passwd\n{}/C/d.opus\n#EXTINF:-1,x\nsub/e.flac\n../A/b.mp3\n../F/g.mp3\n",
            root.display()
        );
        fs::write(root.join("Playlists/Mix.m3u8"), &text).unwrap();
        let parsed = parse(&text, root);
        let resolved: Vec<String> = parsed.iter().filter_map(|e| e.track.clone()).collect();
        for first in [0, 2, 3, 100] {
            let s = Playlist::summary(root, "Mix", first).unwrap();
            assert_eq!(s.count, parsed.len());
            assert_eq!(s.first_tracks, resolved[..first.min(resolved.len())]);
        }
        assert_eq!(parsed.len(), 7);
        assert_eq!(&resolved[..2], ["A/b.mp3", "C/d.opus"]);
        // Same rules as `read`: missing Liked Songs is empty, others fail,
        // symlinks are refused, lossy UTF-8.
        fs::write(root.join("Playlists/Bad.m3u8"), b"../a/\xff.mp3\n").unwrap();
        let bad = Playlist::summary(root, "Bad", 5).unwrap();
        assert_eq!(
            bad.first_tracks,
            [Playlist::read(root, "Bad").unwrap().entries[0]
                .track
                .clone()
                .unwrap()]
        );
        let liked = Playlist::summary(root, LIKED_SONGS, 5).unwrap();
        assert_eq!((liked.count, liked.first_tracks.len()), (0, 0));
        assert!(Playlist::summary(root, "Nope", 5).is_err());
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("s.m3u8"), "../x.mp3\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("s.m3u8"),
            root.join("Playlists/Link.m3u8"),
        )
        .unwrap();
        assert!(Playlist::summary(root, "Link", 5).is_err());
    }
}
