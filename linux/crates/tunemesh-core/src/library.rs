//! Library scanning, track metadata with the contract's fallbacks, and the
//! album/artist index built from it.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::fsx::{Dir, Kind};
use crate::limits::{
    self, MAX_ARTISTS, MAX_COVER_BYTES, MAX_DEPTH, MAX_DIR_ENTRIES, MAX_TAG_ITEM_BYTES,
    MAX_TAG_TEXT_BYTES, MAX_TRACKS,
};
use crate::search::{Query, fold};
use crate::{PLAYLISTS_DIR, TUNEMESH_DIR};

pub const AUDIO_EXTENSIONS: [&str; 10] = [
    "opus", "ogg", "oga", "m4a", "mp4", "aac", "mp3", "flac", "wav", "webm",
];
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

/// Whether a name contains a line break, which no playlist line can hold.
pub fn has_line_break(name: &str) -> bool {
    name.contains(['\n', '\r'])
}

/// Whether a directory entry name is skipped while scanning (hidden files,
/// conflict copies, names with line breaks).
fn is_ignored_name(name: &str) -> bool {
    name.starts_with('.') || is_conflict_copy(name) || has_line_break(name)
}

/// Whether the root-relative path (with `/` separators) names a library track.
pub fn is_track_path(rel: &str) -> bool {
    let mut parts = rel.split('/').peekable();
    let Some(first) = parts.peek() else {
        return false;
    };
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
    /// Nanoseconds within `mtime`'s second: a file retagged within the same
    /// second (same size) must still count as changed. Caches written
    /// before this field existed do not parse, so they are rebuilt.
    pub mtime_nsec: u32,
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
    let album = if n >= 2 {
        parts[n - 2].to_owned()
    } else {
        UNKNOWN_ALBUM.to_owned()
    };
    let artist = if n >= 3 {
        parts[n - 3].to_owned()
    } else {
        UNKNOWN_ARTIST.to_owned()
    };
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

/// A trimmed, non-empty tag text cut to `MAX_TAG_TEXT_BYTES`.
fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(|s| limits::cut(s.trim(), MAX_TAG_TEXT_BYTES).trim_end())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_year(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

fn tag_info(tag: &Tag) -> TagInfo {
    let mut artists: Vec<String> = Vec::new();
    for value in tag.get_strings(ItemKey::TrackArtist) {
        for part in value.split(['\0', ';']) {
            if artists.len() < MAX_ARTISTS
                && let Some(a) = non_empty(Some(part))
            {
                artists.push(a);
            }
        }
    }
    let year = [
        ItemKey::RecordingDate,
        ItemKey::Year,
        ItemKey::OriginalReleaseDate,
    ]
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
        has_picture: tag.pictures().iter().any(|p| limits::cover_ok(p.data())),
    }
}

/// Runs a lofty call, treating a panic as an unreadable file. lofty 0.25.4
/// panics on some malformed files (an Opus header with 0 channels), and a
/// file any synced device can write must not take down a scan, the app or
/// the worker. The release profile therefore unwinds (`linux/Cargo.toml`).
/// The first such file is logged with a note; the panic message itself is
/// printed by the panic hook each time.
pub fn guarded<T>(what: &str, read: impl FnOnce() -> T) -> Option<T> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    static OPTIONS: std::sync::Once = std::sync::Once::new();
    // lofty refuses any single item (a picture, a frame) above this.
    OPTIONS.call_once(|| {
        lofty::config::apply_global_options(
            lofty::config::GlobalOptions::new().allocation_limit(MAX_TAG_ITEM_BYTES),
        );
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read));
    if result.is_err() && !LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!("tunemesh: the tag reader crashed on {what}; treating it as unreadable");
    }
    result.ok()
}

/// Reads tags and duration; unreadable files yield an empty `TagInfo` so the
/// track still appears with its fallbacks.
///
/// Follows symlinks: for files below the library root use
/// [`read_tags_from`] with a file from `fsx::Dir::open_file`.
pub fn read_tags(path: &Path) -> TagInfo {
    guarded(&path.display().to_string(), || {
        lofty::read_from_path(path).map_or_else(|_| TagInfo::default(), |f| tagged_info(&f))
    })
    .unwrap_or_default()
}

/// [`read_tags`] for an already open file (see `fsx::Dir::open_file`).
pub fn read_tags_from(file: &mut fs::File) -> TagInfo {
    guarded("a library file", || {
        lofty::read_from(file).map_or_else(|_| TagInfo::default(), |f| tagged_info(&f))
    })
    .unwrap_or_default()
}

fn tagged_info(file: &lofty::file::TaggedFile) -> TagInfo {
    let mut info = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .map(tag_info)
        .unwrap_or_default();
    if !info.has_picture {
        info.has_picture = file
            .tags()
            .iter()
            .any(|t| t.pictures().iter().any(|p| limits::cover_ok(p.data())));
    }
    info.duration_ms = file.properties().duration().as_millis() as u64;
    info
}

/// Reads the embedded front cover (or first picture) of an audio file.
///
/// Follows symlinks: for files below the library root use
/// [`read_embedded_cover_from`] with a file from `fsx::Dir::open_file`.
pub fn read_embedded_cover(path: &Path) -> Option<Vec<u8>> {
    guarded(&path.display().to_string(), || {
        cover_of(&lofty::read_from_path(path).ok()?)
    })
    .flatten()
}

/// [`read_embedded_cover`] for an already open file (see
/// `fsx::Dir::open_file`).
pub fn read_embedded_cover_from(file: &mut fs::File) -> Option<Vec<u8>> {
    guarded("a library file", || cover_of(&lofty::read_from(file).ok()?)).flatten()
}

fn cover_of(file: &lofty::file::TaggedFile) -> Option<Vec<u8>> {
    use lofty::picture::PictureType;
    // Only covers within the contract's limits are used.
    let pictures: Vec<_> = file
        .tags()
        .iter()
        .flat_map(|t| t.pictures())
        .filter(|p| limits::cover_ok(p.data()))
        .collect();
    pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first())
        .map(|p| p.data().to_vec())
}

/// Builds a track from its tags plus the contract's fallbacks.
pub fn make_track(
    rel: &str,
    info: TagInfo,
    dir_cover: Option<&str>,
    (mtime, mtime_nsec): (i64, u32),
    size: u64,
) -> Track {
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
        title: info
            .title
            .unwrap_or_else(|| title_from_file_name(file_name)),
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
        mtime_nsec,
        size,
    }
}

/// Walks the library below `root` through directory handles: every
/// subdirectory is opened with `O_NOFOLLOW` relative to its parent, so a
/// directory swapped for a symlink while the walk runs is skipped, never
/// followed. Hidden names, conflict copies, `Playlists/` and `.tunemesh/`
/// are left out. `visit` gets each directory's handle, its root-relative
/// path ("" for the root) and its regular files. Directories deeper than
/// `MAX_DEPTH` are skipped with a log line, and so are the entries of a
/// directory past `MAX_DIR_ENTRIES`. Each level keeps one directory handle
/// open, so the walk never holds more than `MAX_DEPTH + 1` descriptors.
fn walk(root: &Path, mut visit: impl FnMut(&Dir, &str, Vec<String>)) {
    struct Frame {
        dir: Dir,
        rel: String,
        /// Subdirectory names not walked yet.
        pending: Vec<String>,
    }
    // Reads `dir`, hands its files to `visit`, and returns its frame.
    let mut enter = |dir: Dir, rel: String| -> Frame {
        let mut files = Vec::new();
        let mut pending = Vec::new();
        let (entries, complete) = dir.entries_up_to(MAX_DIR_ENTRIES).unwrap_or_default();
        if !complete {
            eprintln!(
                "tunemesh: scanning only {MAX_DIR_ENTRIES} entries of {} (contract: \"Limits\")",
                if rel.is_empty() {
                    "the library folder"
                } else {
                    &rel
                }
            );
        }
        for (name, kind) in entries {
            if is_ignored_name(&name) {
                continue;
            }
            match kind {
                Kind::Dir
                    if !(rel.is_empty() && (name == PLAYLISTS_DIR || name == TUNEMESH_DIR)) =>
                {
                    pending.push(name)
                }
                // Symlinks are never followed (docs/library-format.md).
                Kind::File => files.push(name),
                _ => {}
            }
        }
        visit(&dir, &rel, files);
        pending.reverse(); // popped in directory order
        Frame { dir, rel, pending }
    };
    let Ok(root_dir) = Dir::open(root) else {
        return;
    };
    // Depth first, keeping open only the directories on the current path.
    let mut path = vec![enter(root_dir, String::new())];
    let mut too_deep = 0usize;
    loop {
        let depth = path.len();
        let Some(top) = path.last_mut() else { break };
        let Some(name) = top.pending.pop() else {
            path.pop();
            continue;
        };
        let rel = if top.rel.is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", top.rel)
        };
        if depth > MAX_DEPTH {
            if too_deep == 0 {
                eprintln!(
                    "tunemesh: not scanning {rel}: the library is read at most {MAX_DEPTH} directories deep"
                );
            }
            too_deep += 1;
            continue;
        }
        if let Ok(child) = top.dir.child(&name) {
            let frame = enter(child, rel);
            path.push(frame);
        }
    }
    if too_deep > 1 {
        eprintln!("tunemesh: {too_deep} directories deeper than {MAX_DEPTH} levels were skipped");
    }
}

/// Scans `root`. Tracks in `previous` whose path, mtime and size are
/// unchanged are reused without reading the file again. Returns tracks sorted
/// by path. Files are read through `fsx` handles and never through a
/// symlink, even one swapped in while the scan runs.
pub fn scan(root: &Path, previous: &HashMap<String, Track>) -> Vec<Track> {
    let mut tracks = Vec::new();
    let mut left_out = 0usize;
    walk(root, |dir, rel_dir, files| {
        let rel_of = |name: &str| {
            if rel_dir.is_empty() {
                name.to_owned()
            } else {
                format!("{rel_dir}/{name}")
            }
        };
        let mut audio = Vec::new();
        let mut dir_cover: Option<(usize, String)> = None;
        for name in files {
            let lower = name.to_ascii_lowercase();
            if let Some(rank) = COVER_FILES.iter().position(|c| *c == lower) {
                let fits = dir
                    .metadata(&name)
                    .is_ok_and(|m| m.kind == Kind::File && m.len <= MAX_COVER_BYTES);
                if fits && dir_cover.as_ref().is_none_or(|(r, _)| rank < *r) {
                    dir_cover = Some((rank, rel_of(&name)));
                }
                continue;
            }
            if !has_audio_extension(&name) {
                continue;
            }
            if tracks.len() + audio.len() >= MAX_TRACKS {
                left_out += 1;
                continue;
            }
            let Ok(meta) = dir.metadata(&name) else {
                continue;
            };
            if meta.kind == Kind::File {
                audio.push((name, meta));
            }
        }
        let dir_cover = dir_cover.map(|(_, rel)| rel);
        for (name, meta) in audio {
            let rel = rel_of(&name);
            let track = match previous.get(&rel) {
                Some(old)
                    if old.mtime == meta.mtime
                        && old.mtime_nsec == meta.mtime_nsec
                        && old.size == meta.len =>
                {
                    let mut t = old.clone();
                    if t.cover != CoverSource::Embedded {
                        t.cover = dir_cover
                            .clone()
                            .map_or(CoverSource::None, CoverSource::File);
                    }
                    t
                }
                _ => {
                    // Opened without following a symlink; the open file's own
                    // size and time are recorded.
                    let Ok(mut file) = dir.open_file(&name) else {
                        continue;
                    };
                    let (len, mtime) = file
                        .metadata()
                        .map(|m| (m.len(), (m.mtime(), m.mtime_nsec() as u32)))
                        .unwrap_or((meta.len, (meta.mtime, meta.mtime_nsec)));
                    make_track(
                        &rel,
                        read_tags_from(&mut file),
                        dir_cover.as_deref(),
                        mtime,
                        len,
                    )
                }
            };
            tracks.push(track);
        }
    });
    if left_out > 0 {
        eprintln!(
            "tunemesh: {left_out} audio files left out: the library holds at most {MAX_TRACKS} (contract: \"Limits\")"
        );
    }
    tracks.sort_by(|a, b| a.path.cmp(&b.path));
    tracks
}

/// Maps source ids (`[<id>]` in file names) to the library path carrying
/// them; when several files carry one id, the lexically first path wins.
pub fn source_id_index(root: &Path) -> HashMap<String, String> {
    let mut index: HashMap<String, String> = HashMap::new();
    walk(root, |_, rel_dir, files| {
        for name in files {
            if !has_audio_extension(&name) {
                continue;
            }
            let Some(id) = source_id_from_file_name(&name) else {
                continue;
            };
            let rel = if rel_dir.is_empty() {
                name.clone()
            } else {
                format!("{rel_dir}/{name}")
            };
            match index.get_mut(id) {
                Some(existing) if *existing <= rel => {}
                Some(existing) => *existing = rel,
                None => {
                    index.insert(id.to_owned(), rel);
                }
            }
        }
    });
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
        let by_path = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.path.clone(), i))
            .collect();

        let mut albums: Vec<Album> = Vec::new();
        let mut album_by_key: HashMap<String, usize> = HashMap::new();
        // Album of each track, so later passes need no key lookups.
        let mut album_of: Vec<usize> = Vec::with_capacity(tracks.len());
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
            album_of.push(idx);
            let a = &mut albums[idx];
            a.tracks.push(i);
            a.added = a.added.max(t.mtime);
            a.duration_ms += t.duration_ms;
            a.year = a.year.or(t.year);
        }
        for a in &mut albums {
            a.tracks.sort_by(|&x, &y| {
                let (x, y) = (&tracks[x], &tracks[y]);
                (
                    x.disc_number.unwrap_or(1),
                    x.track_number.unwrap_or(u32::MAX),
                    &x.title,
                    &x.path,
                )
                    .cmp(&(
                        y.disc_number.unwrap_or(1),
                        y.track_number.unwrap_or(u32::MAX),
                        &y.title,
                        &y.path,
                    ))
            });
        }

        // Position of each track within its album, for the artist order.
        let mut in_album = vec![0usize; tracks.len()];
        for a in &albums {
            for (p, &ti) in a.tracks.iter().enumerate() {
                in_album[ti] = p;
            }
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
        // The artist (by index) that last listed each album: dedups in O(1).
        let mut listed_by = vec![usize::MAX; albums.len()];
        for (k, artist) in artists.iter_mut().enumerate() {
            let mut seen = Vec::new();
            for &ti in &artist.tracks {
                let ai = album_of[ti];
                if listed_by[ai] != k {
                    listed_by[ai] = k;
                    seen.push(ai);
                }
            }
            seen.sort_by(|&x, &y| {
                albums[y]
                    .year
                    .cmp(&albums[x].year)
                    .then_with(|| albums[x].name.cmp(&albums[y].name))
            });
            artist.albums = seen;
            // Artist track order: by album (as above), then album order.
            let order: HashMap<usize, usize> = artist
                .albums
                .iter()
                .enumerate()
                .map(|(p, &a)| (a, p))
                .collect();
            artist
                .tracks
                .sort_by_key(|&ti| (order[&album_of[ti]], in_album[ti]));
        }
        artists.sort_by_cached_key(|a| fold(&a.name));
        let artist_by_name = artists
            .iter()
            .enumerate()
            .map(|(i, a)| (a.name.clone(), i))
            .collect();

        let track_folded = tracks
            .iter()
            .map(|t| fold(&format!("{} {} {}", t.title, t.artist, t.album)))
            .collect();
        let title_folded = tracks.iter().map(|t| fold(&t.title)).collect();

        Self {
            tracks,
            albums,
            artists,
            by_path,
            album_by_key,
            artist_by_name,
            track_folded,
            title_folded,
        }
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
            return SearchResults {
                tracks: vec![],
                albums: vec![],
                artists: vec![],
            };
        }
        let mut tracks: Vec<usize> = (0..self.tracks.len())
            .filter(|&i| query.matches(&self.track_folded[i]))
            .collect();
        tracks.sort_by_key(|&i| query.rank(&self.title_folded[i]));
        let mut albums: Vec<usize> = (0..self.albums.len())
            .filter(|&i| query.matches(&self.albums[i].folded))
            .collect();
        albums.sort_by_key(|&i| query.rank(&fold(&self.albums[i].name)));
        let mut artists: Vec<usize> = (0..self.artists.len())
            .filter(|&i| query.matches(&self.artists[i].folded))
            .collect();
        artists.sort_by_key(|&i| query.rank(&self.artists[i].folded));
        SearchResults {
            tracks,
            albums,
            artists,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_fallback_strips_extension_and_id() {
        assert_eq!(
            title_from_file_name("Me at the zoo [jNQXAC9IVRw].opus"),
            "Me at the zoo"
        );
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
        assert_eq!(
            dir_fallbacks("a.mp3"),
            (UNKNOWN_ALBUM.into(), UNKNOWN_ARTIST.into())
        );
        assert_eq!(
            dir_fallbacks("Album/a.mp3"),
            ("Album".into(), UNKNOWN_ARTIST.into())
        );
        assert_eq!(
            dir_fallbacks("Artist/Album/a.mp3"),
            ("Album".into(), "Artist".into())
        );
        assert_eq!(
            dir_fallbacks("x/Artist/Album/a.mp3"),
            ("Album".into(), "Artist".into())
        );
    }

    #[test]
    fn make_track_uses_tags_then_fallbacks() {
        let t = make_track(
            "Art/Alb/Song [id1].opus",
            TagInfo::default(),
            Some("Art/Alb/cover.jpg"),
            (5, 0),
            9,
        );
        assert_eq!(
            (t.title.as_str(), t.artist.as_str(), t.album.as_str()),
            ("Song", "Art", "Alb")
        );
        assert_eq!(t.cover, CoverSource::File("Art/Alb/cover.jpg".into()));
        assert_eq!(t.listed_artist(), "Art");

        let info = TagInfo {
            title: Some("Real".into()),
            artists: vec!["A1".into(), "A2".into()],
            album: Some("Rec".into()),
            has_picture: true,
            ..Default::default()
        };
        let t = make_track("x.mp3", info, None, (0, 0), 0);
        assert_eq!(
            (t.title.as_str(), t.artist.as_str(), t.album.as_str()),
            ("Real", "A1, A2", "Rec")
        );
        assert_eq!(t.listed_artist(), "A1");
        assert_eq!(t.cover, CoverSource::Embedded);
    }

    /// Review item 12: names with line breaks are not tracks, so they never
    /// reach a playlist line.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn scan_skips_names_with_line_breaks() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("A/B")).unwrap();
        fs::create_dir_all(root.join("C\nD")).unwrap();
        for p in [
            "A/B/one\ntwo.mp3",
            "A/B/cr\rx.mp3",
            "A/B/ok.mp3",
            "C\nD/x.mp3",
        ] {
            fs::write(root.join(p), b"x").unwrap();
        }
        let paths: Vec<_> = scan(root, &HashMap::new())
            .into_iter()
            .map(|t| t.path)
            .collect();
        assert_eq!(paths, vec!["A/B/ok.mp3"]);
        assert!(!is_track_path("A/B/one\ntwo.mp3"));
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
    #[cfg_attr(miri, ignore = "file system")]
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
        assert_eq!(
            tracks[0].cover,
            CoverSource::File("Art/Alb/cover.jpg".into())
        );
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

    /// A device swapping a scanned directory for a symlink to an outside
    /// directory (renameat2 RENAME_EXCHANGE) while the scan runs must never
    /// make it list or read a file from outside the root.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn scan_survives_directory_symlink_swaps() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.mp3"), b"outside").unwrap();
        fs::write(outside.path().join("secret [leak].opus"), b"outside").unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path().to_path_buf();
        fs::create_dir_all(root.join("swap/Album")).unwrap();
        fs::write(root.join("swap/real.mp3"), b"x").unwrap();
        fs::write(root.join("swap/Album/inner.mp3"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let swapper = {
            let (stop, root) = (stop.clone(), root.clone());
            std::thread::spawn(move || {
                let dir = fs::File::open(&root).unwrap();
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    rustix::fs::renameat_with(
                        &dir,
                        "swap",
                        &dir,
                        "link",
                        rustix::fs::RenameFlags::EXCHANGE,
                    )
                    .unwrap();
                    n += 1;
                }
                n
            })
        };
        for _ in 0..400 {
            for t in scan(&root, &HashMap::new()) {
                assert!(!t.path.contains("secret"), "read outside file: {}", t.path);
            }
            for (_, rel) in source_id_index(&root) {
                assert!(!rel.contains("secret"), "indexed outside file: {rel}");
            }
        }
        stop.store(true, Ordering::Relaxed);
        assert!(swapper.join().unwrap() > 100);
    }

    /// A 1500-level tree with two branches, scanned with an open-file
    /// limit of 64: the walk stops at `MAX_DEPTH` with a log line, holds at
    /// most one descriptor per level, finds the shallow tracks, and leaves
    /// descriptors for everything else. Runs in a child process because
    /// the limit is per process.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn deep_trees_never_exhaust_descriptors() {
        const CHILD: &str = "TUNEMESH_DEEP_TREE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "library::tests::deep_trees_never_exhaust_descriptors",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                out.status.success(),
                "{stderr}{}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(stderr.contains("directories deep"), "{stderr}");
            return;
        }
        use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Built through handles: whole paths would exceed PATH_MAX.
        for branch in ["a", "b"] {
            let mut d = Dir::open(root).unwrap();
            for level in 0..1500 {
                if level == 10 {
                    d.write_atomic(&format!("{branch}.mp3"), b"x").unwrap();
                }
                d = d.sub(&format!("{branch}{level}"), true).unwrap();
            }
            d.write_atomic("deep.mp3", b"x").unwrap();
        }
        let limit = getrlimit(Resource::Nofile);
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: Some(MAX_DEPTH as u64 + 30),
                maximum: limit.maximum,
            },
        )
        .unwrap();
        let tracks = scan(root, &HashMap::new());
        let names: Vec<_> = tracks
            .iter()
            .map(|t| t.path.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["a.mp3", "b.mp3"]);
        assert_eq!(source_id_index(root).len(), 0);
        // Descriptors are all back: another 20 files open fine.
        let open: Vec<_> = (0..20)
            .map(|_| std::fs::File::open(root).unwrap())
            .collect();
        assert_eq!(open.len(), 20);
        // Removing the tree takes one descriptor per level.
        drop(open);
        setrlimit(
            Resource::Nofile,
            Rlimit {
                current: limit.maximum,
                maximum: limit.maximum,
            },
        )
        .unwrap();
        dir.close().unwrap();
    }

    /// Ogg CRC-32 (polynomial 0x04c11db7, no reflection, init 0).
    fn ogg_crc(data: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &b in data {
            crc ^= u32::from(b) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04c1_1db7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// One Ogg page holding one packet (< 255 bytes).
    fn ogg_page(header_type: u8, granule: u64, seq: u32, packet: &[u8]) -> Vec<u8> {
        let mut p = b"OggS\0".to_vec();
        p.push(header_type);
        p.extend_from_slice(&granule.to_le_bytes());
        p.extend_from_slice(&1u32.to_le_bytes()); // serial
        p.extend_from_slice(&seq.to_le_bytes());
        p.extend_from_slice(&[0; 4]); // CRC, filled in below
        p.push(1);
        p.push(packet.len() as u8);
        p.extend_from_slice(packet);
        let crc = ogg_crc(&p);
        p[22..26].copy_from_slice(&crc.to_le_bytes());
        p
    }

    /// An Ogg Opus file whose OpusHead claims `channels` channels (byte 37
    /// of the file, as in an ffmpeg-made file).
    fn opus_with_channels(channels: u8) -> Vec<u8> {
        let mut head = b"OpusHead".to_vec();
        head.extend_from_slice(&[1, channels]);
        head.extend_from_slice(&312u16.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&[0, 0, 0]);
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&4u32.to_le_bytes());
        tags.extend_from_slice(b"test");
        tags.extend_from_slice(&0u32.to_le_bytes());
        let mut f = ogg_page(2, 0, 0, &head);
        f.extend(ogg_page(0, 0, 1, &tags));
        f.extend(ogg_page(4, 48_000, 2, &[0xf8, 0xff, 0xfe]));
        assert_eq!(f[37], channels);
        f
    }

    /// R1-opus-b-1: lofty 0.25.4 panics on an Opus header with 0 channels.
    /// The file reads as unreadable (empty tags, no cover) and the scan
    /// still returns the other tracks. Uses a hand-made file, and an
    /// ffmpeg-made one with byte 37 set to 0 when ffmpeg is on `PATH`.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn zero_channel_opus_does_not_stop_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("A/B")).unwrap();
        fs::write(root.join("A/B/Good.opus"), opus_with_channels(2)).unwrap();
        fs::write(root.join("A/B/Bad.opus"), opus_with_channels(0)).unwrap();
        let ffmpeg_made = root.join("A/B/Made.opus");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=330:duration=1",
            ])
            .args(["-c:a", "libopus"])
            .arg(&ffmpeg_made)
            .status()
            .is_ok_and(|s| s.success());
        if made {
            let mut bytes = fs::read(&ffmpeg_made).unwrap();
            assert_eq!(&bytes[28..36], b"OpusHead");
            bytes[37] = 0;
            fs::write(&ffmpeg_made, bytes).unwrap();
        } else {
            eprintln!("ffmpeg not on PATH: only the hand-made file is tested");
        }
        // The bug is still in lofty (when this fails, lofty no longer
        // panics and the guard is only a safety net).
        let raw =
            std::panic::catch_unwind(|| lofty::read_from_path(root.join("A/B/Bad.opus")).is_ok());
        assert!(raw.is_err(), "lofty no longer panics on 0 channels");
        for bad in ["A/B/Bad.opus", "A/B/Made.opus"]
            .iter()
            .filter(|p| root.join(p).exists())
        {
            let empty =
                |t: TagInfo| t.title.is_none() && t.artists.is_empty() && t.duration_ms == 0;
            assert!(empty(read_tags(&root.join(bad))));
            assert!(read_embedded_cover(&root.join(bad)).is_none());
            let mut f = fs::File::open(root.join(bad)).unwrap();
            assert!(empty(read_tags_from(&mut f)));
            let mut f = fs::File::open(root.join(bad)).unwrap();
            assert!(read_embedded_cover_from(&mut f).is_none());
        }
        let tracks = scan(root, &HashMap::new());
        let paths: Vec<_> = tracks.iter().map(|t| t.path.as_str()).collect();
        let mut expected = vec!["A/B/Bad.opus", "A/B/Good.opus"];
        if made {
            expected.push("A/B/Made.opus");
        }
        assert_eq!(paths, expected);
        assert_eq!(tracks[0].title, "Bad");
    }

    /// R1-astra-3: a same-size rewrite within one second (only the
    /// nanoseconds of the modification time differ) is read again.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn scan_cache_compares_nanoseconds() {
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("A/B")).unwrap();
        let path = root.join("A/B/song.mp3");
        fs::write(&path, b"xxxx").unwrap();
        let second = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_470_994);
        let set = |nanos: u32| {
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(second + Duration::from_nanos(nanos.into()))
                .unwrap()
        };
        set(281_060_366);
        let stale = |tracks: Vec<Track>| -> HashMap<String, Track> {
            tracks
                .into_iter()
                .map(|mut t| {
                    t.title = "stale".into();
                    (t.path.clone(), t)
                })
                .collect()
        };
        let first = scan(root, &HashMap::new());
        assert_eq!(first[0].mtime_nsec, 281_060_366);
        // Unchanged: the cached track is reused.
        let previous = stale(first);
        assert_eq!(scan(root, &previous)[0].title, "stale");
        // Same size, same second, other nanoseconds: read again.
        set(123_456_789);
        let again = scan(root, &previous);
        assert_eq!(again[0].title, "song");
        assert_eq!(
            (again[0].mtime, again[0].mtime_nsec),
            (1_792_470_994, 123_456_789)
        );
        // A cache written before `mtime_nsec` existed does not parse.
        let mut old = serde_json::to_value(&again[0]).unwrap();
        old.as_object_mut().unwrap().remove("mtime_nsec");
        assert!(serde_json::from_value::<Track>(old).is_err());
    }

    /// R2-opus-2: grouping is O(n log n): one 40,000-track album with
    /// shuffled track numbers builds in well under a second (it took 8 s
    /// when each artist track looked up its album position linearly).
    #[test]
    #[cfg_attr(miri, ignore = "too slow under Miri")]
    fn huge_album_builds_quickly() {
        let n = 40_000u32;
        let mut numbers: Vec<u32> = (1..=n).collect();
        fastrand::Rng::with_seed(7).shuffle(&mut numbers);
        let tracks: Vec<Track> = numbers
            .iter()
            .enumerate()
            .map(|(i, &no)| {
                let info = TagInfo {
                    title: Some(format!("Song {i}")),
                    artists: vec!["One Artist".into()],
                    album: Some("One Album".into()),
                    track_number: Some(no),
                    ..TagInfo::default()
                };
                make_track(&format!("A/B/{i:05}.mp3"), info, None, (0, 0), 0)
            })
            .collect();
        let start = std::time::Instant::now();
        let lib = Library::new(tracks);
        let took = start.elapsed();
        // Generous for debug builds and busy machines (release: ~30 ms).
        assert!(
            took < std::time::Duration::from_secs(if cfg!(debug_assertions) { 3 } else { 1 }),
            "{took:?}"
        );
        assert_eq!(lib.albums.len(), 1);
        let album = &lib.albums[0].tracks;
        assert_eq!(lib.tracks[album[0]].track_number, Some(1));
        assert_eq!(lib.tracks[album[n as usize - 1]].track_number, Some(n));
        assert_eq!(lib.artists[0].tracks, *album);
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn scan_ignores_symlinks() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.mp3"), b"x").unwrap();
        fs::create_dir(outside.path().join("Dir")).unwrap();
        fs::write(outside.path().join("Dir/song.mp3"), b"x").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Art/Alb")).unwrap();
        fs::write(root.join("Art/Alb/real.mp3"), b"x").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.mp3"),
            root.join("Art/Alb/link.mp3"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path().join("Dir"), root.join("Linked")).unwrap();
        let tracks = scan(root, &HashMap::new());
        let paths: Vec<_> = tracks.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(paths, vec!["Art/Alb/real.mp3"]);
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
            make_track(path, info, None, (n as i64, 0), 0)
        };
        let lib = Library::new(vec![
            mk("a/2.mp3", Some("VA"), &["X"], "Comp", 2),
            mk("a/1.mp3", Some("VA"), &["Y"], "Comp", 1),
            mk("b/1.mp3", None, &["Z", "W"], "Solo", 1),
        ]);
        assert_eq!(lib.albums.len(), 2);
        let comp = &lib.albums[lib.album_index(&album_key("VA", "Comp")).unwrap()];
        let titles: Vec<_> = comp
            .tracks
            .iter()
            .map(|&i| lib.tracks[i].title.as_str())
            .collect();
        assert_eq!(titles, vec!["T1", "T2"]);
        let names: Vec<_> = lib.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["VA", "Z"]);
        let r = lib.search(&Query::new("w solo"));
        assert_eq!(r.tracks.len(), 1);
    }
}
