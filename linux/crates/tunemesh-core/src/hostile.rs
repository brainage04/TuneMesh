//! A "hostile" library: everything a device sharing the folder could write
//! to make a reader slow or run out of memory, at or just past the
//! contract's limits (`docs/library-format.md`, "Limits").
//!
//! Used by this crate's stress test and, through
//! `examples/hostile_library.rs`, by the desktop app's GUI stress check.
//! The Android instrumented test builds the same shape on the device.
//! Audio files are minimal MP3s (an ID3v2.4 tag and two silent frames).

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

use crate::limits::{
    MAX_JSON_BYTES, MAX_LABEL_BYTES, MAX_LISTED_REQUESTS, MAX_PLAYLIST_BYTES, MAX_PLAYLIST_ENTRIES,
    MAX_PLAYLIST_LINE_BYTES, MAX_PLAYLISTS, MAX_STIGNORE_BYTES,
};

/// What to generate.
#[derive(Debug, Clone)]
pub struct Options {
    /// Tracks in `Hostile Artist/One Album/` (track numbers shuffled).
    pub album_tracks: usize,
    /// Audio files in total; the rest are albums of 10 under `Artist N/`.
    pub files: usize,
    /// Status files just under 1 MiB (the read limit), every field past
    /// its limit or as many tiny entries as fit, without request files.
    pub statuses: usize,
    /// Request files (URLs the worker refuses), more than it considers.
    pub requests: usize,
    /// Playlists with the maximum number of entries.
    pub big_playlists: usize,
    /// One-entry playlists, more than are listed.
    pub small_playlists: usize,
    /// Files over the limits: 1 MiB tags, 20 MB covers, oversized JSON,
    /// `.stignore` and playlists, a 200-level folder chain.
    pub huge: bool,
}

impl Options {
    /// The full stress library (about 1.2 GB on disk).
    pub fn full() -> Self {
        Self {
            album_tracks: 40_000,
            files: 100_000,
            statuses: 500,
            requests: 600,
            big_playlists: 50,
            small_playlists: MAX_PLAYLISTS + 50,
            huge: true,
        }
    }

    /// A tenth of it, for unit tests (about 120 MB).
    pub fn small() -> Self {
        Self {
            album_tracks: 4_000,
            files: 10_000,
            statuses: 20,
            requests: 600,
            big_playlists: 3,
            small_playlists: MAX_PLAYLISTS + 50,
            huge: true,
        }
    }
}

/// A syncsafe 28-bit integer (ID3v2.4 sizes).
fn syncsafe(n: usize) -> [u8; 4] {
    assert!(n < 1 << 28);
    [
        (n >> 21) as u8 & 0x7f,
        (n >> 14) as u8 & 0x7f,
        (n >> 7) as u8 & 0x7f,
        n as u8 & 0x7f,
    ]
}

fn frame(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&syncsafe(body.len()));
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(body);
}

fn text_frame(out: &mut Vec<u8>, id: &[u8; 4], text: &str) {
    let mut body = vec![3]; // UTF-8
    body.extend_from_slice(text.as_bytes());
    frame(out, id, &body);
}

/// Tags of a generated MP3.
#[derive(Debug, Default, Clone)]
pub struct Tags<'a> {
    pub title: &'a str,
    /// Several artists are written as separate ID3v2.4 values.
    pub artists: &'a [&'a str],
    pub album: &'a str,
    pub track: Option<u32>,
    /// An embedded front cover (`APIC`, `image/jpeg`).
    pub cover: Option<&'a [u8]>,
}

/// A minimal MP3: an ID3v2.4 tag and two silent MPEG-1 Layer III frames.
pub fn mp3(tags: &Tags) -> Vec<u8> {
    let mut body = Vec::new();
    text_frame(&mut body, b"TIT2", tags.title);
    text_frame(&mut body, b"TPE1", &tags.artists.join("\0"));
    text_frame(&mut body, b"TALB", tags.album);
    if let Some(n) = tags.track {
        text_frame(&mut body, b"TRCK", &n.to_string());
    }
    if let Some(cover) = tags.cover {
        let mut apic = vec![0];
        apic.extend_from_slice(b"image/jpeg\0");
        apic.push(3); // front cover
        apic.push(0); // empty description
        apic.extend_from_slice(cover);
        frame(&mut body, b"APIC", &apic);
    }
    let mut out = b"ID3\x04\x00\x00".to_vec();
    out.extend_from_slice(&syncsafe(body.len()));
    out.extend_from_slice(&body);
    for _ in 0..2 {
        // MPEG-1 Layer III, 128 kbit/s, 44.1 kHz: 417-byte frames.
        out.extend_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
        out.resize(out.len() + 413, 0);
    }
    out
}

/// A JPEG that states `w` x `h` pixels in its header, padded to `len`
/// bytes (the pixels are not valid; readers must not get that far).
pub fn jpeg(w: u16, h: u16, len: usize) -> Vec<u8> {
    let mut v = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00".to_vec();
    v.extend_from_slice(b"\xff\xc0\x00\x11\x08");
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    v.resize(len.max(v.len() + 2) - 2, 0);
    v.extend_from_slice(b"\xff\xd9");
    v
}

fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, bytes)
}

/// Writes a playlist line by line.
fn playlist(root: &Path, name: &str, lines: impl Iterator<Item = String>) -> io::Result<()> {
    let mut f = BufWriter::new(File::create(root.join(format!("Playlists/{name}.m3u8")))?);
    f.write_all(b"#EXTM3U\n")?;
    for line in lines {
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
    }
    f.flush()
}

/// Request ids `<ms>-<hex>`: old timestamps, one per index.
fn id(i: usize) -> String {
    format!("{}-{:08x}", 1_700_000_000_000u64 + i as u64, i)
}

/// JSON of `v` with `field` (a list) filled with `item(i)` until the file
/// is about `len` bytes.
fn fill(
    mut v: serde_json::Value,
    field: &str,
    len: usize,
    item: impl Fn(usize) -> serde_json::Value,
) -> String {
    let base = v.to_string().len();
    let one = item(0).to_string().len() + 1;
    let n = len.saturating_sub(base) / one;
    v[field] = (0..n).map(item).collect();
    v.to_string()
}

/// A request file of about `len` bytes with every field longer than its
/// limit (the input most).
pub(crate) fn request_json(id: &str, len: usize) -> String {
    let v = serde_json::json!({
        "version": 1, "id": "i".repeat(1_000), "input": "",
        "playlist": "p".repeat(20_000), "playlistFromSource": true,
        "requestedBy": "d".repeat(20_000), "requestedAt": "t".repeat(20_000),
    });
    let input = len.saturating_sub(v.to_string().len());
    let mut v = v;
    v["input"] = format!("https://127.0.0.1/hostile/{id}/{}", "q".repeat(input)).into();
    v.to_string()
}

/// A status file of about `len` bytes. Even `i`: every text field longer
/// than its limit and every list past its count, filled up with long track
/// paths. Odd `i`: as many tiny entries as fit (the shape that costs a
/// reader the most per byte), filled up with one-character tracks.
pub(crate) fn status_json(i: usize, len: usize) -> String {
    let long = |c: &str, n: usize| c.repeat(n);
    if i.is_multiple_of(2) {
        let v = serde_json::json!({
            "version": 1, "id": long("i", 1_000), "input": long("q", 100_000),
            "state": "done", "title": long("t", 50_000), "progress": 0.5,
            "message": long("m", 100_000), "itemsDone": u32::MAX, "itemsTotal": u32::MAX,
            "warnings": (0..60).map(|n| format!("{n} {}", long("w", 2_000))).collect::<Vec<_>>(),
            "failures": (0..250).map(|_| serde_json::json!({"item": long("f", 1_000), "error": long("e", 1_000)})).collect::<Vec<_>>(),
            "updatedAt": long("u", 50_000), "tracks": [],
        });
        fill(v, "tracks", len, |n| {
            format!("{n:06}{}", long("p", 5_000)).into()
        })
    } else {
        let v = serde_json::json!({
            "version": 1, "input": "q", "state": "failed", "title": "t", "message": "m",
            "warnings": vec!["w"; 80],
            "failures": vec![serde_json::json!({"item": "", "error": ""}); 300],
            "updatedAt": "u", "tracks": [],
        });
        fill(v, "tracks", len, |_| "a".into())
    }
}

/// A `worker.json` of about `len` bytes with every field longer than its
/// limit (a heartbeat that is not fresh).
pub(crate) fn worker_json(len: usize) -> String {
    let v = serde_json::json!({
        "version": 1, "device": "", "machine": "m".repeat(10_000),
        "instance": "i".repeat(10_000), "pid": u32::MAX,
        "workerVersion": "v".repeat(10_000), "heartbeatAt": "h".repeat(10_000),
    });
    let device = len.saturating_sub(v.to_string().len());
    let mut v = v;
    v["device"] = "d".repeat(device).into();
    v.to_string()
}

/// Size of the files `max_requests` and `max_statuses` write: just under
/// the read limit.
pub(crate) const MAX_FILE: usize = MAX_JSON_BYTES as usize - 1024;

/// Writes `n` request files (ids `id(0)`..`id(n)`), each just under the
/// 1 MiB read limit with every field longer than its limit.
pub fn max_requests(root: &Path, n: usize) -> io::Result<()> {
    requests_at(root, 0..n, |_| true)
}

/// Writes `n` status files with the same ids as `max_requests`, each just
/// under the 1 MiB read limit: every field past its limit, or (every other
/// file) as many tiny entries as fit.
pub fn max_statuses(root: &Path, n: usize) -> io::Result<()> {
    statuses_at(root, 0..n)
}

fn requests_at(
    root: &Path,
    ids: std::ops::Range<usize>,
    big: impl Fn(usize) -> bool,
) -> io::Result<()> {
    for i in ids {
        let json = if big(i) {
            request_json(&id(i), MAX_FILE)
        } else {
            request_json(&id(i), 0)
        };
        write(
            &root.join(format!(".tunemesh/requests/{}.json", id(i))),
            json.as_bytes(),
        )?;
    }
    Ok(())
}

fn statuses_at(root: &Path, ids: std::ops::Range<usize>) -> io::Result<()> {
    for i in ids {
        write(
            &root.join(format!(".tunemesh/status/{}.json", id(i))),
            status_json(i, MAX_FILE).as_bytes(),
        )?;
    }
    Ok(())
}

/// The track path at index `i` of the big album.
pub fn album_track(i: usize) -> String {
    format!("Hostile Artist/One Album/{i:05}.mp3")
}

/// Generates the library under `root` (which should be empty).
pub fn generate(root: &Path, o: &Options) -> io::Result<()> {
    fs::create_dir_all(root.join("Playlists"))?;
    fs::create_dir_all(root.join(".tunemesh/requests"))?;
    fs::create_dir_all(root.join(".tunemesh/status"))?;

    // One huge album, track numbers shuffled against path order.
    let mut numbers: Vec<u32> = (1..=o.album_tracks as u32).collect();
    fastrand::Rng::with_seed(1).shuffle(&mut numbers);
    fs::create_dir_all(root.join("Hostile Artist/One Album"))?;
    for (i, &n) in numbers.iter().enumerate() {
        let title = format!("Track {n}");
        let bytes = mp3(&Tags {
            title: &title,
            artists: &["Hostile Artist"],
            album: "One Album",
            track: Some(n),
            cover: None,
        });
        fs::write(root.join(album_track(i)), bytes)?;
    }
    // The other files: albums of 10, 10 albums per artist.
    for i in 0..o.files.saturating_sub(o.album_tracks) {
        let (artist, album) = (
            format!("Artist {:04}", i / 100),
            format!("Album {:02}", i / 10 % 10),
        );
        let title = format!("Song {i}");
        let rel = format!("{artist}/{album}/{:02}.mp3", i % 10);
        let bytes = mp3(&Tags {
            title: &title,
            artists: &[&artist],
            album: &album,
            track: Some((i % 10) as u32 + 1),
            cover: None,
        });
        write(&root.join(rel), &bytes)?;
    }

    // Playlists at the entry limit, and more playlists than are listed.
    for p in 0..o.big_playlists {
        let n = o.album_tracks.max(1);
        playlist(
            root,
            &format!("Big {p:02}"),
            (0..MAX_PLAYLIST_ENTRIES).map(|i| format!("../{}", album_track(i % n))),
        )?;
    }
    for p in 0..o.small_playlists {
        playlist(
            root,
            &format!("Small {p:04}"),
            std::iter::once(format!("../{}", album_track(0))),
        )?;
    }

    // Requests (more than the worker considers; the newest that clients
    // list at the file limit with every field past its limit, the rest
    // smaller) and status files without requests, in both
    // of `status_json`'s shapes.
    let listed = o.requests.saturating_sub(MAX_LISTED_REQUESTS + 10);
    requests_at(root, 0..o.requests, |i| i >= listed)?;
    statuses_at(root, 10_000..10_000 + o.statuses)?;
    // worker.json, .stignore and a playlist of over-long labels: at their
    // limits, every field filled.
    write(
        &root.join(".tunemesh/worker.json"),
        worker_json(MAX_FILE).as_bytes(),
    )?;
    let mut stignore = String::from("// TuneMesh\n(?d).~tunemesh-*\n");
    while stignore.len() + 101 < MAX_STIGNORE_BYTES as usize {
        stignore.push_str(&format!("// {}\n", "s".repeat(97)));
    }
    write(&root.join(".stignore"), stignore.as_bytes())?;
    let label = "L".repeat(2 * MAX_LABEL_BYTES);
    let entries = MAX_PLAYLIST_BYTES as usize / (label.len() + 60);
    playlist(
        root,
        "Long Labels",
        (0..entries).map(|i| {
            format!(
                "#EXTINF:{i},{label}\n../{}",
                album_track(i % o.album_tracks.max(1))
            )
        }),
    )?;

    if o.huge {
        huge(root)?;
    }
    Ok(())
}

/// The files past each limit.
fn huge(root: &Path) -> io::Result<()> {
    let mib = 1024 * 1024;
    // Tags: 1 MiB text fields and 40 artists; covers: 20 MB embedded and
    // folder covers stating 20,000 x 20,000 pixels; a small valid cover.
    let big = "T".repeat(mib);
    let artists: Vec<String> = (0..40).map(|i| format!("Artist {i}")).collect();
    let artist_refs: Vec<&str> = artists.iter().map(String::as_str).collect();
    write(
        &root.join("Huge/Tags/big tag.mp3"),
        &mp3(&Tags {
            title: &big,
            artists: &artist_refs,
            album: &big,
            track: Some(1),
            cover: None,
        }),
    )?;
    let big_cover = jpeg(20_000, 20_000, 20_000_000);
    write(
        &root.join("Huge/Cover/big cover.mp3"),
        &mp3(&Tags {
            title: "Big cover",
            artists: &["Huge"],
            album: "Cover",
            track: Some(1),
            cover: Some(&big_cover),
        }),
    )?;
    write(&root.join("Huge/Cover/cover.jpg"), &big_cover)?;
    let small_cover = jpeg(500, 500, 4096);
    write(
        &root.join("Huge/Small Cover/small cover.mp3"),
        &mp3(&Tags {
            title: "Small cover",
            artists: &["Huge"],
            album: "Small Cover",
            track: Some(1),
            cover: Some(&small_cover),
        }),
    )?;

    // Folders deeper than scanned: a track at level 10 and at level 200.
    let mut deep = root.join("Deep");
    for level in 1..=200 {
        deep.push(format!("d{level:03}"));
        let title = format!("Level {level}");
        if level == 10 || level == 200 {
            let tags = Tags {
                title: &title,
                artists: &["Deep"],
                album: "Deep",
                track: None,
                cover: None,
            };
            write(&deep.join(format!("level {level}.mp3")), &mp3(&tags))?;
        }
    }

    // Playlists over each limit, and the long entry a writer must keep
    // readable (R2-astra-1).
    let n = MAX_PLAYLIST_ENTRIES + 1;
    playlist(
        root,
        "Over Entries",
        (0..n).map(|i| format!("../{}", album_track(i % 10))),
    )?;
    playlist(
        root,
        "Over Line",
        std::iter::once(format!("../{}", "x".repeat(MAX_PLAYLIST_LINE_BYTES))),
    )?;
    let label = "L".repeat(MAX_PLAYLIST_LINE_BYTES - 20);
    let lines = (MAX_PLAYLIST_BYTES as usize / label.len()) + 10;
    playlist(
        root,
        "Over Bytes",
        (0..lines).map(|_| format!("#EXTINF:1,{label}")),
    )?;
    playlist(
        root,
        "Long Entry",
        std::iter::once(format!("../{}", "x".repeat(MAX_PLAYLIST_LINE_BYTES - 3))),
    )?;

    // JSON and .stignore past 1 MiB, and an input past 8 KiB.
    let pad = "p".repeat(2 * mib);
    let json = |extra: serde_json::Value| extra.to_string();
    write(
        &root.join(format!(".tunemesh/requests/{}.json", id(900_000))),
        json(serde_json::json!({"version": 1, "id": id(900_000), "input": pad})).as_bytes(),
    )?;
    write(
        &root.join(format!(".tunemesh/requests/{}.json", id(900_001))),
        json(serde_json::json!({"version": 1, "id": id(900_001), "input": "q".repeat(9 * 1024)}))
            .as_bytes(),
    )?;
    write(
        &root.join(format!(".tunemesh/status/{}.json", id(900_002))),
        status_json(0, 2 * mib).as_bytes(),
    )?;
    write(
        &root.join(".stignore"),
        format!("// TuneMesh\n(?d).~tunemesh-*\n//{pad}\n").as_bytes(),
    )?;
    Ok(())
}

/// What `check` measured.
#[derive(Debug, Clone)]
pub struct Report {
    pub tracks: usize,
    pub albums: usize,
    pub playlists: usize,
    pub too_large: usize,
    pub downloads: usize,
    pub scan: std::time::Duration,
    pub library: std::time::Duration,
    pub playlists_time: std::time::Duration,
    pub downloads_time: std::time::Duration,
    /// Peak resident memory of this process (`VmHWM`), in KiB.
    pub peak_rss_kib: u64,
}

/// Peak resident memory of this process, in KiB (0 if unknown).
pub fn peak_rss_kib() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("VmHWM:"))
                .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        })
        .unwrap_or(0)
}

/// Does what a client does on start with `root`: scan, build the library,
/// summarise every listed playlist, list the recent downloads.
pub fn check(root: &Path) -> Report {
    use std::time::Instant;
    let t = Instant::now();
    let tracks = crate::library::scan(root, &std::collections::HashMap::new());
    let scan = t.elapsed();
    let t = Instant::now();
    let lib = crate::library::Library::new(tracks);
    let library = t.elapsed();
    let t = Instant::now();
    let names = crate::playlist::list_names(root);
    let mut too_large = 0;
    for name in &names {
        if let Err(e) = crate::playlist::Playlist::summary(root, name, 8) {
            too_large += usize::from(crate::playlist::is_too_large(&e));
        }
    }
    let playlists_time = t.elapsed();
    let t = Instant::now();
    let (downloads, _) = crate::request::recent(root);
    let _ = crate::worker::read(root);
    let downloads_time = t.elapsed();
    Report {
        tracks: lib.tracks.len(),
        albums: lib.albums.len(),
        playlists: names.len(),
        too_large,
        downloads: downloads.len(),
        scan,
        library,
        playlists_time,
        downloads_time,
        peak_rss_kib: peak_rss_kib(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{read_embedded_cover, read_tags};
    use crate::limits::{MAX_ARTISTS, MAX_TAG_TEXT_BYTES};

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn tags_and_covers_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        for sub in ["Playlists", ".tunemesh/requests", ".tunemesh/status"] {
            fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        huge(dir.path()).unwrap();
        let big = read_tags(&dir.path().join("Huge/Tags/big tag.mp3"));
        assert_eq!(
            big.title.as_ref().map(String::len),
            Some(MAX_TAG_TEXT_BYTES)
        );
        assert_eq!(
            big.album.as_ref().map(String::len),
            Some(MAX_TAG_TEXT_BYTES)
        );
        assert_eq!(big.artists.len(), MAX_ARTISTS);
        // A 20 MB cover is not used; the other tags are kept.
        let cover = dir.path().join("Huge/Cover/big cover.mp3");
        let info = read_tags(&cover);
        assert_eq!(info.title.as_deref(), Some("Big cover"));
        assert!(!info.has_picture);
        assert!(read_embedded_cover(&cover).is_none());
        let small = dir.path().join("Huge/Small Cover/small cover.mp3");
        assert!(read_tags(&small).has_picture);
        assert!(read_embedded_cover(&small).is_some());
    }

    /// Generates a library, then runs `check` on it in a child process (so
    /// the peak memory measured is the client's alone) and returns the
    /// child's report line.
    fn check_in_child(options: &Options) -> String {
        // Disk-backed /var/tmp when there is one (the full library is
        // about 1.2 GB, too much for a RAM-backed /tmp), else the system
        // temporary directory ($TMPDIR; the Nix build sandbox has no
        // /var/tmp).
        let base = std::env::var_os("TUNEMESH_HOSTILE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let var_tmp = Path::new("/var/tmp");
                if var_tmp.is_dir() {
                    var_tmp.to_owned()
                } else {
                    std::env::temp_dir()
                }
            });
        let dir = tempfile::tempdir_in(base).unwrap();
        let root = dir.path().join("lib");
        generate(&root, options).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "hostile::tests::child",
                "--nocapture",
                "--include-ignored",
            ])
            .env("TUNEMESH_HOSTILE_ROOT", &root)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
            .lines()
            .find(|l| l.starts_with("REPORT "))
            .unwrap_or_else(|| panic!("no report: {stdout}"))
            .to_owned()
    }

    /// The child half of `check_in_child`.
    #[test]
    #[ignore = "run by check_in_child"]
    fn child() {
        let Some(root) = std::env::var_os("TUNEMESH_HOSTILE_ROOT") else {
            return;
        };
        let r = check(Path::new(&root));
        let secs = |d: std::time::Duration| d.as_secs_f64();
        println!(
            "REPORT tracks={} albums={} playlists={} too_large={} downloads={} scan={:.3} library={:.3} playlists_s={:.3} downloads_s={:.3} rss_mib={}",
            r.tracks,
            r.albums,
            r.playlists,
            r.too_large,
            r.downloads,
            secs(r.scan),
            secs(r.library),
            secs(r.playlists_time),
            secs(r.downloads_time),
            r.peak_rss_kib / 1024
        );
    }

    fn field(report: &str, key: &str) -> f64 {
        report
            .split_whitespace()
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{key} missing in {report}"))
    }

    /// Asserts the limits hold and every step stays under `time_bound`
    /// seconds and the client under `rss_bound_mib`.
    fn assert_bounded(report: &str, o: &Options, time_bound: f64, rss_bound_mib: f64) {
        eprintln!("{report}");
        // Every generated audio file but the one at level 200.
        assert_eq!(
            field(report, "tracks") as usize,
            o.files.max(o.album_tracks) + 4
        );
        assert_eq!(field(report, "playlists") as usize, MAX_PLAYLISTS);
        assert_eq!(field(report, "too_large"), 3.0);
        assert_eq!(
            field(report, "downloads") as usize,
            crate::limits::MAX_LISTED_REQUESTS
        );
        for key in ["scan", "library", "playlists_s", "downloads_s"] {
            assert!(
                field(report, key) < time_bound,
                "{key} took too long: {report}"
            );
        }
        assert!(
            field(report, "rss_mib") < rss_bound_mib,
            "too much memory: {report}"
        );
    }

    /// The small hostile library: every limit applies and a client's start
    /// stays within seconds and a few hundred MiB.
    #[test]
    #[cfg_attr(miri, ignore = "child process")]
    fn hostile_library_stays_bounded() {
        let report = check_in_child(&Options::small());
        // Measured: debug 0.4 s per step and 29 MiB, release 0.15 s.
        let slow = if cfg!(debug_assertions) { 10.0 } else { 3.0 };
        assert_bounded(&report, &Options::small(), slow, 200.0);
    }

    /// The full hostile library (about 1.2 GB of disk; run with
    /// `cargo test --release -p tunemesh-core -- --ignored hostile_full`).
    #[test]
    #[ignore = "needs about 1.2 GB of disk and a release build"]
    fn hostile_full() {
        let report = check_in_child(&Options::full());
        // Measured (release): scan 2.6 s, library 0.11 s, playlists 0.5 s,
        // downloads 0.27 s, 142 MiB.
        assert_bounded(&report, &Options::full(), 15.0, 400.0);
    }
}
