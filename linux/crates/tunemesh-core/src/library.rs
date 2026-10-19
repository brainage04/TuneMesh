//! Library scanning, track metadata with the contract's fallbacks, and the
//! album/artist index built from it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::search::{Query, fold};
use crate::{PLAYLISTS_DIR, TUNEMESH_DIR};

pub const AUDIO_EXTENSIONS: [&str; 10] =
    ["opus", "ogg", "oga", "m4a", "mp4", "aac", "mp3", "flac", "wav", "webm"];
const COVER_FILES: [&str; 4] = ["cover.jpg", "cover.png", "folder.jpg", "folder.png"];
pub const UNKNOWN_ALBUM: &str = "Unknown Album";
pub const UNKNOWN_ARTIST: &str = "Unknown Artist";

/// Whether a file name has an audio extension (case-insensitive).
pub fn has_audio_extension(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| AUDIO_EXTENSIONS.iter().any(|a| a.eq_ignore_ascii_case(ext)))
}

/// Whether a file name is a Syncthing conflict copy.
pub fn is_conflict_copy(name: &str) -> bool {
    name.contains(".sync-conflict-")
}

/// Whether a directory entry name is skipped while scanning (hidden files and
/// conflict copies).
fn is_ignored_name(name: &str) -> bool {
    name.starts_with('.') || is_conflict_copy(name)
}

/// Whether the root-relative path (with `/` separators) names a library track.
pub fn is_track_path(rel: &str) -> bool {
    let mut parts = rel.split('/').peekable();
    let Some(first) = parts.peek() else { return false };
    if *first == PLAYLISTS_DIR || *first == TUNEMESH_DIR {
        return false;
    }
    let mut last = "";
    for p in parts {
        if p.is_empty() || is_ignored_name(p) {
            return false;
        }
        last = p;
    }
    has_audio_extension(last)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverSource {
    /// A picture embedded in the audio file.
    Embedded,
    /// A root-relative image file next to the track.
    File(String),
    /// Nothing: the client draws a placeholder.
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// Identity: path relative to the root with `/` separators.
    pub path: String,
    pub title: String,
    /// Artist as tagged (all artists joined), or the directory fallback.
    pub artist: String,
    /// First tagged artist.
    pub first_artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: u64,
    pub cover: CoverSource,
    /// Modification time in seconds since the epoch.
    pub mtime: i64,
    pub size: u64,
}

impl Track {
    /// The artist the track is listed and its album grouped under: album
    /// artist, falling back to the first artist.
    pub fn listed_artist(&self) -> &str {
        self.album_artist.as_deref().unwrap_or(&self.first_artist)
    }

    pub fn duration_secs(&self) -> Option<u64> {
        (self.duration_ms > 0).then(|| (self.duration_ms + 500) / 1000)
    }
}

/// Title fallback: file name without extension and trailing ` [<id>]`.
pub fn title_from_file_name(name: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    };
    if let Some(inner) = stem.strip_suffix(']')
        && let Some(i) = inner.rfind('[')
        && i + 1 < inner.len()
        && !inner[i + 1..].contains(']')
    {
        let title = stem[..i].trim_end();
        if !title.is_empty() {
            return title.to_owned();
        }
    }
    stem.to_owned()
}

/// Extracts the source id from a file name ending in ` [<id>].<ext>`.
pub fn source_id_from_file_name(name: &str) -> Option<&str> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let inner = stem.strip_suffix(']')?;
    let i = inner.rfind(" [")?;
    let id = &inner[i + 2..];
    (!id.is_empty() && !id.contains(['[', ']'])).then_some(id)
}

/// Directory fallbacks for (album, artist) from the root-relative path.
pub fn dir_fallbacks(rel: &str) -> (String, String) {
    let parts: Vec<&str> = rel.split('/').collect();
    let n = parts.len();
    let album = if n >= 2 { parts[n - 2].to_owned() } else { UNKNOWN_ALBUM.to_owned() };
    let artist = if n >= 3 { parts[n - 3].to_owned() } else { UNKNOWN_ARTIST.to_owned() };
    (album, artist)
}

/// Tag values read from a file; `None`/empty fields fall back.
#[derive(Debug, Default, Clone)]
pub struct TagInfo {
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub year: Option<u32>,
    pub duration_ms: u64,
    pub has_picture: bool,
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn parse_year(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

fn tag_info(tag: &Tag) -> TagInfo {
    let mut artists: Vec<String> = Vec::new();
    for value in tag.get_strings(ItemKey::TrackArtist) {
        for part in value.split(['\0', ';']) {
            if let Some(a) = non_empty(Some(part)) {
                artists.push(a);
            }
        }
    }
    let year = [ItemKey::RecordingDate, ItemKey::Year, ItemKey::OriginalReleaseDate]
        .iter()
        .find_map(|k| tag.get_string(*k).and_then(parse_year));
    TagInfo {
        title: non_empty(tag.title().as_deref()),
        artists,
        album: non_empty(tag.album().as_deref()),
        album_artist: non_empty(tag.get_string(ItemKey::AlbumArtist)),
        track_number: tag.track(),
        disc_number: tag.disk(),
        year,
        duration_ms: 0,
        has_picture: !tag.pictures().is_empty(),
    }
}

/// Reads tags and duration; unreadable files yield an empty `TagInfo` so the
/// track still appears with its fallbacks.
pub fn read_tags(path: &Path) -> TagInfo {
    lofty::read_from_path(path).map_or_else(|_| TagInfo::default(), |f| tagged_info(&f))
}

/// [`read_tags`] for an already open file (see `fsx::Dir::open_file`).
pub fn read_tags_from(file: &mut fs::File) -> TagInfo {
    lofty::read_from(file).map_or_else(|_| TagInfo::default(), |f| tagged_info(&f))
}

fn tagged_info(file: &lofty::file::TaggedFile) -> TagInfo {
    let mut info = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .map(tag_info)
        .unwrap_or_default();
    if !info.has_picture {
        info.has_picture = file.tags().iter().any(|t| !t.pictures().is_empty());
    }
    info.duration_ms = file.properties().duration().as_millis() as u64;
    info
}

/// Reads the embedded front cover (or first picture) of an audio file.
pub fn read_embedded_cover(path: &Path) -> Option<Vec<u8>> {
    use lofty::picture::PictureType;
    let file = lofty::read_from_path(path).ok()?;
    let pictures: Vec<_> = file.tags().iter().flat_map(|t| t.pictures()).collect();
    pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|p| p.data().to_vec())
}

/// Builds a track from its tags plus the contract's fallbacks.
pub fn make_track(rel: &str, info: TagInfo, dir_cover: Option<&str>, mtime: i64, size: u64) -> Track {
    let file_name = rel.rsplit('/').next().unwrap_or(rel);
    let (fallback_album, fallback_artist) = dir_fallbacks(rel);
    let (artist, first_artist) = if info.artists.is_empty() {
        (fallback_artist.clone(), fallback_artist)
    } else {
        (info.artists.join(", "), info.artists[0].clone())
    };
    let cover = if info.has_picture {
        CoverSource::Embedded
    } else if let Some(c) = dir_cover {
        CoverSource::File(c.to_owned())
    } else {
        CoverSource::None
    };
    Track {
        path: rel.to_owned(),
        title: info.title.unwrap_or_else(|| title_from_file_name(file_name)),
        artist,
        first_artist,
        album: info.album.unwrap_or(fallback_album),
        album_artist: info.album_artist,
        track_number: info.track_number,
        disc_number: info.disc_number,
        year: info.year,
        duration_ms: info.duration_ms,
        cover,
        mtime,
        size,
    }
}

/// Scans `root`. Tracks in `previous` whose path, mtime and size are
/// unchanged are reused without reading the file again. Returns tracks sorted
/// by path.
pub fn scan(root: &Path, previous: &HashMap<String, Track>) -> Vec<Track> {
    let mut tracks = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        let mut audio = Vec::new();
        let mut dir_cover: Option<(usize, String)> = None;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if is_ignored_name(name) {
                continue;
            }
            let rel = if rel_dir.is_empty() { name.to_owned() } else { format!("{rel_dir}/{name}") };
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_dir() {
                if rel_dir.is_empty() && (name == PLAYLISTS_DIR || name == TUNEMESH_DIR) {
                    continue;
                }
                stack.push((entry.path(), rel));
                continue;
            }
            // Regular files and symlinks to regular files.
            let Ok(meta) = fs::metadata(entry.path()) else { continue };
            if !meta.is_file() {
                continue;
            }
            let lower = name.to_ascii_lowercase();
            if let Some(rank) = COVER_FILES.iter().position(|c| *c == lower) {
                if dir_cover.as_ref().is_none_or(|(r, _)| rank < *r) {
                    dir_cover = Some((rank, rel));
                }
                continue;
            }
            if has_audio_extension(name) {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                audio.push((entry.path(), rel, mtime, meta.len()));
            }
        }
        let dir_cover = dir_cover.map(|(_, rel)| rel);
        for (abs, rel, mtime, size) in audio {
            let track = match previous.get(&rel) {
                Some(old) if old.mtime == mtime && old.size == size => {
                    let mut t = old.clone();
                    if t.cover != CoverSource::Embedded {
                        t.cover = dir_cover.clone().map_or(CoverSource::None, CoverSource::File);
                    }
                    t
                }
                _ => make_track(&rel, read_tags(&abs), dir_cover.as_deref(), mtime, size),
            };
            tracks.push(track);
        }
    }
    tracks.sort_by(|a, b| a.path.cmp(&b.path));
    tracks
}

/// Maps source ids (`[<id>]` in file names) to the library path carrying
/// them; when several files carry one id, the lexically first path wins.
pub fn source_id_index(root: &Path) -> HashMap<String, String> {
    let mut index: HashMap<String, String> = HashMap::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if is_ignored_name(name) {
                continue;
            }
            let rel = if rel_dir.is_empty() { name.to_owned() } else { format!("{rel_dir}/{name}") };
            match entry.file_type() {
                Ok(t) if t.is_dir()
                    && !(rel_dir.is_empty() && (name == PLAYLISTS_DIR || name == TUNEMESH_DIR)) => {
                        stack.push((entry.path(), rel));
                    }
                Ok(t) if t.is_file() && has_audio_extension(name) => {
                    if let Some(id) = source_id_from_file_name(name) {
                        match index.get_mut(id) {
                            Some(existing) if *existing <= rel => {}
                            Some(existing) => *existing = rel,
                            None => {
                                index.insert(id.to_owned(), rel);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    index
}

#[derive(Debug, Clone)]
pub struct Album {
    pub name: String,
    pub artist: String,
    /// Track indices in disc/track order.
    pub tracks: Vec<usize>,
    pub year: Option<u32>,
    /// Newest track mtime ("recently added").
    pub added: i64,
    pub duration_ms: u64,
    folded: String,
}

impl Album {
    /// Stable key used in navigation.
    pub fn key(&self) -> String {
        album_key(&self.artist, &self.name)
    }
}

pub fn album_key(artist: &str, album: &str) -> String {
    format!("{artist}\u{1f}{album}")
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub name: String,
    pub tracks: Vec<usize>,
    /// Album indices, newest first.
    pub albums: Vec<usize>,
    folded: String,
}

/// All tracks plus derived album and artist indices.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    by_path: HashMap<String, usize>,
    album_by_key: HashMap<String, usize>,
    artist_by_name: HashMap<String, usize>,
    track_folded: Vec<String>,
    title_folded: Vec<String>,
}

pub struct SearchResults {
    pub tracks: Vec<usize>,
    pub albums: Vec<usize>,
    pub artists: Vec<usize>,
}

impl Library {
    pub fn new(tracks: Vec<Track>) -> Self {
        let by_path = tracks.iter().enumerate().map(|(i, t)| (t.path.clone(), i)).collect();

        let mut albums: Vec<Album> = Vec::new();
        let mut album_by_key: HashMap<String, usize> = HashMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let key = album_key(t.listed_artist(), &t.album);
            let idx = *album_by_key.entry(key).or_insert_with(|| {
                albums.push(Album {
                    name: t.album.clone(),
                    artist: t.listed_artist().to_owned(),
                    tracks: Vec::new(),
                    year: None,
                    added: 0,
                    duration_ms: 0,
                    folded: fold(&format!("{} {}", t.album, t.listed_artist())),
                });
                albums.len() - 1
            });
            let a = &mut albums[idx];
            a.tracks.push(i);
            a.added = a.added.max(t.mtime);
            a.duration_ms += t.duration_ms;
            a.year = a.year.or(t.year);
        }
        for a in &mut albums {
            a.tracks.sort_by(|&x, &y| {
                let (x, y) = (&tracks[x], &tracks[y]);
                (x.disc_number.unwrap_or(1), x.track_number.unwrap_or(u32::MAX), &x.title, &x.path).cmp(&(
                    y.disc_number.unwrap_or(1),
                    y.track_number.unwrap_or(u32::MAX),
                    &y.title,
                    &y.path,
                ))
            });
        }

        let mut artists: Vec<Artist> = Vec::new();
        let mut artist_by_name: HashMap<String, usize> = HashMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let name = t.listed_artist();
            let idx = *artist_by_name.entry(name.to_owned()).or_insert_with(|| {
                artists.push(Artist {
                    name: name.to_owned(),
                    tracks: Vec::new(),
                    albums: Vec::new(),
                    folded: fold(name),
                });
                artists.len() - 1
            });
            artists[idx].tracks.push(i);
        }
        for artist in &mut artists {
            let mut seen = Vec::new();
            for &ti in &artist.tracks {
                let t = &tracks[ti];
                let ai = album_by_key[&album_key(t.listed_artist(), &t.album)];
                if !seen.contains(&ai) {
                    seen.push(ai);
                }
            }
            seen.sort_by(|&x, &y| {
                albums[y].year.cmp(&albums[x].year).then_with(|| albums[x].name.cmp(&albums[y].name))
            });
            artist.albums = seen;
            // Artist track order: by album (as above), then album order.
            let order: HashMap<usize, usize> = artist.albums.iter().enumerate().map(|(p, &a)| (a, p)).collect();
            let album_pos = |ti: usize| {
                let t = &tracks[ti];
                let ai = album_by_key[&album_key(t.listed_artist(), &t.album)];
                let within = albums[ai].tracks.iter().position(|&x| x == ti).unwrap_or(0);
                (order[&ai], within)
            };
            artist.tracks.sort_by_key(|&ti| album_pos(ti));
        }
        artists.sort_by_key(|a| fold(&a.name));
        let artist_by_name = artists.iter().enumerate().map(|(i, a)| (a.name.clone(), i)).collect();

        let track_folded = tracks
            .iter()
            .map(|t| fold(&format!("{} {} {}", t.title, t.artist, t.album)))
            .collect();
        let title_folded = tracks.iter().map(|t| fold(&t.title)).collect();

        Self { tracks, albums, artists, by_path, album_by_key, artist_by_name, track_folded, title_folded }
    }

    pub fn track_index(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    pub fn track(&self, path: &str) -> Option<&Track> {
        self.track_index(path).map(|i| &self.tracks[i])
    }

    pub fn album_index(&self, key: &str) -> Option<usize> {
        self.album_by_key.get(key).copied()
    }

    pub fn album_of(&self, track: &Track) -> Option<usize> {
        self.album_index(&album_key(track.listed_artist(), &track.album))
    }

    pub fn artist_index(&self, name: &str) -> Option<usize> {
        self.artist_by_name.get(name).copied()
    }

    /// Albums ordered by newest track first.
    pub fn recently_added_albums(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.albums.len()).collect();
        v.sort_by(|&a, &b| self.albums[b].added.cmp(&self.albums[a].added));
        v
    }

    /// Albums sorted by name.
    pub fn albums_by_name(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.albums.len()).collect();
        v.sort_by_cached_key(|&a| (fold(&self.albums[a].name), fold(&self.albums[a].artist)));
        v
    }

    pub fn search(&self, query: &Query) -> SearchResults {
        if query.is_empty() {
            return SearchResults { tracks: vec![], albums: vec![], artists: vec![] };
        }
        let mut tracks: Vec<usize> =
            (0..self.tracks.len()).filter(|&i| query.matches(&self.track_folded[i])).collect();
        tracks.sort_by_key(|&i| query.rank(&self.title_folded[i]));
        let mut albums: Vec<usize> =
            (0..self.albums.len()).filter(|&i| query.matches(&self.albums[i].folded)).collect();
        albums.sort_by_key(|&i| query.rank(&fold(&self.albums[i].name)));
        let mut artists: Vec<usize> =
            (0..self.artists.len()).filter(|&i| query.matches(&self.artists[i].folded)).collect();
        artists.sort_by_key(|&i| query.rank(&self.artists[i].folded));
        SearchResults { tracks, albums, artists }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_fallback_strips_extension_and_id() {
        assert_eq!(title_from_file_name("Me at the zoo [jNQXAC9IVRw].opus"), "Me at the zoo");
        assert_eq!(title_from_file_name("Song.mp3"), "Song");
        assert_eq!(title_from_file_name("A [live] B.flac"), "A [live] B");
        assert_eq!(title_from_file_name("[only].mp3"), "[only]");
        assert_eq!(title_from_file_name("v1.2 mix.ogg"), "v1.2 mix");
        assert_eq!(title_from_file_name("Tight[abc].opus"), "Tight");
        assert_eq!(title_from_file_name("Spaced  [abc].opus"), "Spaced");
        assert_eq!(title_from_file_name("Nested [a[b]].opus"), "Nested [a[b]]");
    }

    #[test]
    fn source_id_parsing() {
        assert_eq!(source_id_from_file_name("T [abc_-1].opus"), Some("abc_-1"));
        assert_eq!(source_id_from_file_name("T.opus"), None);
        assert_eq!(source_id_from_file_name("T [].opus"), None);
    }

    #[test]
    fn directory_fallbacks() {
        assert_eq!(dir_fallbacks("a.mp3"), (UNKNOWN_ALBUM.into(), UNKNOWN_ARTIST.into()));
        assert_eq!(dir_fallbacks("Album/a.mp3"), ("Album".into(), UNKNOWN_ARTIST.into()));
        assert_eq!(dir_fallbacks("Artist/Album/a.mp3"), ("Album".into(), "Artist".into()));
        assert_eq!(dir_fallbacks("x/Artist/Album/a.mp3"), ("Album".into(), "Artist".into()));
    }

    #[test]
    fn make_track_uses_tags_then_fallbacks() {
        let t = make_track("Art/Alb/Song [id1].opus", TagInfo::default(), Some("Art/Alb/cover.jpg"), 5, 9);
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str()), ("Song", "Art", "Alb"));
        assert_eq!(t.cover, CoverSource::File("Art/Alb/cover.jpg".into()));
        assert_eq!(t.listed_artist(), "Art");

        let info = TagInfo {
            title: Some("Real".into()),
            artists: vec!["A1".into(), "A2".into()],
            album: Some("Rec".into()),
            has_picture: true,
            ..Default::default()
        };
        let t = make_track("x.mp3", info, None, 0, 0);
        assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str()), ("Real", "A1, A2", "Rec"));
        assert_eq!(t.listed_artist(), "A1");
        assert_eq!(t.cover, CoverSource::Embedded);
    }

    #[test]
    fn track_path_rules() {
        assert!(is_track_path("A/B/c.OPUS"));
        assert!(!is_track_path("Playlists/x.mp3"));
        assert!(!is_track_path(".tunemesh/requests/x.mp3"));
        assert!(!is_track_path("A/.hidden/c.mp3"));
        assert!(!is_track_path("A/c.sync-conflict-20260101-000000-ABC.mp3"));
        assert!(!is_track_path("A/c.txt"));
        assert!(is_track_path("Sub/Playlists/c.mp3"));
    }

    #[test]
    fn scan_applies_exclusions_and_dir_cover() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in [
            "Art/Alb/one.mp3",
            "Art/Alb/cover.jpg",
            "Art/Alb/notes.txt",
            "Art/Alb/one.sync-conflict-20260101-010101-AAAAAAA.mp3",
            "Playlists/p.mp3",
            ".tunemesh/requests/r.mp3",
            ".hidden/h.mp3",
            "root.flac",
        ] {
            let path = root.join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"not really audio").unwrap();
        }
        let tracks = scan(root, &HashMap::new());
        let paths: Vec<_> = tracks.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(paths, vec!["Art/Alb/one.mp3", "root.flac"]);
        assert_eq!(tracks[0].cover, CoverSource::File("Art/Alb/cover.jpg".into()));
        assert_eq!(tracks[1].album, UNKNOWN_ALBUM);
        assert_eq!(tracks[1].artist, UNKNOWN_ARTIST);

        assert!(source_id_index(root).is_empty());
        fs::write(root.join("Art/Alb/x [vid42].opus"), b"").unwrap();
        fs::write(root.join("Art/Alb/a [vid42].opus"), b"").unwrap();
        fs::write(root.join("Playlists/p [vid43].opus"), b"").unwrap();
        let index = source_id_index(root);
        assert_eq!(index.len(), 1);
        assert_eq!(index["vid42"], "Art/Alb/a [vid42].opus");
    }

    #[test]
    fn library_groups_albums_and_artists() {
        let mk = |path: &str, album_artist: Option<&str>, artists: &[&str], album: &str, n: u32| {
            let info = TagInfo {
                title: Some(format!("T{n}")),
                artists: artists.iter().map(|s| s.to_string()).collect(),
                album: Some(album.into()),
                album_artist: album_artist.map(str::to_owned),
                track_number: Some(n),
                ..Default::default()
            };
            make_track(path, info, None, n as i64, 0)
        };
        let lib = Library::new(vec![
            mk("a/2.mp3", Some("VA"), &["X"], "Comp", 2),
            mk("a/1.mp3", Some("VA"), &["Y"], "Comp", 1),
            mk("b/1.mp3", None, &["Z", "W"], "Solo", 1),
        ]);
        assert_eq!(lib.albums.len(), 2);
        let comp = &lib.albums[lib.album_index(&album_key("VA", "Comp")).unwrap()];
        let titles: Vec<_> = comp.tracks.iter().map(|&i| lib.tracks[i].title.as_str()).collect();
        assert_eq!(titles, vec!["T1", "T2"]);
        let names: Vec<_> = lib.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["VA", "Z"]);
        let r = lib.search(&Query::new("w solo"));
        assert_eq!(r.tracks.len(), 1);
    }
}
