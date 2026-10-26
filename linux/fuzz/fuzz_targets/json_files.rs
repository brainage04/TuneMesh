//! Synced request, status and `worker.json` files: parsing never panics,
//! what is read is within the limits, and writing it back reads back the
//! same.
#![no_main]

use std::time::{Duration, UNIX_EPOCH};

use libfuzzer_sys::fuzz_target;
use tunemesh_core::limits::{
    MAX_DEVICE_BYTES, MAX_FAILURES, MAX_INPUT_BYTES, MAX_JSON_BYTES, MAX_LISTED_WARNINGS,
    MAX_MESSAGE_BYTES, MAX_REQUEST_PLAYLIST_BYTES, MAX_STATUS_TRACKS, MAX_TEXT_BYTES,
    MAX_TIMESTAMP_BYTES, MAX_TRACK_PATH_BYTES, MAX_WARNINGS, MAX_WORKER_ID_BYTES,
};
use tunemesh_core::request::{Status, parse_request, parse_status, parse_status_row};
use tunemesh_core::worker;

const ID: &str = "1791265297123-3f9c2a1b";

/// Statuses equal up to float rounding in `progress`.
fn same(a: &Status, b: &Status) -> bool {
    let close = match (a.progress, b.progress) {
        (Some(x), Some(y)) => x == y || (x - y).abs() <= x.abs().max(y.abs()) * 1e-15,
        (x, y) => x == y,
    };
    close
        && Status {
            progress: None,
            ..a.clone()
        } == Status {
            progress: None,
            ..b.clone()
        }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(r) = parse_request(data, ID) {
        assert_eq!(r.id, ID);
        assert!(
            r.playlist
                .as_ref()
                .is_none_or(|p| p.len() <= MAX_REQUEST_PLAYLIST_BYTES)
        );
        assert!(r.input.len() <= MAX_INPUT_BYTES);
        assert!(r.requested_by.len() <= MAX_DEVICE_BYTES);
        assert!(r.requested_at.len() <= MAX_TIMESTAMP_BYTES);
        let json = serde_json::to_vec_pretty(&r).unwrap();
        assert_eq!(parse_request(&json, ID).unwrap(), r);
    }

    if let Ok(s) = parse_status(data, ID) {
        assert_eq!(s.id, ID);
        assert!(s.input.len() <= MAX_INPUT_BYTES);
        assert!(s.title.as_ref().is_none_or(|t| t.len() <= MAX_TEXT_BYTES));
        assert!(
            s.message
                .as_ref()
                .is_none_or(|m| m.len() <= MAX_MESSAGE_BYTES)
        );
        assert!(s.warnings.len() <= MAX_WARNINGS);
        assert!(s.warnings.iter().all(|w| w.len() <= MAX_TEXT_BYTES));
        assert!(s.failures.len() <= MAX_FAILURES);
        assert!(
            s.failures
                .iter()
                .all(|f| f.item.len() <= MAX_TEXT_BYTES && f.error.len() <= MAX_TEXT_BYTES)
        );
        assert!(s.tracks.len() <= MAX_STATUS_TRACKS);
        assert!(s.tracks.iter().all(|t| t.len() <= MAX_TRACK_PATH_BYTES));
        assert!(s.updated_at.len() <= MAX_TIMESTAMP_BYTES);
        // A list row of the same file agrees and keeps less.
        let row = parse_status_row(data, ID).expect("a valid status is a valid row");
        assert_eq!((row.state, &row.input, &row.title), (s.state, &s.input, &s.title));
        assert_eq!(row.track_count.get(), s.tracks.len());
        assert_eq!(row.failure_count.get(), s.failures.len());
        assert!(row.warnings.len() <= MAX_LISTED_WARNINGS);
        assert_eq!(row.warning_count(), s.warnings.len());
        // What the worker would write for it reads back the same.
        let json = s.to_json().unwrap();
        assert!(json.len() as u64 <= MAX_JSON_BYTES);
        let back = parse_status(&json, ID).unwrap();
        assert!(same(&back, &s), "{back:?} != {s:?}");
    }

    if let Some(w) = worker::parse(data) {
        let _ = w.is_fresh();
        for secs in [0, 1_791_265_297, u64::from(u32::MAX), 1 << 40] {
            let _ = w.is_fresh_at(UNIX_EPOCH + Duration::from_secs(secs));
        }
        assert!(w.device.len() <= MAX_DEVICE_BYTES && w.heartbeat_at.len() <= MAX_TIMESTAMP_BYTES);
        assert!(
            [&w.machine, &w.instance, &w.worker_version]
                .iter()
                .all(|f| f.len() <= MAX_WORKER_ID_BYTES)
        );
        let json = serde_json::to_vec_pretty(&w).unwrap();
        assert_eq!(worker::parse(&json), Some(w));
    }
});
