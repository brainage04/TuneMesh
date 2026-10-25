//! What sites send the worker (Spotify embed pages, yt-dlp's JSON about a
//! page, yt-dlp progress lines): parsing never panics, Spotify track ids
//! are valid ids, and progress is a fraction.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_worker::spotify::{Kind, next_data, parse_embed};
use tunemesh_worker::ytdlp::{parse_listing, parse_progress};

const ID: &str = "4uLU6hMCjMI75M1A2tKUQC";

fuzz_target!(|s: &str| {
    // The page's JSON, or the input itself as JSON.
    let data = next_data(s).or_else(|| serde_json::from_str(s).ok());
    if let Some(v) = &data {
        for kind in [Kind::Track, Kind::Album, Kind::Playlist] {
            let e = parse_embed(kind, ID, v);
            for t in &e.tracks {
                assert!(t.id.len() == 22 && t.id.bytes().all(|b| b.is_ascii_alphanumeric()));
            }
        }
        let l = parse_listing(v, "https://example.com/x");
        assert!(l.is_playlist || l.items.len() == 1);
        assert!(l.items.iter().all(|i| !i.url.is_empty()));
    }
    for line in s.lines() {
        if let Some(p) = parse_progress(line) {
            assert!((0.0..=1.0).contains(&p), "{line:?} -> {p}");
        }
    }
});
