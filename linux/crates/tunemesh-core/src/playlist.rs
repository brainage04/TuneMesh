//! `Playlists/<Name>.m3u8` (contract: "Playlists").

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use crate::fsx::{Dir, Kind};
use crate::library::{has_line_break, is_conflict_copy, title_from_file_name};
use crate::limits::{
    self, MAX_LABEL_BYTES, MAX_PLAYLIST_BYTES, MAX_PLAYLIST_ENTRIES as MAX_ENTRIES,
    MAX_PLAYLIST_LINE_BYTES as MAX_LINE_BYTES, MAX_PLAYLISTS,
};
use crate::{PLAYLISTS_DIR, sanitize};

pub const LIKED_SONGS: &str = "Liked Songs";
const EXT: &str = ".m3u8";

/// A playlist over the contract's limits (`limits::MAX_PLAYLIST_*`): listed, but never opened for
/// editing, rewritten or truncated. Carried inside an `io::Error`
/// (`is_too_large`).
#[derive(Debug)]
pub struct TooLarge(&'static str);

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the playlist is too large ({}), so it is left unchanged",
            self.0
        )
    }
}

impl std::error::Error for TooLarge {}

fn too_large(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, TooLarge(why))
}

/// Whether reading or writing failed because the playlist is over the
/// contract's size limits.
pub fn is_too_large(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<TooLarge>())
}

/// `meta` bit: the entry's line points outside the root.
const OUTSIDE: u32 = 1 << 31;
/// `meta` duration value: unknown.
const NO_DURATION: u32 = OUTSIDE - 1;

/// Where one entry's text lives in `Entries::text`: the path at
/// `start..start + path_len`, directly followed by its `#EXTINF` label
/// (`label_len` bytes, 0 for none). 12 bytes per entry; both lengths fit
/// in 16 bits because readers refuse longer lines and cut labels to
/// `MAX_LABEL_BYTES` (`push` enforces it for every entry). `repr(C)` keeps
/// the declared field order, so the layout (and its 12 bytes) is fixed.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
struct Span {
    start: u32,
    path_len: u16,
    label_len: u16,
    /// `OUTSIDE` flag plus the `#EXTINF` duration in seconds
    /// (`NO_DURATION`: unknown).
    meta: u32,
}

/// A playlist's entries: one text buffer and a 12-byte span per entry, so a
/// 250,000-entry playlist costs little more than its paths.
///
/// Invariants: every span lies inside `text` at character boundaries.
/// Edits only rearrange or drop spans; text no span refers to any more is
/// garbage until `compact` (run by the dropping edits when more than half
/// of `text` is garbage).
#[derive(Debug, Clone, Default)]
pub struct Entries {
    text: String,
    spans: Vec<Span>,
}

/// One playlist entry, borrowed from its `Playlist`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaylistEntry<'a> {
    /// The root-relative track path, or the line as read when it points
    /// outside the root. Writers write a track as `../<path>`, so the
    /// original spelling of a track line is not kept.
    path: &'a str,
    label: Option<&'a str>,
    meta: u32,
}

impl<'a> PlaylistEntry<'a> {
    /// Root-relative track path; `None` when the line points outside the
    /// root.
    pub fn track(self) -> Option<&'a str> {
        (self.meta & OUTSIDE == 0).then_some(self.path)
    }

    /// The entry's identity: the track path, or the line as written when it
    /// points outside the root.
    pub fn key(self) -> &'a str {
        self.path
    }

    /// The path line writers write for this entry.
    pub fn line(self) -> Cow<'a, str> {
        match self.track() {
            Some(t) => Cow::Owned(format!("../{t}")),
            None => Cow::Borrowed(self.path),
        }
    }

    /// `#EXTINF` display text.
    pub fn label(self) -> Option<&'a str> {
        self.label
    }

    /// `#EXTINF` duration in seconds, when known.
    pub fn duration(self) -> Option<u64> {
        let d = self.meta & !OUTSIDE;
        (d != NO_DURATION).then_some(u64::from(d))
    }
}

impl Entries {
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    fn view(&self, s: Span) -> PlaylistEntry<'_> {
        Self::view_in(&self.text, s)
    }

    pub fn get(&self, i: usize) -> Option<PlaylistEntry<'_>> {
        self.spans.get(i).map(|&s| self.view(s))
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = PlaylistEntry<'_>> + ExactSizeIterator {
        self.spans.iter().map(|&s| self.view(s))
    }

    /// Appends an entry; `false` (nothing added) when its path is longer
    /// than 64 KiB (twice a playlist line; no writer could write it) or
    /// the buffer would pass 4 GiB. The label is cut to `MAX_LABEL_BYTES`.
    fn push(&mut self, path: &str, label: Option<&str>, meta: u32) -> bool {
        let label = label.map_or("", |l| limits::cut(l, MAX_LABEL_BYTES));
        let (Ok(path_len), Ok(label_len), Ok(start)) = (
            u16::try_from(path.len()),
            u16::try_from(label.len()),
            u32::try_from(self.text.len()),
        ) else {
            return false;
        };
        if start
            .checked_add(u32::from(path_len) + u32::from(label_len))
            .is_none()
        {
            return false;
        }
        self.text.push_str(path);
        self.text.push_str(label);
        self.spans.push(Span {
            start,
            path_len,
            label_len,
            meta,
        });
        true
    }

    /// Appends a track entry (no label, unknown duration).
    fn push_track(&mut self, track: &str) -> bool {
        self.push(track, None, NO_DURATION)
    }

    /// Drops the spans for which `keep` is false, then compacts if much of
    /// the text is garbage.
    fn retain(&mut self, mut keep: impl FnMut(usize, PlaylistEntry<'_>) -> bool) {
        let text = self.text.as_str();
        let mut i = 0;
        self.spans.retain(|&s| {
            let view = Entries::view_in(text, s);
            i += 1;
            keep(i - 1, view)
        });
        self.compact_if_sparse();
    }

    fn view_in(text: &str, s: Span) -> PlaylistEntry<'_> {
        let start = s.start as usize;
        let mid = start + usize::from(s.path_len);
        let end = mid + usize::from(s.label_len);
        PlaylistEntry {
            path: &text[start..mid],
            label: (s.label_len > 0).then(|| &text[mid..end]),
            meta: s.meta,
        }
    }

    fn live_bytes(&self) -> usize {
        self.spans
            .iter()
            .map(|s| usize::from(s.path_len) + usize::from(s.label_len))
            .sum()
    }

    fn compact_if_sparse(&mut self) {
        if self.live_bytes() * 2 < self.text.len() {
            self.compact();
        }
    }

    /// Rewrites `text` with only the live spans, in entry order.
    fn compact(&mut self) {
        let mut text = String::with_capacity(self.live_bytes());
        for s in &mut self.spans {
            let start = s.start as usize;
            let end = start + usize::from(s.path_len) + usize::from(s.label_len);
            let new = text.len() as u32;
            text.push_str(&self.text[start..end]);
            s.start = new;
        }
        self.text = text;
    }

    fn shrink_to_fit(&mut self) {
        self.text.shrink_to_fit();
        self.spans.shrink_to_fit();
    }
}

impl PartialEq for Entries {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl Eq for Entries {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub name: String,
    pub entries: Entries,
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

/// Resolves a playlist line to a root-relative path, built in `out`
/// (cleared first; reused between lines so resolving allocates nothing).
fn resolve_into(line: &str, root: &Path, out: &mut String) -> bool {
    out.clear();
    let rest = if line.starts_with('/') {
        let Some(root_str) = root.to_str() else {
            return false;
        };
        let Some(rest) = line
            .strip_prefix(root_str.trim_end_matches('/'))
            .and_then(|r| r.strip_prefix('/'))
        else {
            return false;
        };
        rest
    } else {
        out.push_str(PLAYLISTS_DIR);
        line
    };
    for comp in rest.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                if out.is_empty() {
                    return false;
                }
                out.truncate(out.rfind('/').unwrap_or(0));
            }
            c => {
                if !out.is_empty() {
                    out.push('/');
                }
                out.push_str(c);
            }
        }
    }
    !out.is_empty()
}

/// Resolves a playlist line to a root-relative path.
#[cfg(test)]
fn resolve(line: &str, root: &Path) -> Option<String> {
    let mut out = String::new();
    resolve_into(line, root, &mut out).then_some(out)
}

/// A meaningful playlist line.
enum Line<'a> {
    /// The text after `#EXTINF:`.
    Extinf(&'a str),
    /// A path line.
    Entry(&'a str),
}

/// Reads playlist lines from `reader` (BOM, CRLF and invalid UTF-8
/// tolerated; blank lines and other `#` lines skipped) and hands the
/// meaningful ones to `f`. A file over `MAX_PLAYLIST_BYTES`, `MAX_ENTRIES`
/// or `MAX_LINE_BYTES` is a `TooLarge` error; so is `f` returning `false`.
fn read_lines(mut reader: impl BufRead, mut f: impl FnMut(Line<'_>) -> bool) -> io::Result<()> {
    let mut buf = Vec::new();
    let (mut total, mut entries, mut first) = (0u64, 0usize, true);
    loop {
        buf.clear();
        let max_read = MAX_LINE_BYTES as u64 + 2;
        let n = (&mut reader).take(max_read).read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        total += n as u64;
        if total > MAX_PLAYLIST_BYTES {
            return Err(too_large("larger than 32 MiB"));
        }
        // Cut off by the per-line limit: only CRs may follow before the
        // line's LF (or the end); any other byte makes the line, CRs inside
        // it included, longer than the limit.
        if n as u64 == max_read && buf.last() != Some(&b'\n') {
            loop {
                let rest = reader.fill_buf()?;
                let crs = rest.iter().take_while(|b| **b == b'\r').count();
                let (used, end) = match rest.get(crs) {
                    None if rest.is_empty() => break,
                    None => (crs, false),
                    Some(b'\n') => (crs + 1, true),
                    Some(_) => return Err(too_large("a line is longer than 32 KiB")),
                };
                reader.consume(used);
                total += used as u64;
                if end {
                    break;
                }
            }
        }
        if total > MAX_PLAYLIST_BYTES {
            return Err(too_large("larger than 32 MiB"));
        }
        let mut bytes = buf.strip_suffix(b"\n").unwrap_or(&buf);
        while let Some(b) = bytes.strip_suffix(b"\r") {
            bytes = b;
        }
        if bytes.len() > MAX_LINE_BYTES {
            return Err(too_large("a line is longer than 32 KiB"));
        }
        let text = String::from_utf8_lossy(bytes);
        let mut line: &str = &text;
        if first {
            line = line.strip_prefix('\u{feff}').unwrap_or(line);
            first = false;
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            if !f(Line::Extinf(info)) {
                return Err(too_large("an entry does not fit"));
            }
        } else if !line.starts_with('#') {
            entries += 1;
            if entries > MAX_ENTRIES {
                return Err(too_large("more than 250,000 entries"));
            }
            if !f(Line::Entry(line)) {
                return Err(too_large("an entry does not fit"));
            }
        }
    }
}

/// Parses the text after `#EXTINF:`: the `meta` duration, and the label
/// (cut to `MAX_LABEL_BYTES`) into `label`.
fn parse_extinf(info: &str, label: &mut String) -> u32 {
    let (dur, text) = info.split_once(',').unwrap_or((info, ""));
    label.clear();
    label.push_str(limits::cut(text.trim(), MAX_LABEL_BYTES).trim_end());
    dur.trim()
        .parse::<i64>()
        .ok()
        .filter(|d| *d >= 0)
        .map_or(NO_DURATION, |d| d.min(i64::from(NO_DURATION - 1)) as u32)
}

/// Reads entries from `reader` with the contract's limits (see
/// `read_lines`). Nothing is allocated per entry.
fn read_entries(reader: impl BufRead, root: &Path) -> io::Result<Entries> {
    let mut entries = Entries::default();
    let mut pending: Option<u32> = None;
    let mut label = String::new();
    let mut resolved = String::new();
    read_lines(reader, |line| match line {
        Line::Extinf(info) => {
            pending = Some(parse_extinf(info, &mut label));
            true
        }
        Line::Entry(line) => {
            let (duration, label) = match pending.take() {
                Some(d) => (d, (!label.is_empty()).then_some(label.as_str())),
                None => (NO_DURATION, None),
            };
            // Fails only for a line of invalid UTF-8 that its U+FFFD
            // replacements make longer than 64 KiB.
            if resolve_into(line, root, &mut resolved) {
                entries.push(&resolved, label, duration)
            } else {
                entries.push(line, label, duration | OUTSIDE)
            }
        }
    })?;
    entries.shrink_to_fit();
    Ok(entries)
}

/// Parses playlist text, with the same limits as `Playlist::read`.
/// Tolerates a BOM and CRLF line endings.
pub fn parse(text: &str, root: &Path) -> io::Result<Entries> {
    read_entries(text.as_bytes(), root)
}

/// Parses a playlist file's bytes exactly as `Playlist::read` does (invalid
/// UTF-8 replaced).
pub fn parse_bytes(bytes: &[u8], root: &Path) -> io::Result<Entries> {
    read_entries(bytes, root)
}

/// `label` as readers read it back (`parse_extinf`): line breaks replaced,
/// white space trimmed, cut to `MAX_LABEL_BYTES` at a character boundary.
fn clean_label(label: &str) -> String {
    let label = label.replace(['\n', '\r'], " ");
    limits::cut(label.trim(), MAX_LABEL_BYTES)
        .trim_end()
        .to_owned()
}

/// Serialises entries: `#EXTM3U`, then `#EXTINF` + path per entry, LF endings.
pub fn serialize(entries: &Entries, info: &dyn Fn(&str) -> Option<TrackInfo>) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("#EXTM3U\n");
    // An entry with a line break would turn into two lines.
    for e in entries.iter().filter(|e| !has_line_break(e.path)) {
        let known = e.track().and_then(info);
        let duration = known
            .as_ref()
            .and_then(|k| k.duration_secs)
            .or(e.duration());
        let label = match (&known, e.label()) {
            (Some(k), _) => clean_label(&k.label),
            (None, Some(l)) => clean_label(l),
            (None, None) => clean_label(&title_from_file_name(
                e.path.rsplit('/').next().unwrap_or(e.path),
            )),
        };
        match duration {
            Some(d) => _ = writeln!(out, "#EXTINF:{d},{label}"),
            None => _ = writeln!(out, "#EXTINF:-1,{label}"),
        }
        if e.track().is_some() {
            out.push_str("../");
        }
        out.push_str(e.path);
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
    if names.len() > MAX_PLAYLISTS {
        eprintln!(
            "tunemesh: listing the first {MAX_PLAYLISTS} of {} playlists (contract: \"Limits\")",
            names.len()
        );
        names.truncate(MAX_PLAYLISTS);
    }
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

/// Reads `name` through `f` with the contract's limits; a missing Liked
/// Songs reads as empty. The file is opened without following symlinks.
fn read_file(
    root: &Path,
    name: &str,
    f: impl FnOnce(&mut dyn BufRead) -> io::Result<()>,
) -> io::Result<()> {
    match open_dir(root, false).and_then(|d| d.open_file(&file_name(name))) {
        Ok(file) => f(&mut BufReader::with_capacity(64 * 1024, file)),
        Err(e) if e.kind() == io::ErrorKind::NotFound && name == LIKED_SONGS => Ok(()),
        Err(e) => Err(e),
    }
}

impl Playlist {
    /// An empty playlist (not written).
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            entries: Entries::default(),
        }
    }

    /// Reads a playlist; a missing Liked Songs file reads as empty. A
    /// playlist that is a symlink (or below a symlinked `Playlists`) is
    /// refused, so a synced link cannot leak the file it points to. A
    /// playlist over the size limits is an error (`is_too_large`).
    pub fn read(root: &Path, name: &str) -> io::Result<Self> {
        let mut entries = Entries::default();
        read_file(root, name, |r| {
            entries = read_entries(r, root)?;
            Ok(())
        })?;
        Ok(Self {
            name: name.to_owned(),
            entries,
        })
    }

    /// The entry count and the first `first` resolvable tracks, read with
    /// `read`'s rules (and limits) without building the entries.
    pub fn summary(root: &Path, name: &str, first: usize) -> io::Result<Summary> {
        let mut count = 0;
        let mut first_tracks = Vec::new();
        let mut resolved = String::new();
        read_file(root, name, |r| {
            read_lines(r, |line| {
                if let Line::Entry(line) = line {
                    count += 1;
                    if first_tracks.len() < first && resolve_into(line, root, &mut resolved) {
                        first_tracks.push(resolved.clone());
                    }
                }
                true
            })
        })?;
        Ok(Summary {
            name: name.to_owned(),
            count,
            first_tracks,
        })
    }

    /// Writes the playlist atomically. A playlist that would be over the
    /// size limits is not written (`is_too_large`), so readers can always
    /// open what writers write.
    pub fn write(&self, root: &Path, info: &dyn Fn(&str) -> Option<TrackInfo>) -> io::Result<()> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(too_large("more than 250,000 entries"));
        }
        let text = serialize(&self.entries, info);
        if text.len() as u64 > MAX_PLAYLIST_BYTES {
            return Err(too_large("larger than 32 MiB"));
        }
        // Every line, #EXTINF lines included, must be one readers accept.
        if text.lines().any(|line| line.len() > MAX_LINE_BYTES) {
            return Err(too_large("a line is longer than 32 KiB"));
        }
        open_dir(root, true)?.write_atomic(&file_name(&self.name), text.as_bytes())
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
        let p = Self::new(&name);
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
        self.entries.iter().any(|e| e.track() == Some(track))
    }

    /// Liked Songs: liking inserts at the top (no-op when already liked).
    /// Tracks whose path has a line break are skipped (contract: they are
    /// never library tracks, and no playlist line can hold them).
    pub fn like(&mut self, track: &str) {
        self.like_all([track]);
    }

    /// Liked Songs: likes `tracks` at once, in their order, at the top
    /// (already liked ones and line-break paths skipped). Linear, unlike
    /// calling `like` for each.
    pub fn like_all<'a>(&mut self, tracks: impl IntoIterator<Item = &'a str>) {
        let mut seen: HashSet<&str> = self
            .entries
            .iter()
            .filter_map(PlaylistEntry::track)
            .collect();
        let new: Vec<&'a str> = tracks
            .into_iter()
            .filter(|t| !has_line_break(t) && seen.insert(t))
            .collect();
        let added = new
            .into_iter()
            .filter(|t| self.entries.push_track(t))
            .count();
        // Pushed at the end; move them to the top, keeping their order.
        self.entries.spans.rotate_right(added);
    }

    /// Liked Songs: unlikes every track in `tracks` at once (linear).
    pub fn unlike_all<'a>(&mut self, tracks: impl IntoIterator<Item = &'a str>) {
        let gone: HashSet<&str> = tracks.into_iter().collect();
        self.entries
            .retain(|_, e| e.track().is_none_or(|t| !gone.contains(t)));
    }

    /// Liked Songs: unliking removes every entry for the track.
    pub fn unlike(&mut self, track: &str) {
        self.entries.retain(|_, e| e.track() != Some(track));
    }

    /// Appends tracks at the end (line-break paths skipped).
    pub fn append<'a>(&mut self, tracks: impl IntoIterator<Item = &'a str>) {
        for t in tracks.into_iter().filter(|t| !has_line_break(t)) {
            self.entries.push_track(t);
        }
    }

    /// Removes the entries at `positions` (any order, duplicates ignored).
    pub fn remove_positions(&mut self, positions: &[usize]) {
        let mut remove = vec![false; self.entries.len()];
        for &p in positions {
            if let Some(r) = remove.get_mut(p) {
                *r = true;
            }
        }
        self.entries.retain(|i, _| !remove[i]);
    }

    /// Moves the entry at `from` so it ends up at index `to`.
    pub fn move_entry(&mut self, from: usize, to: usize) {
        let spans = &mut self.entries.spans;
        if from < spans.len() && to < spans.len() && from != to {
            let e = spans.remove(from);
            spans.insert(to, e);
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
    #[cfg_attr(miri, ignore = "file system")]
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
        let p = Playlist::new("z");
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
        let entries = parse(text, root).unwrap();
        let tracks: Vec<_> = entries.iter().map(|e| e.track()).collect();
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
        assert_eq!(entries.get(0).unwrap().duration(), Some(100));
        assert_eq!(entries.get(0).unwrap().label(), Some("Old label"));

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
        let again = parse(&out, root).unwrap();
        assert_eq!(again.len(), 5);
        assert_eq!(
            again
                .iter()
                .map(|e| e.track().map(str::to_owned))
                .collect::<Vec<_>>(),
            tracks
                .iter()
                .map(|t| t.map(str::to_owned))
                .collect::<Vec<_>>()
        );
        assert_eq!(serialize(&again, &info), out);
    }

    #[test]
    fn liked_songs_insert_at_top_and_unlike_removes_all() {
        let mut p = Playlist::new(LIKED_SONGS);
        p.like("a.mp3");
        p.like("b.mp3");
        p.like("a.mp3");
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track().unwrap())
                .collect::<Vec<_>>(),
            vec!["b.mp3", "a.mp3"]
        );
        p.append(["c.mp3", "b.mp3"]);
        p.unlike("b.mp3");
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track().unwrap())
                .collect::<Vec<_>>(),
            vec!["a.mp3", "c.mp3"]
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
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
    #[cfg_attr(miri, ignore = "file system")]
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
        let mut p = Playlist::new("x");
        p.append(["A/B/one\ntwo.mp3", "A/B/ok.mp3", "A/B/cr\r.mp3"]);
        p.like("L/x\ny.mp3");
        assert_eq!(p.entries.len(), 1);
        p.entries.push_track("A/B/sneak\ny.mp3");
        let out = serialize(&p.entries, &|_| None);
        assert_eq!(out, "#EXTM3U\n#EXTINF:-1,ok\n../A/B/ok.mp3\n");
    }

    #[test]
    fn remove_and_move_by_position() {
        let mut p = Playlist::new("x");
        p.append(["a", "b", "a", "c"]);
        p.remove_positions(&[2]);
        p.move_entry(2, 0);
        assert_eq!(
            p.entries
                .iter()
                .map(|e| e.track().unwrap())
                .collect::<Vec<_>>(),
            vec!["c", "a", "b"]
        );
    }

    /// cargo-mutants: `contains`, `is_empty`, equality, out-of-range moves,
    /// `parse_bytes`, the duration cap and the error type had no test that
    /// noticed them changed.
    #[test]
    fn entries_api_edges() {
        let root = Path::new("/r");
        let mut p = Playlist::new("x");
        assert!(p.entries.is_empty());
        p.append(["a", "b"]);
        assert!(!p.entries.is_empty());
        assert!(p.contains("a") && !p.contains("c"));
        p.move_entry(2, 0);
        p.move_entry(0, 2);
        assert_eq!(
            p.entries.iter().map(PlaylistEntry::key).collect::<Vec<_>>(),
            ["a", "b"]
        );
        let outside = parse("/elsewhere/x.mp3\n", root).unwrap();
        let q = Playlist {
            name: "y".to_owned(),
            entries: outside,
        };
        assert!(!q.contains("/elsewhere/x.mp3"));

        assert_eq!(
            parse("../a\n", root).unwrap(),
            parse("../a\n", root).unwrap()
        );
        assert_ne!(
            parse("../a\n", root).unwrap(),
            parse("../b\n", root).unwrap()
        );
        assert_ne!(
            parse("../a\n", root).unwrap(),
            parse("../a\n../a\n", root).unwrap()
        );

        let lossy = parse_bytes(b"../a/\xff.mp3\n", root).unwrap();
        assert_eq!(lossy.get(0).unwrap().track(), Some("a/\u{fffd}.mp3"));
        let capped = parse("#EXTINF:99999999999,L\n../a.mp3\n", root).unwrap();
        assert_eq!(
            capped.get(0).unwrap().duration(),
            Some(u64::from(NO_DURATION - 1))
        );

        let e = too_large("x");
        assert!(is_too_large(&e));
        assert_eq!(
            e.to_string(),
            "the playlist is too large (x), so it is left unchanged"
        );
        assert!(!is_too_large(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(!is_too_large(&io::Error::other("x")));
    }

    /// Dropping most entries compacts the text, labels included.
    /// cargo-mutants: no test noticed compaction switched off.
    #[test]
    fn removals_reclaim_their_text() {
        let text: String = (0..1_000)
            .map(|i| format!("#EXTINF:{i},Label {i}\n../A/{i:04}.mp3\n"))
            .collect();
        let mut p = Playlist {
            name: "x".to_owned(),
            entries: parse(&text, Path::new("/r")).unwrap(),
        };
        let full = p.entries.text.len();
        p.remove_positions(&(0..900).collect::<Vec<_>>());
        assert_eq!(p.entries.text.len(), p.entries.live_bytes());
        assert!(p.entries.text.len() < full / 5);
        let first = p.entries.get(0).unwrap();
        assert_eq!(
            (first.track(), first.label(), first.duration()),
            (Some("A/0900.mp3"), Some("Label 900"), Some(900))
        );
        // 51 of 100 (all the same size): fewer than half are left.
        let last: Vec<String> = (949..1_000).map(|i| format!("A/{i:04}.mp3")).collect();
        p.unlike_all(last.iter().map(String::as_str));
        assert_eq!(p.entries.len(), 49);
        assert_eq!(p.entries.text.len(), p.entries.live_bytes());
        assert_eq!(p.entries.get(48).unwrap().label(), Some("Label 948"));
    }

    /// A playlist of exactly 32 MiB is read; one byte more is too large.
    /// cargo-mutants: only files far over the limit were tested.
    #[test]
    #[cfg_attr(miri, ignore = "too slow under Miri")]
    fn playlist_size_limit_is_inclusive() {
        let line = format!("#{}\n", "x".repeat(MAX_LINE_BYTES - 2));
        let exact = line.repeat(MAX_PLAYLIST_BYTES as usize / line.len());
        assert_eq!(exact.len() as u64, MAX_PLAYLIST_BYTES);
        assert!(parse(&exact, Path::new("/r")).is_ok());
        let over = format!("{exact}\n");
        assert!(is_too_large(&parse(&over, Path::new("/r")).unwrap_err()));
    }

    /// `summary` agrees with `parse` on count and first resolvable tracks,
    /// and follows `read`'s rules.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
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
        let parsed = parse(&text, root).unwrap();
        let resolved: Vec<String> = parsed
            .iter()
            .filter_map(|e| e.track().map(str::to_owned))
            .collect();
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
            [Playlist::read(root, "Bad")
                .unwrap()
                .entries
                .get(0)
                .unwrap()
                .track()
                .unwrap()
                .to_owned()]
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

    /// Over any of the three limits, a playlist is refused by `read`,
    /// `summary` and the writers, and its file is left byte for byte.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn too_large_playlists_are_refused_and_never_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        let entries_ok = format!("#EXTM3U\n{}", "../a.mp3\n".repeat(MAX_ENTRIES));
        let too_many = format!("{entries_ok}../b.mp3\n");
        let long_line = format!("#EXTM3U\n../{}.mp3\n", "x".repeat(MAX_LINE_BYTES));
        let long_comment = format!("#EXTM3U\n#{}\n", "x".repeat(MAX_LINE_BYTES));
        let mut huge = String::from("#EXTM3U\n");
        let line = format!("#EXTINF:1,{}\n", "y".repeat(30_000));
        while huge.len() as u64 <= MAX_PLAYLIST_BYTES {
            huge.push_str(&line);
        }
        let ok_line = format!("#EXTM3U\n../{}.mp3\r\n", "x".repeat(MAX_LINE_BYTES - 7));
        // CRs inside a line count; only trailing ones are the line ending.
        let inner_crs = format!("#EXTM3U\n{}\r\r\rb\n", "a".repeat(MAX_LINE_BYTES - 1));
        let crs_at_end = format!("#EXTM3U\n{}\r\r\r\n", "a".repeat(MAX_LINE_BYTES));
        let crs_at_eof = format!("#EXTM3U\n{}\r\r", "a".repeat(MAX_LINE_BYTES));
        for (name, text, ok) in [
            ("Many", &entries_ok, true),
            ("TooMany", &too_many, false),
            ("Long", &long_line, false),
            ("LongComment", &long_comment, false),
            ("Huge", &huge, false),
            ("LongOk", &ok_line, true),
            ("InnerCrs", &inner_crs, false),
            ("CrsAtEnd", &crs_at_end, true),
            ("CrsAtEof", &crs_at_eof, true),
        ] {
            let path = root.join(format!("Playlists/{name}.m3u8"));
            fs::write(&path, text).unwrap();
            let read = Playlist::read(root, name);
            let summary = Playlist::summary(root, name, 3);
            if ok {
                assert!(read.is_ok() && summary.is_ok(), "{name}");
                continue;
            }
            let (Err(r), Err(s)) = (read, summary) else {
                panic!("{name} was read");
            };
            assert!(is_too_large(&r) && is_too_large(&s), "{name}: {r}");
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes(), "{name}");
        }
        // A writer cannot produce what readers refuse.
        let mut p = Playlist::read(root, "Many").unwrap();
        p.append(["b.mp3"]);
        let e = p.write(root, &|_| None).unwrap_err();
        assert!(is_too_large(&e));
        assert_eq!(
            fs::read_to_string(root.join("Playlists/Many.m3u8")).unwrap(),
            entries_ok
        );
        // cargo-mutants: exactly at the entry limit is still written, and a
        // text over 32 MiB is not.
        let at_limit = Playlist {
            name: "AtLimit".to_owned(),
            entries: Playlist::read(root, "Many").unwrap().entries,
        };
        at_limit.write(root, &|_| None).unwrap();
        let mut big = Playlist::new("Big");
        let tracks: Vec<String> = (0..40_000).map(|i| format!("A/{i}.mp3")).collect();
        big.append(tracks.iter().map(String::as_str));
        let long_label = |_: &str| {
            Some(TrackInfo {
                duration_secs: None,
                label: "l".repeat(MAX_LABEL_BYTES),
            })
        };
        assert!(is_too_large(&big.write(root, &long_label).unwrap_err()));
        assert!(!root.join("Playlists/Big.m3u8").exists());
        // Labels are cut so a written line always fits.
        let mut q = Playlist::create(root, "Labels").unwrap();
        q.append(["c.mp3"]);
        q.write(root, &|_| {
            Some(TrackInfo {
                duration_secs: None,
                label: "é".repeat(40_000),
            })
        })
        .unwrap();
        assert!(Playlist::read(root, "Labels").is_ok());
    }

    /// Found by `fuzz/fuzz_targets/playlist_parse.rs`: a label with
    /// surrounding white space (the fallback title of `\t1`) was written as
    /// is and read back trimmed, so writing what was read changed the file.
    #[test]
    fn written_labels_read_back_unchanged() {
        let root = Path::new("/r");
        let e = parse("\t1\n#EXTINF:3, a\rb \n../x.mp3\n", root).unwrap();
        let out = serialize(&e, &|_| None);
        assert_eq!(
            out,
            "#EXTM3U\n#EXTINF:-1,1\n../Playlists/\t1\n#EXTINF:3,a b\n../x.mp3\n"
        );
        assert_eq!(serialize(&parse(&out, root).unwrap(), &|_| None), out);
        // Android's Jazzer run: a fallback title ending in U+000B.
        let e = parse("ĂP\u{b}\n", root).unwrap();
        assert_eq!(
            serialize(&e, &|_| None),
            "#EXTM3U\n#EXTINF:-1,ĂP\n../Playlists/ĂP\u{b}\n"
        );
        // Trimmed before the cut, as readers do.
        let mut p = Playlist::new("x");
        p.append(["a.mp3"]);
        let padded = |_: &str| {
            Some(TrackInfo {
                duration_secs: None,
                label: format!("{}x ", " ".repeat(2 * MAX_LABEL_BYTES)),
            })
        };
        assert_eq!(
            serialize(&p.entries, &padded),
            "#EXTM3U\n#EXTINF:-1,x\n../a.mp3\n"
        );
    }

    /// `Span` is `repr(C)`: 12 bytes, fields in declared order.
    #[test]
    fn span_layout_is_fixed() {
        use std::mem::{align_of, offset_of, size_of};
        assert_eq!(size_of::<Span>(), 12);
        assert_eq!(align_of::<Span>(), 4);
        assert_eq!(offset_of!(Span, start), 0);
        assert_eq!(offset_of!(Span, path_len), 4);
        assert_eq!(offset_of!(Span, label_len), 6);
        assert_eq!(offset_of!(Span, meta), 8);
    }

    /// Entries stay small: a track line keeps only its path, and `line()`
    /// writes it back as `../<path>`; outside lines keep their text.
    #[test]
    fn entries_are_compact_and_round_trip() {
        assert_eq!(std::mem::size_of::<Span>(), 12);
        let root = Path::new("/r");
        let e = parse("#EXTINF:5,L\n.././Playlists/../A/./b.mp3\n/x/y.mp3\n", root).unwrap();
        assert_eq!(e.get(0).unwrap().track(), Some("A/b.mp3"));
        assert_eq!(e.get(0).unwrap().line(), "../A/b.mp3");
        assert_eq!(
            (e.get(0).unwrap().duration(), e.get(0).unwrap().label()),
            (Some(5), Some("L"))
        );
        assert_eq!(
            (
                e.get(1).unwrap().track(),
                e.get(1).unwrap().key(),
                &*e.get(1).unwrap().line()
            ),
            (None, "/x/y.mp3", "/x/y.mp3")
        );
        assert_eq!(
            (e.get(1).unwrap().duration(), e.get(1).unwrap().label()),
            (None, None)
        );
        assert_eq!(resolve("../../a", root), None);
        assert_eq!(resolve("..", root), None);
        assert_eq!(resolve("/r/a//b", root).as_deref(), Some("a/b"));
    }

    /// R2-astra-1: a long entry without metadata gets a fallback label cut
    /// like any other, so what is written can be read again; a label read
    /// from the file is cut as well.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn long_entries_without_metadata_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        let line = format!("../{}", "x".repeat(MAX_LINE_BYTES - 3));
        fs::write(root.join("Playlists/Long.m3u8"), format!("{line}\n")).unwrap();
        let p = Playlist::read(root, "Long").unwrap();
        p.write(root, &|_| None).unwrap();
        let text = fs::read_to_string(root.join("Playlists/Long.m3u8")).unwrap();
        assert!(text.lines().all(|l| l.len() <= MAX_LINE_BYTES));
        let again = Playlist::read(root, "Long").unwrap();
        assert_eq!(again.entries.len(), 1);
        assert_eq!(again.entries.get(0).unwrap().line(), line);
        assert_eq!(
            again.entries.get(0).unwrap().label().unwrap().len(),
            MAX_LABEL_BYTES
        );
        let big = format!("#EXTINF:3,{}\n../a.mp3\n", "é".repeat(2_000));
        let e = parse(&big, root).unwrap();
        assert_eq!(e.get(0).unwrap().label().unwrap().len(), MAX_LABEL_BYTES);
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn lists_at_most_max_playlists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        for i in 0..MAX_PLAYLISTS + 5 {
            fs::write(root.join(format!("Playlists/P{i:04}.m3u8")), "").unwrap();
        }
        fs::write(root.join("Playlists/Liked Songs.m3u8"), "").unwrap();
        let names = list_names(root);
        assert_eq!(names.len(), MAX_PLAYLISTS);
        assert_eq!(names[0], "P0000");
    }

    /// Bulk edits are linear: removing or unliking 100,000 entries is quick
    /// (they were quadratic: 11 s for 100,000 unlikes).
    #[test]
    #[cfg_attr(miri, ignore = "too slow under Miri")]
    fn bulk_edits_are_linear() {
        let tracks: Vec<String> = (0..100_000).map(|i| format!("A/B/{i}.mp3")).collect();
        let mut p = Playlist::new(LIKED_SONGS);
        p.append(tracks.iter().map(String::as_str));
        let start = std::time::Instant::now();
        let mut q = p.clone();
        q.remove_positions(&(0..100_000).step_by(2).collect::<Vec<_>>());
        assert_eq!(q.entries.len(), 50_000);
        assert_eq!(q.entries.get(0).unwrap().track(), Some("A/B/1.mp3"));
        p.unlike_all(tracks.iter().skip(1).map(String::as_str));
        assert_eq!(p.entries.len(), 1);
        p.like_all(tracks.iter().map(String::as_str));
        assert_eq!(p.entries.len(), 100_000);
        assert_eq!(p.entries.get(0).unwrap().track(), Some("A/B/1.mp3"));
        assert_eq!(p.entries.get(99_999).unwrap().track(), Some("A/B/0.mp3"));
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    /// The span storage behaves like a plain list under random edits, and
    /// dropping edits reclaim the text of removed entries.
    #[test]
    fn edits_match_a_plain_list() {
        let mut rng = fastrand::Rng::with_seed(3);
        let mut p = Playlist::new("x");
        let mut model: Vec<String> = Vec::new();
        // Miri is far slower: fewer steps keep it to minutes.
        let steps = if cfg!(miri) { 150 } else { 2_000 };
        for step in 0..steps {
            let t = format!("A/{}.mp3", rng.u32(..300));
            match rng.u8(..6) {
                0 => {
                    p.append([t.as_str()]);
                    model.push(t);
                }
                1 => {
                    if !model.contains(&t) {
                        model.insert(0, t.clone());
                    }
                    p.like(&t);
                }
                2 => {
                    model.retain(|m| *m != t);
                    p.unlike(&t);
                }
                3 if !model.is_empty() => {
                    let (from, to) = (rng.usize(..model.len()), rng.usize(..model.len()));
                    let e = model.remove(from);
                    model.insert(to, e);
                    p.move_entry(from, to);
                }
                4 if !model.is_empty() => {
                    let at = rng.usize(..model.len());
                    model.remove(at);
                    p.remove_positions(&[at, at]);
                }
                _ => {
                    let new: Vec<String> = (0..3).map(|k| format!("L/{step}-{k}.mp3")).collect();
                    for n in new.iter().rev() {
                        model.insert(0, n.clone());
                    }
                    p.like_all(new.iter().map(String::as_str));
                }
            }
            let got: Vec<&str> = p.entries.iter().map(|e| e.track().unwrap()).collect();
            assert_eq!(got, model, "step {step}");
            assert!(
                p.entries.text.len() <= 2 * p.entries.live_bytes() + 64,
                "step {step}"
            );
        }
        // Equality compares entries, not the buffers.
        let mut fresh = Playlist::new("x");
        fresh.append(model.iter().map(String::as_str));
        assert_eq!(p, fresh);
    }
}
