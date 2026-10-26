//! Merging playlist conflict copies (contract: "Playlist conflict copies").
//!
//! When two devices change one playlist before Syncthing syncs either
//! change, Syncthing keeps one version and renames the other to
//! `<Name>.sync-conflict-<date>-<time>-<ID7>.m3u8`. `merge_conflicts` merges
//! such copies back with a three-way merge against this device's base
//! (`Bases`), so neither edit is lost and removals stay removed.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::fsx::Kind;
use crate::limits::MAX_PLAYLIST_BYTES;
use crate::playlist::{self, Entries, Playlist, PlaylistEntry, serialize};

/// A copy whose name is not this device's short ID is merged once it is
/// this old.
pub const MERGE_AFTER: Duration = Duration::from_secs(10 * 60);

/// FNV-1a, 64 bits: a stable hash for names and own-write markers.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// This device's bases of one library's playlists: the version each
/// playlist had before this device's own edits since it last saw another
/// device's version (contract: "Playlist conflict copies", Base). Kept
/// outside the library, never synced: `<state dir>/playlist-base/<library>/`
/// with `<Name>.m3u8` (the base) and `<Name>.own` (a hash of what this
/// device wrote last). Errors are ignored: without a base, a merge falls
/// back to a union.
#[derive(Debug, Clone)]
pub struct Bases {
    dir: Option<PathBuf>,
    root: PathBuf,
}

impl Bases {
    /// Bases under `$XDG_STATE_HOME/tunemesh` (default
    /// `~/.local/state/tunemesh`), shared by the desktop app and a worker of
    /// the same user; none when neither variable is set.
    pub fn new(root: &Path) -> Self {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|h| h.join(".local/state"))
            });
        match state {
            Some(s) => Self::in_dir(&s.join("tunemesh"), root),
            None => Self::none(),
        }
    }

    /// Bases under `state_dir` (the app's per-device state directory).
    pub fn in_dir(state_dir: &Path, root: &Path) -> Self {
        let library = format!("{:016x}", fnv(root.as_os_str().as_encoded_bytes()));
        Self {
            dir: Some(state_dir.join("playlist-base").join(library)),
            root: root.to_owned(),
        }
    }

    /// No bases: every merge falls back to a union.
    pub fn none() -> Self {
        Self {
            dir: None,
            root: PathBuf::new(),
        }
    }

    fn path(&self, name: &str, ext: &str) -> Option<PathBuf> {
        // Playlist names are file names (no `/`), as in `Playlists/`.
        if name.is_empty() || name.contains('/') || name.starts_with('.') {
            return None;
        }
        Some(self.dir.as_ref()?.join(format!("{name}.{ext}")))
    }

    fn read_limited(path: &Path, max: u64) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        File::open(path)
            .ok()?
            .take(max + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= max).then_some(bytes)
    }

    fn put(path: Option<PathBuf>, bytes: &[u8]) {
        if let Some(p) = path
            && let Err(e) = crate::atomic::write(&p, bytes)
        {
            eprintln!("tunemesh: keeping playlist base {}: {e}", p.display());
        }
    }

    /// The base of `name`, read with the playlist limits.
    pub fn base(&self, name: &str) -> Option<Entries> {
        let bytes = Self::read_limited(&self.path(name, "m3u8")?, MAX_PLAYLIST_BYTES)?;
        playlist::parse_bytes(&bytes, &self.root).ok()
    }

    fn own(&self, name: &str) -> Option<u64> {
        let bytes = Self::read_limited(&self.path(name, "own")?, 16)?;
        u64::from_str_radix(std::str::from_utf8(&bytes).ok()?, 16).ok()
    }

    /// Before an edit of `name`, which re-read `entries`: they become the
    /// base unless they are exactly what this device wrote last.
    pub(crate) fn before_edit(&self, name: &str, entries: &Entries) {
        if self.dir.is_none() {
            return;
        }
        let text = serialize(entries, &|_| None);
        if self.own(name) != Some(fnv(text.as_bytes())) {
            Self::put(self.path(name, "m3u8"), text.as_bytes());
        }
    }

    /// After this device wrote `text` as `name`.
    pub(crate) fn wrote(&self, name: &str, text: &str) {
        Self::put(
            self.path(name, "own"),
            format!("{:016x}", fnv(text.as_bytes())).as_bytes(),
        );
    }

    /// After a merge wrote `text`: it is the new base.
    fn merged(&self, name: &str, text: &str) {
        Self::put(self.path(name, "m3u8"), text.as_bytes());
        self.wrote(name, text);
    }

    pub(crate) fn rename(&self, old: &str, new: &str) {
        for ext in ["m3u8", "own"] {
            if let (Some(from), Some(to)) = (self.path(old, ext), self.path(new, ext)) {
                let _ = std::fs::rename(from, to);
            }
        }
    }

    pub(crate) fn remove(&self, name: &str) {
        for ext in ["m3u8", "own"] {
            if let Some(p) = self.path(name, ext) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// How many entries have each key.
fn counts(e: &Entries) -> HashMap<&str, usize> {
    let mut m = HashMap::new();
    for x in e.iter() {
        *m.entry(x.key()).or_insert(0) += 1;
    }
    m
}

/// The three-way merge of the contract (steps 1 to 3): `winner` and `copy`
/// merged against `base` (`None`: no base, a union).
pub fn merge(base: Option<&Entries>, winner: &Entries, copy: &Entries) -> Entries {
    let (cw, cc) = (counts(winner), counts(copy));
    let cb = base.map(counts);
    let get = |m: &HashMap<&str, usize>, k: &str| m.get(k).copied().unwrap_or(0);
    // Step 1: how many entries each key gets.
    let wanted = |k: &str| {
        let (w, c) = (get(&cw, k), get(&cc, k));
        let b = cb.as_ref().map_or(0, |b| get(b, k));
        if w >= b && c >= b {
            w.max(c)
        } else if w <= b && c <= b {
            w.min(c)
        } else {
            (w + c).saturating_sub(b)
        }
    };
    // Step 2: whether a side changed the order of the keys whose count it
    // kept.
    let reordered = |side: &Entries, cs: &HashMap<&str, usize>| {
        let (Some(base), Some(cb)) = (base, &cb) else {
            return false;
        };
        let same = |k: &&str| get(cs, k) == get(cb, k);
        let ours = side.iter().map(PlaylistEntry::key).filter(same);
        let theirs = base.iter().map(PlaylistEntry::key).filter(same);
        !ours.eq(theirs)
    };
    let from_copy = reordered(copy, &cc) && !reordered(winner, &cw);
    let (start, other) = if from_copy {
        (copy, winner)
    } else {
        (winner, copy)
    };
    let mut have: HashMap<&str, usize> = HashMap::new();
    // Kept entries, and per key the positions of its kept entries.
    let mut kept: Vec<PlaylistEntry<'_>> = Vec::new();
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for e in start.iter() {
        let n = have.entry(e.key()).or_insert(0);
        if *n < wanted(e.key()) {
            *n += 1;
            positions.entry(e.key()).or_default().push(kept.len());
            kept.push(e);
        }
    }
    // Step 3: entries still missing, from the other side, each after its
    // anchor. Slot 0 is the top, slot i + 1 is after kept entry i.
    let mut inserted: Vec<(usize, PlaylistEntry<'_>)> = Vec::new();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut anchor = 0;
    for e in other.iter() {
        let j = seen.entry(e.key()).or_insert(0);
        let matched = positions.get(e.key()).and_then(|p| p.get(*j)).copied();
        *j += 1;
        if let Some(pos) = matched {
            anchor = pos + 1;
        } else {
            let n = have.entry(e.key()).or_insert(0);
            if *n < wanted(e.key()) {
                *n += 1;
                inserted.push((anchor, e));
            }
        }
    }
    // Stable: entries inserted at one slot keep their order.
    inserted.sort_by_key(|(slot, _)| *slot);
    let mut out = Entries::default();
    let mut ins = inserted.into_iter().peekable();
    for slot in 0..=kept.len() {
        while let Some((_, e)) = ins.next_if(|(s, _)| *s == slot) {
            out.push_entry(e);
        }
        if let Some(e) = kept.get(slot) {
            out.push_entry(*e);
        }
    }
    out.shrink_to_fit();
    out
}

/// The text a merge writes: the writers' format from the entries' own
/// `#EXTINF` data, without looking anything up, so every device writes the
/// same bytes.
pub fn merged_text(entries: &Entries) -> String {
    serialize(entries, &|_| None)
}

/// A conflict copy's playlist name and the short device ID in its name, if
/// `file` is exactly `<Name>.sync-conflict-<8 digits>-<6 digits>-<ID7>.m3u8`.
pub fn conflict_copy(file: &str) -> Option<(&str, &str)> {
    let rest = file.strip_suffix(".m3u8")?;
    let (name, suffix) = rest.rsplit_once(".sync-conflict-")?;
    let b = suffix.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let ok = !name.is_empty()
        && b.len() == 8 + 1 + 6 + 1 + 7
        && digits(0..8)
        && b[8] == b'-'
        && digits(9..15)
        && b[15] == b'-'
        && b[16..]
            .iter()
            .all(|c| c.is_ascii_uppercase() || (b'2'..=b'7').contains(c));
    ok.then(|| (name, &suffix[16..]))
}

/// Merges the conflict copies in `Playlists/` that are due (contract: "Who
/// merges"): at once those naming `own_device_id`'s short form (the first 7
/// characters of this device's Syncthing ID, when known; Syncthing names a
/// copy after the device that made the winning version), others once their
/// modification time is `MERGE_AFTER` before `now`. Each playlist is merged
/// under the `Playlists/` lock; returns the playlists merged into, each with
/// its result (an error leaves its files as they were).
pub fn merge_conflicts(
    root: &Path,
    bases: &Bases,
    own_device_id: Option<&str>,
    now: SystemTime,
) -> Vec<(String, io::Result<()>)> {
    let Ok(dir) = playlist::open_dir(root, false) else {
        return Vec::new();
    };
    let own = own_device_id.and_then(|id| id.get(..7));
    let old_enough = |file: &str| {
        dir.metadata(file).is_ok_and(|m| {
            let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(m.mtime.max(0) as u64);
            now.duration_since(mtime)
                .is_ok_and(|age| age >= MERGE_AFTER)
        })
    };
    // Due copies by playlist name, each list in byte order.
    let mut due: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, kind) in dir.entries().unwrap_or_default() {
        if kind != Kind::File {
            continue;
        }
        let Some((name, id)) = conflict_copy(&file) else {
            continue;
        };
        if own == Some(id) || old_enough(&file) {
            due.entry(name.to_owned()).or_default().push(file.clone());
        }
    }
    due.into_iter()
        .map(|(name, mut copies)| {
            copies.sort();
            let result = merge_copies(root, bases, &name, &copies);
            (name, result)
        })
        .collect()
}

/// Merges `copies` (file names, in order) into the playlist `name`.
fn merge_copies(root: &Path, bases: &Bases, name: &str, copies: &[String]) -> io::Result<()> {
    let _lock = playlist::lock(root)?;
    let dir = playlist::open_dir(root, false)?;
    if !dir.is_file(&format!("{name}.m3u8")) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("“{name}” no longer exists"),
        ));
    }
    let base = bases.base(name);
    let mut last = None;
    for file in copies {
        // Merged meanwhile by another process on this machine.
        if !dir.is_file(file) {
            continue;
        }
        let copy_name = &file[..file.len() - ".m3u8".len()];
        let winner = Playlist::read(root, name)?;
        let copy = Playlist::read(root, copy_name)?;
        let merged = Playlist {
            name: name.to_owned(),
            entries: merge(base.as_ref(), &winner.entries, &copy.entries),
        };
        let text = merged.write_text(root, &|_| None)?;
        dir.remove_file(file)?;
        last = Some(text);
    }
    if let Some(text) = last {
        bases.merged(name, &text);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn vectors() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/test-vectors/playlist-merge")
    }

    fn entries(bytes: &[u8]) -> Entries {
        playlist::parse_bytes(bytes, Path::new("/r")).unwrap()
    }

    /// The shared cases in `docs/test-vectors/playlist-merge/`.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn shared_test_vectors() {
        let mut cases = 0;
        for case in fs::read_dir(vectors()).unwrap() {
            let case = case.unwrap().path();
            if !case.is_dir() {
                continue;
            }
            let read = |f: &str| fs::read(case.join(f)).ok().map(|b| entries(&b));
            let base = read("base.m3u8");
            let merged = merge(
                base.as_ref(),
                &read("winner.m3u8").unwrap(),
                &read("copy.m3u8").unwrap(),
            );
            let expected = fs::read_to_string(case.join("merged.m3u8")).unwrap();
            assert_eq!(merged_text(&merged), expected, "{}", case.display());
            cases += 1;
        }
        assert!(cases >= 8);
    }

    fn lines(keys: &str) -> String {
        let mut s = String::from("#EXTM3U\n");
        for k in keys.chars() {
            s.push_str(&format!("#EXTINF:1,{k}\n../{k}.mp3\n"));
        }
        s
    }

    fn keys(e: &Entries) -> String {
        e.iter()
            .map(|x| x.key().trim_start_matches("Playlists/../"))
            .map(|k| k.trim_end_matches(".mp3"))
            .collect()
    }

    fn m(base: Option<&str>, w: &str, c: &str) -> String {
        let base = base.map(|b| entries(lines(b).as_bytes()));
        keys(&merge(
            base.as_ref(),
            &entries(lines(w).as_bytes()),
            &entries(lines(c).as_bytes()),
        ))
    }

    /// The cases the owner asked for, with either side as the winner.
    #[test]
    fn removals_win_and_both_sides_count() {
        // Unlike on A, like on B.
        assert_eq!(m(Some("abc"), "ac", "dabc"), "dac");
        assert_eq!(m(Some("abc"), "dabc", "ac"), "dac");
        // Removal on A, reorder on B.
        assert_eq!(m(Some("abcd"), "acd", "dabc"), "dac");
        assert_eq!(m(Some("abcd"), "dabc", "acd"), "dac");
        // Both add, different tracks and the same one.
        assert_eq!(m(Some("ab"), "abx", "ayb"), "aybx");
        assert_eq!(m(Some("ab"), "xab", "axb"), "xab");
        // One removes a duplicate, the other adds one more.
        assert_eq!(m(Some("aab"), "ab", "aaba"), "aab");
        // No base: a union, so a removal can come back.
        assert_eq!(m(None, "ax", "aby"), "abyx");
        // Nothing changed on one side: the other side's version.
        assert_eq!(m(Some("abc"), "abc", "cb"), "cb");
        assert_eq!(m(Some("abc"), "cb", "abc"), "cb");
        // Merging twice gives the same result.
        let once = m(Some("abc"), "ac", "dabc");
        assert_eq!(m(Some("abc"), &once, "dabc"), once);
    }

    #[test]
    fn conflict_copy_names() {
        assert_eq!(
            conflict_copy("Liked Songs.sync-conflict-20261019-101500-ABCDE27.m3u8"),
            Some(("Liked Songs", "ABCDE27"))
        );
        for bad in [
            ".sync-conflict-20261019-101500-ABCDE27.m3u8",
            "A.sync-conflict-20261019-101500-ABCDE2.m3u8",
            "A.sync-conflict-20261019-101500-abcde27.m3u8",
            "A.sync-conflict-2026101-101500-ABCDE27.m3u8",
            "A.sync-conflict-20261019-101500-ABCDE18.m3u8",
            "A.sync-conflict-20261019-101500-ABCDE27.m3u",
            "A.m3u8",
        ] {
            assert_eq!(conflict_copy(bad), None, "{bad}");
        }
    }

    fn write(root: &Path, file: &str, text: &str) {
        fs::create_dir_all(playlist::playlists_dir(root)).unwrap();
        fs::write(playlist::playlists_dir(root).join(file), text).unwrap();
    }

    fn read_keys(root: &Path, name: &str) -> String {
        keys(&Playlist::read(root, name).unwrap().entries)
    }

    /// The whole flow on two "devices": both edit from the same version,
    /// the copy is merged by the device it names, with its base.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn merges_due_copies_with_the_base() {
        let lib = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let root = lib.path();
        let bases = Bases::in_dir(state.path(), root);
        write(root, "Liked Songs.m3u8", &lines("abc"));
        // This device (B) likes d; its base becomes "abc".
        Playlist::edit(root, &bases, "Liked Songs", &|_| None, |p| {
            p.like("d.mp3");
            Ok(())
        })
        .unwrap();
        assert_eq!(keys(&bases.base("Liked Songs").unwrap()), "abc");
        // A second own edit keeps that base.
        Playlist::edit(root, &bases, "Liked Songs", &|_| None, |_| Ok(())).unwrap();
        assert_eq!(keys(&bases.base("Liked Songs").unwrap()), "abc");
        // Syncthing: A's unlike of b won, B's version became the copy.
        let ours = fs::read_to_string(playlist::playlist_path(root, "Liked Songs")).unwrap();
        write(
            root,
            "Liked Songs.sync-conflict-20261019-101500-BBBBBBB.m3u8",
            &ours,
        );
        write(root, "Liked Songs.m3u8", &lines("ac"));
        let now = SystemTime::now();
        // Another device's ID, and the copy is new: not due yet.
        assert!(merge_conflicts(root, &bases, Some("CCCCCCC-XXXX"), now).is_empty());
        let done = merge_conflicts(root, &bases, Some("BBBBBBB-XXXXXXX"), now);
        assert_eq!(done.len(), 1);
        assert!(done[0].1.is_ok());
        assert_eq!(read_keys(root, "Liked Songs"), "dac");
        assert_eq!(playlist::list_names(root), Vec::<String>::new());
        assert_eq!(
            fs::read_dir(playlist::playlists_dir(root)).unwrap().count(),
            1
        );
        // The merged playlist is the new base.
        assert_eq!(keys(&bases.base("Liked Songs").unwrap()), "dac");
        // Old enough: merged by any device.
        write(root, "P.m3u8", &lines("ab"));
        write(
            root,
            "P.sync-conflict-20261019-101500-DDDDDDD.m3u8",
            &lines("abx"),
        );
        let later = now + MERGE_AFTER + Duration::from_secs(60);
        let done = merge_conflicts(root, &Bases::none(), None, later);
        assert_eq!(done.len(), 1);
        assert_eq!(read_keys(root, "P"), "abx");
    }

    /// Two mergers on one machine: the lock makes the second find the copy
    /// gone, so it neither fails nor writes.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn concurrent_merges_merge_once() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        write(root, "P.m3u8", &lines("ab"));
        write(
            root,
            "P.sync-conflict-20261019-101500-AAAAAAA.m3u8",
            &lines("abx"),
        );
        let results = std::thread::scope(|s| {
            let run = || merge_conflicts(root, &Bases::none(), Some("AAAAAAA"), SystemTime::now());
            let (t1, t2) = (s.spawn(run), s.spawn(run));
            [t1.join().unwrap(), t2.join().unwrap()]
        });
        for r in results.iter().flatten() {
            assert!(r.1.is_ok(), "{:?}", r.1);
        }
        assert_eq!(read_keys(root, "P"), "abx");
        assert_eq!(
            fs::read_dir(playlist::playlists_dir(root)).unwrap().count(),
            1
        );
    }

    /// A merge that would be too large, or a copy without its playlist,
    /// leaves the files alone.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn refused_merges_leave_files_alone() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let copy = "Gone.sync-conflict-20261019-101500-AAAAAAA.m3u8";
        write(root, copy, &lines("a"));
        let done = merge_conflicts(root, &Bases::none(), Some("AAAAAAA"), SystemTime::now());
        assert!(done[0].1.is_err());
        assert!(playlist::playlists_dir(root).join(copy).exists());

        let n = crate::limits::MAX_PLAYLIST_ENTRIES / 2 + 1;
        let many = |prefix: &str| {
            let mut s = String::from("#EXTM3U\n");
            for i in 0..n {
                s.push_str(&format!("../{prefix}{i}.mp3\n"));
            }
            s
        };
        write(root, "Big.m3u8", &many("w"));
        let copy = "Big.sync-conflict-20261019-101500-AAAAAAA.m3u8";
        write(root, copy, &many("c"));
        let done = merge_conflicts(root, &Bases::none(), Some("AAAAAAA"), SystemTime::now());
        assert!(playlist::is_too_large(done[0].1.as_ref().unwrap_err()));
        assert!(playlist::playlists_dir(root).join(copy).exists());
        assert_eq!(Playlist::read(root, "Big").unwrap().entries.len(), n);
    }
}
