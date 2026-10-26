//! `.tunemesh/requests/<id>.json` and `.tunemesh/status/<id>.json`
//! (contract: "Download requests", "Download status").
//!
//! All access goes through `fsx`, so a symlinked `.tunemesh`, `requests` or
//! `status` directory, or a symlinked request or status file, is refused
//! rather than followed.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::TUNEMESH_DIR;
use crate::bounded::{Bounded, BoundedVec, Count, Limited, TextList};
use crate::bounded_struct;
use crate::fsx::{Dir, Kind};
use crate::limits::{
    MAX_DEVICE_BYTES, MAX_FAILURES, MAX_ID_BYTES, MAX_INPUT_BYTES, MAX_JSON_BYTES,
    MAX_LISTED_REQUESTS, MAX_LISTED_WARNINGS, MAX_MESSAGE_BYTES, MAX_REQUEST_PLAYLIST_BYTES,
    MAX_STATUS_TRACKS, MAX_TEXT_BYTES, MAX_TIMESTAMP_BYTES, MAX_TRACK_PATH_BYTES, MAX_WARNINGS,
};

const REQUESTS: &str = "requests";
const STATUS: &str = "status";

/// A request or status id: the file name without `.json`.
pub type Id = Bounded<MAX_ID_BYTES>;
/// Status `title`, a warning, a failure's `item` or `error`.
pub type Text = Bounded<MAX_TEXT_BYTES>;
/// A request's `input` (also a status's).
pub type Input = Bounded<MAX_INPUT_BYTES>;
/// `requestedAt`, `updatedAt`.
pub type Timestamp = Bounded<MAX_TIMESTAMP_BYTES>;
/// A status's `tracks`.
pub type Tracks = TextList<MAX_TRACK_PATH_BYTES, MAX_STATUS_TRACKS>;

bounded_struct! {
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Request {
        pub version: u32,
        /// The file name decides; the JSON `id` is ignored when reading.
        #[serde(skip_deserializing)]
        pub id: Id,
        /// Cut to `MAX_INPUT_BYTES`; `input.was_cut()` tells a request that is
        /// too long, which the worker fails.
        pub input: Input,
        #[serde(default)]
        pub playlist: Option<Bounded<MAX_REQUEST_PLAYLIST_BYTES>>,
        #[serde(default)]
        pub playlist_from_source: bool,
        #[serde(default)]
        pub requested_by: Bounded<MAX_DEVICE_BYTES>,
        #[serde(default)]
        pub requested_at: Timestamp,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    Queued,
    Downloading,
    Done,
    Failed,
    Cancelled,
}

impl Limited for RequestState {}

impl RequestState {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

bounded_struct! {
    /// One item of a request that could not be downloaded.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Failure {
        /// What the item was: a title, `Artist - Title`, or its URL.
        pub item: Text,
        pub error: Text,
    }
}

bounded_struct! {
    /// A status file: what the worker writes, and what a client reads of one
    /// download it opens (`read_status`). Lists of downloads use `StatusRow`.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct Status {
        pub version: u32,
        /// The file name decides; the JSON `id` is ignored when reading.
        #[serde(skip_deserializing)]
        pub id: Id,
        pub input: Input,
        pub state: RequestState,
        #[serde(default)]
        pub title: Option<Text>,
        #[serde(default)]
        pub progress: Option<f64>,
        #[serde(default)]
        pub message: Option<Bounded<MAX_MESSAGE_BYTES>>,
        #[serde(default)]
        pub tracks: Tracks,
        /// Items finished so far (downloaded, already present, or failed).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub items_done: Option<u32>,
        /// Items the request covers, once the source is listed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub items_total: Option<u32>,
        /// Items that could not be downloaded (at most `MAX_FAILURES`).
        #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
        pub failures: BoundedVec<Failure, MAX_FAILURES>,
        /// Things the user should know although the request went on, for
        /// example that a source was cut to the item limit.
        #[serde(default, skip_serializing_if = "BoundedVec::is_empty")]
        pub warnings: BoundedVec<Text, MAX_WARNINGS>,
        #[serde(default)]
        pub updated_at: Timestamp,
    }
}

bounded_struct! {
    /// What a list of downloads keeps of a status file (contract: "Limits"):
    /// the shown fields, the first warnings, and only the number of tracks
    /// and failures. `read_status` reads the whole (cut) status of one.
    #[derive(Debug, Clone, PartialEq, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StatusRow {
        pub version: u32,
        /// The file name decides; the JSON `id` is ignored.
        #[serde(skip)]
        pub id: Id,
        pub input: Input,
        pub state: RequestState,
        #[serde(default)]
        pub title: Option<Text>,
        #[serde(default)]
        pub progress: Option<f64>,
        #[serde(default)]
        pub message: Option<Bounded<MAX_MESSAGE_BYTES>>,
        /// Number of `tracks` (at most `MAX_STATUS_TRACKS`).
        #[serde(default, rename = "tracks")]
        pub track_count: Count<MAX_STATUS_TRACKS>,
        #[serde(default)]
        pub items_done: Option<u32>,
        #[serde(default)]
        pub items_total: Option<u32>,
        /// Number of `failures` (at most `MAX_FAILURES`).
        #[serde(default, rename = "failures")]
        pub failure_count: Count<MAX_FAILURES>,
        /// The first `MAX_LISTED_WARNINGS` warnings; `warning_count` counts
        /// all of them.
        #[serde(default)]
        pub warnings: BoundedVec<Text, MAX_LISTED_WARNINGS>,
        #[serde(default)]
        pub updated_at: Timestamp,
    }
}

impl StatusRow {
    /// Number of warnings in the file (at most `MAX_WARNINGS`).
    pub fn warning_count(&self) -> usize {
        self.warnings.total().min(MAX_WARNINGS)
    }
}

bounded_struct! {
    /// Only the `state` of a status file (the worker's sweep).
    #[derive(Deserialize)]
    struct StateOnly {
        #[serde(rename = "version")]
        _version: u32,
        state: RequestState,
    }
}

impl Status {
    pub fn new(request: &Request, state: RequestState) -> Self {
        Self {
            version: 1,
            id: request.id.clone(),
            input: request.input.clone(),
            state,
            title: None,
            progress: None,
            message: None,
            tracks: Tracks::new(),
            items_done: None,
            items_total: None,
            failures: BoundedVec::new(),
            warnings: BoundedVec::new(),
            updated_at: Timestamp::from_string(crate::now_rfc3339()),
        }
    }
}

pub fn requests_dir(root: &Path) -> PathBuf {
    root.join(TUNEMESH_DIR).join(REQUESTS)
}

pub fn status_dir(root: &Path) -> PathBuf {
    root.join(TUNEMESH_DIR).join(STATUS)
}

pub fn request_path(root: &Path, id: &str) -> PathBuf {
    requests_dir(root).join(file_name(id))
}

pub fn status_path(root: &Path, id: &str) -> PathBuf {
    status_dir(root).join(file_name(id))
}

fn file_name(id: &str) -> String {
    format!("{id}.json")
}

/// `.tunemesh/<sub>`, refusing symlinks on the way.
fn open_dir(root: &Path, sub: &str, create: bool) -> io::Result<Dir> {
    Dir::open(root)?.sub(&format!("{TUNEMESH_DIR}/{sub}"), create)
}

/// A new id: Unix milliseconds, `-`, 8 lowercase hex characters.
pub fn new_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{ms}-{:08x}", fastrand::u32(..))
}

/// Sort key: the numeric millisecond prefix, then the whole id.
pub fn order_key(id: &str) -> (u128, &str) {
    let prefix = id.split('-').next().unwrap_or("");
    (prefix.parse().unwrap_or(u128::MAX), id)
}

impl Request {
    /// A new request; `input` is trimmed and, like the other fields, cut to
    /// its limit (`write` refuses one whose input was too long).
    pub fn new(
        input: &str,
        playlist: Option<String>,
        playlist_from_source: bool,
        device: &str,
    ) -> Self {
        Self {
            version: 1,
            id: Id::from_string(new_id()),
            input: Input::new(input.trim()),
            playlist: playlist.map(Bounded::from_string),
            playlist_from_source,
            requested_by: Bounded::new(device),
            requested_at: Timestamp::from_string(crate::now_rfc3339()),
        }
    }

    /// Writes the request file. A request whose input is longer than
    /// `MAX_INPUT_BYTES` is not written (the worker would fail it): the
    /// error says it is too long.
    pub fn write(&self, root: &Path) -> io::Result<()> {
        if self.input.was_cut() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the link or search text is too long (more than 8 KiB)",
            ));
        }
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        open_dir(root, REQUESTS, true)?.write_atomic(&file_name(&self.id), &json)
    }
}

impl Status {
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let json = self.to_json().map_err(io::Error::other)?;
        open_dir(root, STATUS, true)?.write_atomic(&file_name(&self.id), &json)
    }

    /// The file `write` writes: the status (whose fields are within the
    /// contract's limits by their types), cut to `MAX_JSON_BYTES` as a whole
    /// (readers skip larger files): while it is larger, `tracks`, then
    /// `failures`, then `warnings` lose entries from the end.
    pub fn to_json(&self) -> serde_json::Result<Vec<u8>> {
        let mut s = self.clone();
        loop {
            let json = serde_json::to_vec_pretty(&s)?;
            let lists_empty = s.tracks.is_empty() && s.failures.is_empty() && s.warnings.is_empty();
            // Without the lists it always fits: the other fields' limits
            // add up to far less.
            if json.len() as u64 <= MAX_JSON_BYTES || lists_empty {
                return Ok(json);
            }
            let excess = json.len() - MAX_JSON_BYTES as usize;
            let excess = shed(|| s.tracks.pop(), excess);
            let excess = shed(|| s.failures.pop(), excess);
            shed(|| s.warnings.pop(), excess);
        }
    }

    /// Records a warning, keeping at most `MAX_WARNINGS` of them.
    pub fn add_warning(&mut self, warning: String) {
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.iter().any(|w| **w == *warning) {
            self.warnings.push(Text::from_string(warning));
        }
    }

    /// Records a failed item, keeping at most `MAX_FAILURES` of them.
    pub fn add_failure(&mut self, item: String, error: String) {
        if self.failures.len() < MAX_FAILURES {
            self.failures.push(Failure {
                item: Text::from_string(item),
                error: Text::from_string(error),
            });
        }
    }
}

/// Drops entries (`pop`) until their JSON adds up to `excess` bytes;
/// returns how many bytes are still to go.
fn shed<T: Serialize>(mut pop: impl FnMut() -> Option<T>, mut excess: usize) -> usize {
    while excess > 0
        && let Some(item) = pop()
    {
        // A lower bound of what the entry takes in the file (where it is
        // indented more and followed by a comma).
        let len = serde_json::to_vec_pretty(&item).map_or(1, |j| j.len());
        excess = excess.saturating_sub(len);
    }
    excess
}

/// Whether a request was cancelled: its request file no longer exists (or is
/// no longer a regular file in a real `.tunemesh/requests` directory).
pub fn is_cancelled(root: &Path, id: &str) -> bool {
    !open_dir(root, REQUESTS, false).is_ok_and(|d| d.is_file(&file_name(id)))
}

/// Ids of the `<id>.json` entries in `dir`, oldest first. Symlinks and other
/// non-regular entries are included so callers can report and remove them.
fn ids_in(dir: &Dir) -> Vec<String> {
    let mut ids: Vec<String> = dir
        .entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, kind)| {
            *kind != Kind::Dir && !name.starts_with('.') && !name.contains(".sync-conflict-")
        })
        .filter_map(|(mut name, _)| {
            let len = name
                .strip_suffix(".json")
                .filter(|id| !id.is_empty())?
                .len();
            name.truncate(len);
            name.shrink_to_fit();
            Some(name)
        })
        .collect();
    ids.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    ids
}

fn read_bytes(dir: &Dir, id: &str) -> io::Result<Vec<u8>> {
    dir.read(&file_name(id), MAX_JSON_BYTES)
}

/// A request file's contents, read as `<id>.json` (the id comes from the
/// file name, not the contents), cut to the contract's limits.
pub fn parse_request(bytes: &[u8], id: &str) -> io::Result<Request> {
    let mut r: Request = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    r.id = Id::new(id);
    Ok(r)
}

/// A status file's contents, read as `<id>.json`, cut to the contract's
/// limits.
pub fn parse_status(bytes: &[u8], id: &str) -> io::Result<Status> {
    let mut s: Status = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    s.id = Id::new(id);
    Ok(s)
}

/// What a list of downloads keeps of a status file (`StatusRow`).
pub fn parse_status_row(bytes: &[u8], id: &str) -> io::Result<StatusRow> {
    let mut s: StatusRow = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    s.id = Id::new(id);
    Ok(s)
}

fn read_request(dir: &Dir, id: &str) -> io::Result<Request> {
    parse_request(&read_bytes(dir, id)?, id)
}

fn read_status_in(dir: &Dir, id: &str) -> io::Result<Status> {
    parse_status(&read_bytes(dir, id)?, id)
}

fn read_state_in(dir: &Dir, id: &str) -> io::Result<RequestState> {
    let s: StateOnly = serde_json::from_slice(&read_bytes(dir, id)?).map_err(io::Error::other)?;
    Ok(s.state)
}

/// The oldest `max` requests (by numeric id prefix), parsed and cut.
/// Unparseable or unreadable files among them (including symlinks) are
/// returned separately with their id and error. The third value counts
/// every request file, so callers can tell how many were left out.
pub fn list_oldest_requests(
    root: &Path,
    max: usize,
) -> (Vec<Request>, Vec<(String, io::Error)>, usize) {
    let Ok(dir) = open_dir(root, REQUESTS, false) else {
        return (Vec::new(), Vec::new(), 0);
    };
    let ids = ids_in(&dir);
    let total = ids.len();
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for id in ids.into_iter().take(max) {
        match read_request(&dir, &id) {
            Ok(req) => ok.push(req),
            Err(e) => bad.push((id, e)),
        }
    }
    (ok, bad, total)
}

/// One download as clients list it: a request, its status, or both.
#[derive(Debug, Clone)]
pub struct Recent {
    pub id: String,
    pub request: Option<Request>,
    pub status: Option<StatusRow>,
}

/// The newest `MAX_LISTED_REQUESTS` ids over requests and status files
/// together, newest first, each with whatever of the two could be read (cut
/// to what a list shows); plus the number of distinct ids in both
/// directories, so clients can say how many more there are (contract:
/// "Limits").
pub fn recent(root: &Path) -> (Vec<Recent>, usize) {
    let requests = open_dir(root, REQUESTS, false).ok();
    let statuses = open_dir(root, STATUS, false).ok();
    let mut ids: Vec<String> = requests
        .iter()
        .chain(statuses.iter())
        .flat_map(ids_in)
        .collect();
    ids.sort_by(|a, b| order_key(b).cmp(&order_key(a)));
    ids.dedup();
    let total = ids.len();
    ids.truncate(MAX_LISTED_REQUESTS);
    let rows = ids
        .into_iter()
        .map(|id| Recent {
            request: requests.as_ref().and_then(|d| read_request(d, &id).ok()),
            status: statuses.as_ref().and_then(|d| {
                read_bytes(d, &id)
                    .and_then(|b| parse_status_row(&b, &id))
                    .ok()
            }),
            id,
        })
        .collect();
    (rows, total)
}

/// Ids of the request files, oldest first, without reading them.
pub fn request_ids(root: &Path) -> Vec<String> {
    open_dir(root, REQUESTS, false)
        .map(|d| ids_in(&d))
        .unwrap_or_default()
}

/// Ids of the status files, oldest first, without reading them.
pub fn status_ids(root: &Path) -> Vec<String> {
    open_dir(root, STATUS, false)
        .map(|d| ids_in(&d))
        .unwrap_or_default()
}

/// One status file, cut to the contract's limits (tracks, failures and
/// warnings included).
pub fn read_status(root: &Path, id: &str) -> Option<Status> {
    read_status_in(&open_dir(root, STATUS, false).ok()?, id).ok()
}

/// Only the `state` of one status file, if it is a valid status.
pub fn read_state(root: &Path, id: &str) -> Option<RequestState> {
    read_state_in(&open_dir(root, STATUS, false).ok()?, id).ok()
}

fn remove(root: &Path, sub: &str, id: &str) -> io::Result<()> {
    let result = open_dir(root, sub, false).and_then(|d| d.remove_file(&file_name(id)));
    match result {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Client-side cancellation: deletes the request file.
pub fn cancel(root: &Path, id: &str) -> io::Result<()> {
    remove(root, REQUESTS, id)
}

/// Deletes a status file (a missing one is not an error).
pub fn remove_status(root: &Path, id: &str) -> io::Result<()> {
    remove(root, STATUS, id)
}

/// Status ids whose request file is gone, oldest first, with whether each is
/// among the newest `MAX_LISTED_REQUESTS` status ids. Only file names are
/// read.
fn orphan_status_ids(root: &Path) -> (Option<Dir>, Vec<(String, bool)>) {
    let Ok(dir) = open_dir(root, STATUS, false) else {
        return (None, Vec::new());
    };
    let ids = ids_in(&dir);
    let requests: HashSet<String> = request_ids(root).into_iter().collect();
    let newest_from = ids.len().saturating_sub(MAX_LISTED_REQUESTS);
    let orphans = ids
        .into_iter()
        .enumerate()
        .filter(|(_, id)| !requests.contains(id))
        .map(|(i, id)| (id, i >= newest_from))
        .collect();
    (Some(dir), orphans)
}

/// Client-side "Clear": deletes every status file in a final state whose
/// request file is gone (not only the listed ones), reading one file at a
/// time and only its `state`. Returns how many were removed.
pub fn clear_finished(root: &Path) -> usize {
    let (Some(dir), orphans) = orphan_status_ids(root) else {
        return 0;
    };
    orphans
        .into_iter()
        .filter(|(id, _)| read_state_in(&dir, id).is_ok_and(RequestState::is_final))
        .filter(|(id, _)| is_cancelled(root, id) && remove_status(root, id).is_ok())
        .count()
}

/// What `sweep` did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Sweep {
    /// Ids of unfinished statuses set to `cancelled`.
    pub cancelled: Vec<String>,
    /// Final statuses deleted.
    pub pruned: usize,
    /// Errors writing or deleting, by id.
    pub errors: Vec<(String, String)>,
}

/// The worker's sweep (contract: "Download status"): of the status files
/// whose request file is gone, read one at a time and only for their
/// `state`, an unfinished one is set to `cancelled` (its request was
/// cancelled while no worker was watching), and a final one that is not
/// among the newest `MAX_LISTED_REQUESTS` status ids is deleted. Memory does
/// not grow with the number of status files beyond their names.
pub fn sweep(root: &Path) -> Sweep {
    let mut done = Sweep::default();
    let (Some(dir), orphans) = orphan_status_ids(root) else {
        return done;
    };
    for (id, newest) in orphans {
        let Ok(state) = read_state_in(&dir, &id) else {
            continue;
        };
        // A request file written since the ids were listed.
        if !is_cancelled(root, &id) {
            continue;
        }
        if !state.is_final() {
            let Ok(mut s) = read_status_in(&dir, &id) else {
                continue;
            };
            s.state = RequestState::Cancelled;
            s.progress = None;
            s.updated_at = Timestamp::from_string(crate::now_rfc3339());
            match s.write(root) {
                Ok(()) => done.cancelled.push(id),
                Err(e) => done.errors.push((id, e.to_string())),
            }
        } else if !newest {
            match remove_status(root, &id) {
                Ok(()) => done.pruned += 1,
                Err(e) => done.errors.push((id, e.to_string())),
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    #[cfg_attr(miri, ignore = "wall clock")]
    fn id_format() {
        let id = new_id();
        let (ms, hex) = id.split_once('-').unwrap();
        assert!(ms.parse::<u64>().unwrap() > 1_700_000_000_000);
        assert_eq!(hex.len(), 8);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    fn request_with_id(input: &str, id: &str) -> Request {
        let mut r = Request::new(input, None, false, "test");
        r.id = id.into();
        r
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn requests_ordered_by_numeric_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for id in [
            "1791265297123-ffffffff",
            "999999999999-00000000",
            "1791265297123-0000000a",
            "10000000000000-aaaaaaaa",
        ] {
            request_with_id("x", id).write(root).unwrap();
        }
        fs::write(request_path(root, "broken"), b"{").unwrap();
        let (ok, bad, _) = list_oldest_requests(root, usize::MAX);
        let ids: Vec<_> = ok.iter().map(|r| r.id.as_str()).collect();
        // Lexical order would put "10000000000000" before "1791265297123".
        assert_eq!(
            ids,
            vec![
                "999999999999-00000000",
                "1791265297123-0000000a",
                "1791265297123-ffffffff",
                "10000000000000-aaaaaaaa"
            ]
        );
        assert_eq!(bad.len(), 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn cancellation_is_a_missing_request_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let r = Request::new(" https://example.com/a ", Some("P".into()), false, "pc");
        assert_eq!(r.input, "https://example.com/a");
        r.write(root).unwrap();
        assert!(!is_cancelled(root, &r.id));
        cancel(root, &r.id).unwrap();
        assert!(is_cancelled(root, &r.id));
        cancel(root, &r.id).unwrap();

        Status::new(&r, RequestState::Cancelled)
            .write(root)
            .unwrap();
        let other = Request::new("y", None, false, "pc");
        other.write(root).unwrap();
        Status::new(&other, RequestState::Done).write(root).unwrap();
        // Only final statuses without a request file are cleared.
        assert_eq!(clear_finished(root), 1);
        assert_eq!(status_ids(root), [other.id.as_str()]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "wall clock")]
    fn json_shape_matches_contract() {
        let json = r#"{"version":1,"id":"1-00000000","input":"q","playlist":null,"playlistFromSource":true,"requestedBy":"pixel-8","requestedAt":"2026-10-06T05:01:37Z"}"#;
        let r = parse_request(json.as_bytes(), "1-00000000").unwrap();
        assert!(r.playlist_from_source);
        let s = Status::new(&r, RequestState::Downloading);
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "downloading");
        assert_eq!(v["id"], "1-00000000");
        assert!(v.get("updatedAt").is_some());
        assert!(v["progress"].is_null());
        let back: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(
            back,
            serde_json::from_str::<serde_json::Value>(json).unwrap()
        );
        // Any other state makes the status invalid; so does a version that
        // is not a u32.
        let other = r#"{"version":1,"input":"q","state":"paused"}"#;
        assert!(parse_status(other.as_bytes(), "x").is_err());
        assert!(parse_status_row(other.as_bytes(), "x").is_err());
        let version = r#"{"version":-1,"input":"q","state":"done"}"#;
        assert!(parse_status(version.as_bytes(), "x").is_err());
    }

    /// The JSON `id` is ignored: the file name decides.
    #[test]
    fn the_file_name_is_the_id() {
        let json = format!(
            r#"{{"version":1,"id":"{}","input":"q","state":"done"}}"#,
            "i".repeat(100_000)
        );
        let s = parse_status(json.as_bytes(), "1-00000000").unwrap();
        assert_eq!(s.id, "1-00000000");
        let r = parse_request(json.as_bytes(), "2-00000000").unwrap();
        assert_eq!(r.id, "2-00000000");
    }

    /// Audit finding 2: a symlinked `.tunemesh/status` or `requests`
    /// directory must not let the worker write or delete files elsewhere.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn symlinked_tunemesh_dirs_are_refused() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("settings.json"), b"{}").unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        fs::create_dir_all(root.join(TUNEMESH_DIR)).unwrap();
        symlink(outside.path(), status_dir(root)).unwrap();
        symlink(outside.path(), requests_dir(root)).unwrap();

        let r = request_with_id("x", "settings");
        let s = Status::new(&r, RequestState::Failed);
        assert!(s.write(root).is_err());
        assert!(r.write(root).is_err());
        assert_eq!(
            fs::read(outside.path().join("settings.json")).unwrap(),
            b"{}"
        );
        let (ok, bad, total) = list_oldest_requests(root, 10);
        assert!(ok.is_empty() && bad.is_empty() && total == 0);
        assert!(status_ids(root).is_empty());
        assert_eq!(sweep(root), Sweep::default());
        assert_eq!(clear_finished(root), 0);
        assert!(is_cancelled(root, "settings"));
        assert!(cancel(root, "settings").is_err());
        assert!(remove_status(root, "settings").is_err());
        assert!(outside.path().join("settings.json").exists());

        // A symlinked .tunemesh is refused the same way.
        fs::remove_dir_all(root.join(TUNEMESH_DIR)).unwrap();
        symlink(outside.path(), root.join(TUNEMESH_DIR)).unwrap();
        assert!(s.write(root).is_err());
        assert!(!outside.path().join(STATUS).exists());
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn symlinked_request_files_are_reported_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("other.json");
        fs::write(&target, br#"{"version":1,"id":"x","input":"q"}"#).unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        Request::new("a", None, false, "pc").write(root).unwrap();
        symlink(&target, request_path(root, "1-00000000")).unwrap();
        let (ok, bad, total) = list_oldest_requests(root, 10);
        assert_eq!((ok.len(), bad.len(), total), (1, 1, 2));
        assert_eq!(bad[0].0, "1-00000000");
        // Removing it removes the link only.
        cancel(root, "1-00000000").unwrap();
        assert!(target.exists());
        assert_eq!(list_oldest_requests(root, 0).2, 1);
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn oldest_requests_are_limited() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        for id in ["3-00000000", "1-00000000", "2-00000000"] {
            request_with_id("x", id).write(root).unwrap();
        }
        let (ok, bad, total) = list_oldest_requests(root, 2);
        assert_eq!(
            ok.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["1-00000000", "2-00000000"]
        );
        assert!(bad.is_empty());
        assert_eq!(total, 3);
    }

    #[test]
    #[cfg_attr(miri, ignore = "wall clock")]
    fn progress_fields_are_optional() {
        let r = Request::new("x", None, false, "pc");
        let mut s = Status::new(&r, RequestState::Downloading);
        let v = serde_json::to_value(&s).unwrap();
        assert!(
            v.get("itemsDone").is_none()
                && v.get("failures").is_none()
                && v.get("warnings").is_none()
        );
        s.items_done = Some(1);
        s.items_total = Some(3);
        for i in 0..MAX_FAILURES + 5 {
            s.add_failure(format!("t{i}"), "gone".into());
        }
        s.add_warning("w".into());
        s.add_warning("w".into());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["itemsDone"], 1);
        assert_eq!(v["itemsTotal"], 3);
        assert_eq!(v["failures"].as_array().unwrap().len(), MAX_FAILURES);
        assert_eq!(v["failures"][0]["item"], "t0");
        assert_eq!(v["warnings"], serde_json::json!(["w"]));
        let back = parse_status(&serde_json::to_vec(&v).unwrap(), &s.id).unwrap();
        assert_eq!(back, s);
    }

    /// Every field of a status file is cut to the contract's limits when
    /// read (and the writer cannot exceed them: its fields are the same
    /// types).
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn status_contents_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = "1-00000000";
        let json = serde_json::json!({
            "version": 1, "id": "i".repeat(5_000), "input": "q".repeat(9_000),
            "state": "done", "title": "t".repeat(5_000), "message": "é".repeat(5_000),
            "warnings": (0..80).map(|i| format!("{i}{}", "w".repeat(2_000))).collect::<Vec<_>>(),
            "failures": (0..300).map(|i| serde_json::json!({"item": format!("{i}"), "error": "e".repeat(2_000)})).collect::<Vec<_>>(),
            "tracks": (0..20_000).map(|i| if i == 0 { "p".repeat(5_000) } else { "a.mp3".to_owned() }).collect::<Vec<_>>(),
            "updatedAt": "u".repeat(1_000),
        });
        fs::create_dir_all(status_dir(root)).unwrap();
        fs::write(status_path(root, id), json.to_string()).unwrap();
        let back = read_status(root, id).unwrap();
        assert_eq!(back.id, id);
        assert_eq!(back.input.len(), MAX_INPUT_BYTES);
        assert!(back.input.was_cut());
        assert_eq!(back.title.unwrap().len(), MAX_TEXT_BYTES);
        let m = back.message.unwrap();
        assert!(m.len() <= MAX_MESSAGE_BYTES && m.len() > MAX_MESSAGE_BYTES - 2);
        assert_eq!(back.warnings.len(), MAX_WARNINGS);
        assert!(back.warnings.iter().all(|w| w.len() == MAX_TEXT_BYTES));
        assert_eq!(back.failures.len(), MAX_FAILURES);
        assert_eq!(back.failures[0].error.len(), MAX_TEXT_BYTES);
        assert_eq!(back.tracks.len(), MAX_STATUS_TRACKS);
        assert_eq!(back.tracks.get(0).unwrap().len(), MAX_TRACK_PATH_BYTES);
        assert_eq!(back.updated_at.len(), MAX_TIMESTAMP_BYTES);
        // A list row keeps counts and the first warnings only.
        let (rows, _) = recent(root);
        let row = rows[0].status.as_ref().unwrap();
        assert_eq!(row.track_count.get(), MAX_STATUS_TRACKS);
        assert_eq!(row.failure_count.get(), MAX_FAILURES);
        assert_eq!(row.warnings.len(), MAX_LISTED_WARNINGS);
        assert_eq!(row.warning_count(), MAX_WARNINGS);
        assert_eq!(row.updated_at.len(), MAX_TIMESTAMP_BYTES);
        // Written back, it stays within the limits.
        let written = serde_json::from_slice::<serde_json::Value>(
            &read_status(root, id).unwrap().to_json().unwrap(),
        )
        .unwrap();
        assert_eq!(written["warnings"].as_array().unwrap().len(), MAX_WARNINGS);
        // Larger than 1 MiB: not read.
        fs::write(
            status_path(root, id),
            vec![b' '; MAX_JSON_BYTES as usize + 1],
        )
        .unwrap();
        assert!(read_status(root, id).is_none());
    }

    /// R3-Opus-3: readers gave clients the whole request `input` and the
    /// timestamps (397 MiB for 200 crafted request and status files).
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn readers_keep_cut_copies() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big = "x".repeat(1_000_000);
        fs::create_dir_all(requests_dir(root)).unwrap();
        fs::create_dir_all(status_dir(root)).unwrap();
        let req = serde_json::json!({
            "version": 1, "id": "1-00000000", "input": big, "playlist": "p".repeat(5_000),
            "requestedBy": "d".repeat(5_000), "requestedAt": "t".repeat(5_000),
        });
        fs::write(request_path(root, "1-00000000"), req.to_string()).unwrap();
        let status =
            serde_json::json!({"version": 1, "input": "i", "state": "done", "updatedAt": big});
        fs::write(status_path(root, "1-00000000"), status.to_string()).unwrap();
        let (rows, _) = recent(root);
        let r = rows[0].request.as_ref().unwrap();
        assert_eq!(r.input.len(), MAX_INPUT_BYTES);
        assert!(r.input.was_cut());
        assert_eq!(
            r.playlist.as_ref().unwrap().len(),
            MAX_REQUEST_PLAYLIST_BYTES
        );
        assert_eq!(r.requested_by.len(), MAX_DEVICE_BYTES);
        assert_eq!(r.requested_at.len(), MAX_TIMESTAMP_BYTES);
        let s = rows[0].status.as_ref().unwrap();
        assert_eq!(s.updated_at.len(), MAX_TIMESTAMP_BYTES);
        // The worker sees that the input was too long.
        let (ok, _, _) = list_oldest_requests(root, 1);
        assert!(ok[0].input.was_cut());
    }

    /// Ledger trivial (round 3): a request over the input limit was written
    /// and failed by the worker; clients now get an error instead.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn too_long_requests_are_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let r = Request::new(&"q".repeat(MAX_INPUT_BYTES + 1), None, false, "pc");
        let e = r.write(root).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert!(request_ids(root).is_empty());
        Request::new(&"q".repeat(MAX_INPUT_BYTES), None, false, "pc")
            .write(root)
            .unwrap();
        assert_eq!(request_ids(root).len(), 1);
    }

    /// Clients read only the newest ids over requests and status files.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn recent_reads_the_newest_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..MAX_LISTED_REQUESTS + 50 {
            let r = request_with_id(&format!("in {i}"), &format!("{}-{:08x}", 1_000 + i, i));
            r.write(root).unwrap();
            if i % 2 == 0 {
                Status::new(&r, RequestState::Queued).write(root).unwrap();
            }
        }
        // A status without a request, newest of all.
        let lone = request_with_id("lone", "9999-00000000");
        Status::new(&lone, RequestState::Done).write(root).unwrap();
        let (rows, total) = recent(root);
        assert_eq!(total, MAX_LISTED_REQUESTS + 51);
        assert_eq!(rows.len(), MAX_LISTED_REQUESTS);
        assert_eq!(rows[0].id, "9999-00000000");
        assert!(rows[0].request.is_none() && rows[0].status.is_some());
        let newest = &rows[1];
        assert_eq!(
            newest.request.as_ref().unwrap().input.as_str(),
            format!("in {}", MAX_LISTED_REQUESTS + 49)
        );
        assert!(
            rows.windows(2)
                .all(|w| order_key(&w[0].id) > order_key(&w[1].id))
        );
        assert_eq!(request_ids(root).len(), MAX_LISTED_REQUESTS + 50);
        assert_eq!(status_ids(root).len(), (MAX_LISTED_REQUESTS + 50) / 2 + 1);
    }

    /// R3Deepseek-2: "Clear" looked only at the newest 200 status files, so
    /// older finished ones could never be removed.
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn clear_removes_every_finished_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..MAX_LISTED_REQUESTS + 50 {
            let r = request_with_id("x", &format!("{}-{:08x}", 1_000 + i, i));
            Status::new(&r, RequestState::Done).write(root).unwrap();
        }
        // Not cleared: unfinished, with a request file, or invalid.
        let pending = request_with_id("x", "1-00000000");
        pending.write(root).unwrap();
        Status::new(&pending, RequestState::Done)
            .write(root)
            .unwrap();
        Status::new(
            &request_with_id("x", "2-00000000"),
            RequestState::Downloading,
        )
        .write(root)
        .unwrap();
        fs::write(status_path(root, "3-00000000"), b"{").unwrap();
        assert_eq!(clear_finished(root), MAX_LISTED_REQUESTS + 50);
        assert_eq!(status_ids(root), ["1-00000000", "2-00000000", "3-00000000"]);
    }

    /// R3-Opus-2 / R3Deepseek-1 / R3Deepseek-2: the worker's sweep marks
    /// orphaned unfinished statuses cancelled and deletes orphaned final
    /// ones past the newest 200 status ids (its memory is measured in
    /// `tests/memory.rs`).
    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn sweep_cancels_and_prunes_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let n = MAX_LISTED_REQUESTS + 30;
        for i in 0..n {
            let r = request_with_id("x", &format!("{}-{:08x}", 1_000 + i, i));
            let mut s = Status::new(&r, RequestState::Done);
            s.tracks.push("A/B/c.opus");
            s.write(root).unwrap();
        }
        // An unfinished orphan among the oldest, and an old final status
        // whose request is still there.
        let gone = request_with_id("x", "10-00000000");
        Status::new(&gone, RequestState::Downloading)
            .write(root)
            .unwrap();
        let kept = request_with_id("x", "11-00000000");
        kept.write(root).unwrap();
        Status::new(&kept, RequestState::Done).write(root).unwrap();

        let done = sweep(root);
        assert_eq!(done.cancelled, ["10-00000000"]);
        // n + 2 status ids; the newest 200 stay, of the older ones only the
        // just-cancelled one and the one with a request.
        assert_eq!(done.pruned, n + 2 - MAX_LISTED_REQUESTS - 2);
        assert!(done.errors.is_empty());
        let s = read_status(root, "10-00000000").unwrap();
        assert_eq!(s.state, RequestState::Cancelled);
        assert!(read_status(root, "11-00000000").is_some());
        assert_eq!(status_ids(root).len(), MAX_LISTED_REQUESTS + 2);
        // Now final and old: the next sweep deletes it.
        assert_eq!(sweep(root).pruned, 1);
        assert!(read_status(root, "10-00000000").is_none());
    }

    /// Found by `fuzz/fuzz_targets/status_limits.rs`: 10,000 long track
    /// paths, or 200 failures of control characters (6 bytes each in
    /// JSON), made a status file of several MiB that no client reads.
    #[test]
    #[cfg_attr(miri, ignore = "too slow under Miri")]
    fn written_statuses_stay_readable() {
        let req = Request::new("https://example.com/x", None, false, "pc");
        let mut s = Status::new(&req, RequestState::Done);
        let component = "x".repeat(120);
        for i in 0..MAX_STATUS_TRACKS {
            s.tracks
                .push(&format!("{component}/{component}/{i:0>116} [id].opus"));
        }
        let json = s.to_json().unwrap();
        assert!(json.len() as u64 <= MAX_JSON_BYTES, "{}", json.len());
        let back = parse_status(&json, &s.id).unwrap();
        assert!(back.tracks.len() > 2_000);
        assert!(
            back.tracks
                .iter()
                .eq(s.tracks.iter().take(back.tracks.len()))
        );

        let mut s = Status::new(&req, RequestState::Failed);
        for _ in 0..MAX_FAILURES {
            s.add_failure(
                "\u{1}".repeat(MAX_TEXT_BYTES),
                "\u{1}".repeat(MAX_TEXT_BYTES),
            );
        }
        for i in 0..MAX_WARNINGS {
            s.add_warning(format!("{i}{}", "\u{1}".repeat(MAX_TEXT_BYTES)));
        }
        let json = s.to_json().unwrap();
        assert!(json.len() as u64 <= MAX_JSON_BYTES, "{}", json.len());
        let back = parse_status(&json, &s.id).unwrap();
        assert!(back.failures.len() < MAX_FAILURES && !back.failures.is_empty());
        assert_eq!(back.warnings.len(), MAX_WARNINGS);
        // A status that fits is written whole.
        let small = Status::new(&req, RequestState::Queued);
        assert_eq!(
            parse_status(&small.to_json().unwrap(), &small.id).unwrap(),
            small
        );
    }
}
