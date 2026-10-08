//! `Playlists/<Name>.m3u8` (contract: "Playlists").

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::library::{is_conflict_copy, title_from_file_name};
use crate::{PLAYLISTS_DIR, atomic, sanitize};

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
        Self { track: Some(track.to_owned()), raw: format!("../{track}"), duration: None, label: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub entries: Vec<PlaylistEntry>,
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

/// Parses playlist text. Tolerates a BOM and CRLF line endings.
pub fn parse(text: &str, root: &Path) -> Vec<PlaylistEntry> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entries = Vec::new();
    let mut pending: Option<(Option<u64>, Option<String>)> = None;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            let (dur, label) = info.split_once(',').unwrap_or((info, ""));
            let dur = dur.trim().parse::<i64>().ok().filter(|d| *d >= 0).map(|d| d as u64);
            let label = Some(label.trim().to_owned()).filter(|l| !l.is_empty());
            pending = Some((dur, label));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (duration, label) = pending.take().unwrap_or((None, None));
        entries.push(PlaylistEntry { track: resolve(line, root), raw: line.to_owned(), duration, label });
    }
    entries
}

/// Serialises entries: `#EXTM3U`, then `#EXTINF` + path per entry, LF endings.
pub fn serialize(entries: &[PlaylistEntry], info: &dyn Fn(&str) -> Option<TrackInfo>) -> String {
    let mut out = String::from("#EXTM3U\n");
    for e in entries {
        let known = e.track.as_deref().and_then(info);
        let duration = known.as_ref().and_then(|k| k.duration_secs).or(e.duration);
        let label = known.map(|k| k.label).or_else(|| e.label.clone()).unwrap_or_else(|| {
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
    playlists_dir(root).join(format!("{name}{EXT}"))
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

/// Names of all playlists, sorted case-insensitively, Liked Songs excluded.
pub fn list_names(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(playlists_dir(root)) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if !e.file_type().ok()?.is_file() {
                return None;
            }
            playlist_stem(&name).filter(|stem| *stem != LIKED_SONGS).map(str::to_owned)
        })
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
    fs::read_dir(playlists_dir(root))
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .find_map(|n| playlist_stem(&n).filter(|stem| stem.to_lowercase() == folded).map(str::to_owned))
}

impl Playlist {
    /// Reads a playlist; a missing Liked Songs file reads as empty.
    pub fn read(root: &Path, name: &str) -> io::Result<Self> {
        match fs::read(playlist_path(root, name)) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes);
                Ok(Self { name: name.to_owned(), entries: parse(&text, root) })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && name == LIKED_SONGS => {
                Ok(Self { name: name.to_owned(), entries: Vec::new() })
            }
            Err(e) => Err(e),
        }
    }

    pub fn write(&self, root: &Path, info: &dyn Fn(&str) -> Option<TrackInfo>) -> io::Result<()> {
        atomic::write(&playlist_path(root, &self.name), serialize(&self.entries, info).as_bytes())
    }

    /// Creates an empty playlist with the sanitised `name`; fails if a
    /// playlist with that name, in any letter case, exists.
    pub fn create(root: &Path, name: &str) -> io::Result<Self> {
        let name = sanitize_name(name);
        if let Some(existing) = existing_name(root, &name) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("“{existing}” already exists")));
        }
        let p = Self { name, entries: Vec::new() };
        p.write(root, &|_| None)?;
        Ok(p)
    }

    /// Renames a playlist file; returns the sanitised new name. Changing only
    /// the letter case of a name is allowed.
    pub fn rename(root: &Path, old: &str, new: &str) -> io::Result<String> {
        if old == LIKED_SONGS {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Liked Songs cannot be renamed"));
        }
        let new = sanitize_name(new);
        if new == old {
            return Ok(new);
        }
        if let Some(existing) = existing_name(root, &new).filter(|e| e != old) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("“{existing}” already exists")));
        }
        fs::rename(playlist_path(root, old), playlist_path(root, &new))?;
        Ok(new)
    }

    pub fn delete(root: &Path, name: &str) -> io::Result<()> {
        if name == LIKED_SONGS {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "Liked Songs cannot be deleted"));
        }
        fs::remove_file(playlist_path(root, name))
    }

    pub fn contains(&self, track: &str) -> bool {
        self.entries.iter().any(|e| e.track.as_deref() == Some(track))
    }

    /// Liked Songs: liking inserts at the top (no-op when already liked).
    pub fn like(&mut self, track: &str) {
        if !self.contains(track) {
            self.entries.insert(0, PlaylistEntry::for_track(track));
        }
    }

    /// Liked Songs: unliking removes every entry for the track.
    pub fn unlike(&mut self, track: &str) {
        self.entries.retain(|e| e.track.as_deref() != Some(track));
    }

    pub fn append<'a>(&mut self, tracks: impl IntoIterator<Item = &'a str>) {
        self.entries.extend(tracks.into_iter().map(PlaylistEntry::for_track));
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

    fn info(path: &str) -> Option<TrackInfo> {
        (path != "Gone/Missing.mp3").then(|| TrackInfo { duration_secs: Some(213), label: format!("A - {path}") })
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
        assert_eq!(again.iter().map(|e| e.track.clone()).collect::<Vec<_>>(), tracks.iter().map(|t| t.map(str::to_owned)).collect::<Vec<_>>());
        assert_eq!(serialize(&again, &info), out);
    }

    #[test]
    fn liked_songs_insert_at_top_and_unlike_removes_all() {
        let mut p = Playlist { name: LIKED_SONGS.into(), entries: vec![] };
        p.like("a.mp3");
        p.like("b.mp3");
        p.like("a.mp3");
        assert_eq!(p.entries.iter().map(|e| e.track.as_deref().unwrap()).collect::<Vec<_>>(), vec!["b.mp3", "a.mp3"]);
        p.append(["c.mp3", "b.mp3"]);
        p.unlike("b.mp3");
        assert_eq!(p.entries.iter().map(|e| e.track.as_deref().unwrap()).collect::<Vec<_>>(), vec!["a.mp3", "c.mp3"]);
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
        assert_eq!(String::from_utf8(bytes).unwrap(), "#EXTM3U\n#EXTINF:-1,c\n../Ä/B/c.opus\n#EXTINF:-1,c\n../Ä/B/c.opus\n");
        fs::write(playlists_dir(root).join("Road_ Trip_.sync-conflict-20260101-000000-X.m3u8"), "").unwrap();
        fs::write(playlist_path(root, LIKED_SONGS), "#EXTM3U\n").unwrap();
        assert_eq!(list_names(root), vec!["Road_ Trip_"]);
        assert_eq!(Playlist::rename(root, "Road_ Trip_", "Drive").unwrap(), "Drive");
        assert!(Playlist::rename(root, LIKED_SONGS, "x").is_err());
        assert!(Playlist::delete(root, LIKED_SONGS).is_err());
        Playlist::delete(root, "Drive").unwrap();
        assert!(list_names(root).is_empty());
        assert!(Playlist::read(root, LIKED_SONGS).unwrap().entries.is_empty());
    }

    #[test]
    fn names_never_differ_only_in_case() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Playlist::create(root, "Road Trip").unwrap();
        assert_eq!(existing_name(root, "road TRIP").as_deref(), Some("Road Trip"));
        assert_eq!(existing_name(root, "Drive"), None);
        // Liked Songs is reserved even before its file exists.
        assert_eq!(existing_name(root, "liked songs").as_deref(), Some(LIKED_SONGS));
        assert_eq!(Playlist::create(root, "road trip").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(Playlist::create(root, "LIKED SONGS").unwrap_err().kind(), io::ErrorKind::AlreadyExists);

        Playlist::create(root, "Drive").unwrap();
        assert_eq!(Playlist::rename(root, "Drive", "ROAD trip").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(Playlist::rename(root, "Drive", "liked songs").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        // Changing only the case of its own name is a rename, not a clash.
        assert_eq!(Playlist::rename(root, "Road Trip", "road trip").unwrap(), "road trip");
        assert_eq!(list_names(root), vec!["Drive", "road trip"]);
    }

    #[test]
    fn remove_and_move_by_position() {
        let mut p = Playlist { name: "x".into(), entries: vec![] };
        p.append(["a", "b", "a", "c"]);
        p.remove_positions(&[2]);
        p.move_entry(2, 0);
        assert_eq!(p.entries.iter().map(|e| e.track.as_deref().unwrap()).collect::<Vec<_>>(), vec!["c", "a", "b"]);
    }
}
