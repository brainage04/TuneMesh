//! Every status file the worker writes can be read by every client: it
//! stays under the 1 MiB file limit however long its texts and lists are.
#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tunemesh_core::limits::MAX_JSON_BYTES;
use tunemesh_core::request::{Failure, RequestState, Status, parse_status};

const ID: &str = "1791265297123-3f9c2a1b";

/// A text: `text` repeated `times` times (so a short input makes a long one).
#[derive(Arbitrary, Debug)]
struct Long {
    text: String,
    times: u16,
}

impl Long {
    fn get(&self, max: usize) -> String {
        let n = usize::from(self.times).min(max / self.text.len().max(1));
        self.text.repeat(n)
    }
}

#[derive(Arbitrary, Debug)]
struct Input {
    input: Long,
    title: Option<Long>,
    message: Option<Long>,
    track: Long,
    tracks: u16,
    item: Long,
    error: Long,
    failures: u16,
    warning: Long,
    warnings: u8,
    progress: Option<f64>,
}

fuzz_target!(|i: Input| {
    // Each text at most 16 KiB, past every per-field limit.
    const T: usize = 16 * 1024;
    let tracks: Vec<String> = (0..i.tracks.min(12_000))
        .map(|n| format!("{}{n}", i.track.get(362)))
        .collect();
    let status = Status {
        version: 1,
        id: ID.into(),
        input: i.input.get(T).into(),
        state: RequestState::Done,
        title: i.title.map(|t| t.get(T).into()),
        progress: i.progress.filter(|p| p.is_finite()),
        message: i.message.map(|t| t.get(T).into()),
        // Up to 12,000 library paths (three 120-byte components each).
        tracks: tracks.iter().map(String::as_str).collect(),
        items_done: None,
        items_total: None,
        failures: (0..i.failures.min(300))
            .map(|_| Failure {
                item: i.item.get(T).into(),
                error: i.error.get(T).into(),
            })
            .collect(),
        warnings: (0..i.warnings)
            .map(|n| format!("{}{n}", i.warning.get(T)).into())
            .collect(),
        updated_at: "2026-10-25T00:00:00Z".into(),
    };
    let json = status.to_json().unwrap();
    assert!(
        json.len() as u64 <= MAX_JSON_BYTES,
        "a {} byte status file is too large to be read",
        json.len()
    );
    let back = parse_status(&json, ID).expect("a written status reads back");
    assert_eq!(back.state, status.state);
    assert_eq!(back.input, status.input);
    // What the worker keeps is what it writes: its fields are cut already.
    assert!(back.tracks.iter().eq(status.tracks.iter().take(back.tracks.len())));
});
