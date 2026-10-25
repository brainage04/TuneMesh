//! A request's `input` (any synced device writes it): classification,
//! Spotify and YouTube link parsing and the netguard URL split never panic,
//! and the URLs the worker builds from it point where they should.
#![no_main]

use libfuzzer_sys::fuzz_target;
use tunemesh_worker::job::{Input, classify};
use tunemesh_worker::netguard::host_port;
use tunemesh_worker::spotify::parse_url;
use tunemesh_worker::ytdlp::{YouTubeList, music_search_url, percent_encode, youtube_list};

fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

fuzz_target!(|s: &str| {
    match classify(s) {
        Input::Url(u) | Input::Spotify(u) => {
            let l = u.to_ascii_lowercase();
            assert!(
                l.starts_with("http://") || l.starts_with("https://"),
                "{u:?}"
            );
        }
        Input::Search(t) => assert_eq!(t, s.trim()),
    }

    if let Ok((_, id)) = parse_url(s) {
        assert!(id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric()));
    }

    if let Some(YouTubeList::List(u)) = youtube_list(s) {
        // The list URL stays on YouTube whatever the `list=` value holds.
        let (host, port) = host_port(&u).expect("a list URL is a web link");
        assert!(
            matches!(host.as_str(), "www.youtube.com" | "music.youtube.com") && port == 443,
            "{s:?} -> {u:?}"
        );
    }

    if let Some((host, _)) = host_port(s) {
        assert!(!host.is_empty());
        assert!(!host.contains(['/', '?', '#', '@']), "{host:?}");
        assert_eq!(host, host.to_ascii_lowercase());
    }

    let search = music_search_url(s);
    assert_eq!(
        host_port(&search),
        Some(("music.youtube.com".to_owned(), 443))
    );
    let q = percent_encode(s);
    assert!(
        q.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+%".contains(&b))
    );
    assert_eq!(percent_decode(&q), s.as_bytes());
});
