//! What readers keep of each synced file format, measured with a counting
//! allocator (contract: "Limits"; round 3 class fix). Each test builds files
//! with every field and list at or past its limit (`hostile`) and asserts
//! that the memory a reader retains stays within a fixed multiple of the
//! file's size plus a fixed amount, and that the readers of many files never
//! hold more than one file at a time.
//!
//! Allocations are counted per thread (tests run in parallel), each rounded
//! up to 16 bytes plus 16 bytes of allocator overhead, as glibc's malloc
//! roughly does.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::Path;

use crate::hostile;
use crate::limits::{MAX_JSON_BYTES, MAX_LISTED_REQUESTS, MAX_PLAYLIST_ENTRIES};
use crate::playlist::{Playlist, playlist_path, playlists_dir};
use crate::request;

struct Counting;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
}

fn cost(size: usize) -> usize {
    size.next_multiple_of(16) + 16
}

fn add(n: usize) {
    let _ = LIVE.try_with(|l| {
        let v = l.get() + n;
        l.set(v);
        let _ = PEAK.try_with(|p| p.set(p.get().max(v)));
    });
}

fn sub(n: usize) {
    let _ = LIVE.try_with(|l| l.set(l.get().saturating_sub(n)));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(cost(layout.size()));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        sub(cost(layout.size()));
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        add(cost(layout.size()));
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        add(cost(new_size));
        sub(cost(layout.size()));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// Runs `f`; returns its value, the bytes it left allocated (held by the
/// value) and the most it had allocated at once.
fn measure<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let before = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(before));
    let v = f();
    let after = LIVE.with(Cell::get);
    let peak = PEAK.with(Cell::get);
    (v, after.saturating_sub(before), peak - before)
}

/// Asserts `retained <= factor * file + fixed` and reports the numbers.
fn within(what: &str, retained: usize, file: usize, factor: f64, fixed: usize) {
    eprintln!(
        "MEMORY {what}: file {file} B, retained {retained} B ({:.2} x file)",
        retained as f64 / file.max(1) as f64
    );
    assert!(
        retained as f64 <= factor * file as f64 + fixed as f64,
        "{what}: retained {retained} B for a {file} B file (bound {factor} x + {fixed})"
    );
}

fn lib() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["Playlists", ".tunemesh/requests", ".tunemesh/status"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    dir
}

#[test]
#[cfg_attr(miri, ignore = "file system")]
fn request_files() {
    for len in [0, 40_000, MAX_JSON_BYTES as usize - 1024] {
        let json = hostile::request_json("1-00000000", len);
        let (r, retained, _) = measure(|| request::parse_request(json.as_bytes(), "1-00000000"));
        let r = r.unwrap();
        // Every field past its limit: what is kept is the limits' sum.
        within("request", retained, json.len(), 1.0, 12 * 1024);
        drop(r);
    }
}

#[test]
#[cfg_attr(miri, ignore = "file system")]
fn status_files() {
    for (shape, len) in [
        (0, MAX_JSON_BYTES as usize - 1024),
        (1, MAX_JSON_BYTES as usize - 1024),
        (1, 40_000),
        (0, 120_000),
    ] {
        let json = hostile::status_json(shape, len);
        // One status read whole (to play it), and as a list row.
        let (s, retained, _) = measure(|| request::parse_status(json.as_bytes(), "1-00000000"));
        let s = s.unwrap();
        within(
            &format!("status (shape {shape}, read whole)"),
            retained,
            json.len(),
            1.5,
            64 * 1024,
        );
        drop(s);
        let (row, retained, _) =
            measure(|| request::parse_status_row(json.as_bytes(), "1-00000000"));
        let row = row.unwrap();
        within(
            &format!("status (shape {shape}, list row)"),
            retained,
            json.len(),
            0.0,
            20 * 1024,
        );
        drop(row);
    }
}

#[test]
#[cfg_attr(miri, ignore = "file system")]
fn worker_json() {
    let json = hostile::worker_json(MAX_JSON_BYTES as usize - 1024);
    let (w, retained, _) = measure(|| crate::worker::parse(json.as_bytes()));
    assert!(w.is_some());
    within("worker.json", retained, json.len(), 0.0, 2 * 1024);
}

/// The Downloads list: 200 requests and 200 statuses at the file limit
/// (400 MiB of synced files) keep a few MiB, and listing holds one file at
/// a time.
#[test]
#[cfg_attr(miri, ignore = "file system")]
fn download_list_at_the_listed_count() {
    let dir = lib();
    let root = dir.path();
    hostile::max_requests(root, MAX_LISTED_REQUESTS).unwrap();
    hostile::max_statuses(root, MAX_LISTED_REQUESTS).unwrap();
    let ((rows, _), retained, peak) = measure(|| request::recent(root));
    assert_eq!(rows.len(), MAX_LISTED_REQUESTS);
    assert!(
        rows.iter()
            .all(|r| r.request.is_some() && r.status.is_some())
    );
    let files = 2 * MAX_LISTED_REQUESTS * MAX_JSON_BYTES as usize;
    within(
        "download list (200 requests + 200 statuses)",
        retained,
        files,
        0.0,
        8 << 20,
    );
    eprintln!("MEMORY download list: peak {peak} B");
    assert!(
        peak <= retained + 4 * MAX_JSON_BYTES as usize,
        "peak {peak} B"
    );
}

/// R3-Opus-2 / R3Deepseek-1: the worker's sweep held every status file at
/// once (157 MB of small status files got it killed under MemoryMax=2G).
/// Now it reads one at a time: its peak does not grow with the files.
#[test]
#[cfg_attr(miri, ignore = "file system")]
fn worker_sweep_reads_one_status_at_a_time() {
    let dir = lib();
    let root = dir.path();
    let n = 300;
    let mut total = 0;
    for i in 0..n {
        // The tiny-entries shape, unfinished, with no request file.
        let json = hostile::status_json(1, 40_000).replace("\"failed\"", "\"downloading\"");
        total += json.len();
        fs::write(
            request::status_path(root, &format!("{}-00000000", 1_000 + i)),
            json,
        )
        .unwrap();
    }
    let (done, retained, peak) = measure(|| request::sweep(root));
    // One pass changes at most `MAX_SWEEP_CHANGES` of them.
    assert_eq!(
        done.cancelled.len(),
        n.min(crate::limits::MAX_SWEEP_CHANGES)
    );
    eprintln!(
        "MEMORY worker sweep: {n} status files, {total} B, peak {peak} B, retained {retained} B"
    );
    assert!(
        peak < 2 << 20,
        "peak {peak} B for {total} B of status files"
    );
}

#[test]
#[cfg_attr(miri, ignore = "file system")]
fn playlist_files() {
    let dir = lib();
    let root = dir.path();
    // Labels past their limit on every entry, and the most entries of the
    // shortest lines.
    let label = "L".repeat(2 * crate::limits::MAX_LABEL_BYTES);
    let mut long = String::from("#EXTM3U\n");
    for i in 0..10_000 {
        long.push_str(&format!("#EXTINF:{i},{label}\n../A/{i}.mp3\n"));
    }
    let mut short = String::from("#EXTM3U\n");
    for i in 0..MAX_PLAYLIST_ENTRIES {
        short.push_str(&format!("{}\n", i % 10));
    }
    for (name, text) in [("Long", &long), ("Short", &short)] {
        fs::write(playlist_path(root, name), text).unwrap();
        let (p, retained, _) = measure(|| Playlist::read(root, name));
        let p = p.unwrap();
        // 12 bytes a span, plus the path (resolved below the root, so a
        // bare name gains `Playlists/`) and the label cut to 1 KiB.
        within(
            &format!("playlist ({name})"),
            retained,
            text.len(),
            1.0,
            24 * p.entries.len() + 4096,
        );
        drop(p);
    }
    assert!(playlists_dir(root).is_dir());
}

#[test]
#[cfg_attr(miri, ignore = "file system")]
fn stignore_file() {
    let dir = lib();
    let root = dir.path();
    let mut text = String::new();
    while text.len() + 101 < crate::limits::MAX_STIGNORE_BYTES as usize {
        text.push_str(&format!("// {}\n", "s".repeat(97)));
    }
    fs::write(root.join(".stignore"), &text).unwrap();
    let (r, retained, peak) = measure(|| crate::stignore::ensure(Path::new(root)));
    assert!(r.unwrap());
    within(".stignore", retained, text.len(), 0.0, 0);
    eprintln!("MEMORY .stignore: peak {peak} B");
    assert!(peak <= 3 * text.len() + 64 * 1024);
}
