//! Heap cost of a read playlist, per entry: `cargo run --release --example
//! playlist_mem -- [entries] [extinf]`. Writes `#EXTM3U` and `entries`
//! lines of `../missing.wav` (with `extinf`: each preceded by
//! `#EXTINF:-1,Artist - Title`) to a temporary library, reads it with
//! `Playlist::read` and prints the live and peak heap the read adds:
//! requested bytes, and the same rounded up to glibc malloc chunks (16-byte
//! steps, 32 bytes minimum, 8 bytes of header), which is what RSS sees.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use tunemesh_core::playlist::Playlist;

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIVE_CHUNKS: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn chunk(size: usize) -> usize {
    ((size + 8 + 15) & !15).max(32)
}

fn add(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    LIVE_CHUNKS.fetch_add(chunk(size), Relaxed);
    PEAK.fetch_max(live, Relaxed);
}

fn sub(size: usize) {
    LIVE.fetch_sub(size, Relaxed);
    LIVE_CHUNKS.fetch_sub(chunk(size), Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        sub(layout.size());
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        sub(layout.size());
        add(new);
        unsafe { System.realloc(ptr, layout, new) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

fn main() {
    let mut args = std::env::args().skip(1);
    let n: usize = args.next().map_or(250_000, |a| a.parse().unwrap());
    let extinf = args.next().is_some_and(|a| a == "extinf");
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("Playlists")).unwrap();
    let mut text = String::from("#EXTM3U\n");
    for _ in 0..n {
        if extinf {
            text.push_str("#EXTINF:-1,Artist - Title\n");
        }
        text.push_str("../missing.wav\n");
    }
    std::fs::write(dir.path().join("Playlists/Big.m3u8"), &text).unwrap();
    drop(text);

    let (base, base_chunks) = (LIVE.load(Relaxed), LIVE_CHUNKS.load(Relaxed));
    PEAK.store(base, Relaxed);
    let p = Playlist::read(dir.path(), "Big").unwrap();
    let live = LIVE.load(Relaxed) - base;
    let chunks = LIVE_CHUNKS.load(Relaxed) - base_chunks;
    let peak = PEAK.load(Relaxed) - base;
    assert_eq!(p.entries.len(), n);
    let per = |b: usize| b as f64 / n as f64;
    println!(
        "{n} entries{}: live {:.1} B/entry requested, {:.1} B/entry in malloc chunks; peak {:.1} B/entry ({:.1} MB)",
        if extinf { " with #EXTINF" } else { "" },
        per(live),
        per(chunks),
        per(peak),
        peak as f64 / 1e6
    );
}
