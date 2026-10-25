//! A synced playlist file: parsing never panics, stays in the limits, and
//! what a writer writes for the entries reads back as the same entries.
#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;
use tunemesh_core::limits::{
    MAX_LABEL_BYTES, MAX_PLAYLIST_BYTES, MAX_PLAYLIST_ENTRIES, MAX_PLAYLIST_LINE_BYTES,
};
use tunemesh_core::playlist::{parse, parse_bytes, serialize};

fn has_line_break(s: &str) -> bool {
    s.contains(['\n', '\r'])
}

fuzz_target!(|data: &[u8]| {
    let root = Path::new("/r");
    let Ok(entries) = parse_bytes(data, root) else {
        return;
    };
    assert!(entries.len() <= MAX_PLAYLIST_ENTRIES);
    if let Ok(text) = std::str::from_utf8(data) {
        assert_eq!(parse(text, root).unwrap(), entries);
    }
    for e in entries.iter() {
        if let Some(t) = e.track() {
            assert!(
                t.split('/').all(|c| !c.is_empty() && c != "." && c != ".."),
                "track path not normalised: {t:?}"
            );
        }
        assert!(
            e.label()
                .is_none_or(|l| !l.is_empty() && l.len() <= MAX_LABEL_BYTES)
        );
    }

    // Writing (`Playlist::write` refuses output over the limits) and
    // reading again keeps every writable entry.
    let written = serialize(&entries, &|_| None);
    if written.len() as u64 > MAX_PLAYLIST_BYTES
        || written.lines().any(|l| l.len() > MAX_PLAYLIST_LINE_BYTES)
    {
        return;
    }
    let again = parse(&written, root).expect("a written playlist reads back");
    let kept: Vec<_> = entries
        .iter()
        .filter(|e| !has_line_break(e.key()))
        .collect();
    assert_eq!(again.len(), kept.len());
    for (a, b) in kept.iter().zip(again.iter()) {
        assert_eq!(a.track(), b.track());
        assert_eq!(a.key(), b.key());
        assert_eq!(a.duration(), b.duration());
        if let Some(l) = a.label() {
            assert_eq!(b.label(), Some(&*l.replace('\r', " ")));
        }
    }
    assert_eq!(serialize(&again, &|_| None), written, "not a fixpoint");
});
