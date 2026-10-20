//! `.tunemesh/requests/<id>.json` and `.tunemesh/status/<id>.json`
//! (contract: "Download requests", "Download status").
//!
//! All access goes through `fsx`, so a symlinked `.tunemesh`, `requests` or
//! `status` directory, or a symlinked request or status file, is refused
//! rather than followed.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::TUNEMESH_DIR;
use crate::fsx::{Dir, Kind};
use crate::limits::{
    self, MAX_FAILURES, MAX_INPUT_BYTES, MAX_JSON_BYTES, MAX_LISTED_REQUESTS, MAX_MESSAGE_BYTES,
    MAX_REQUEST_PLAYLIST_BYTES, MAX_STATUS_TRACKS, MAX_TEXT_BYTES, MAX_WARNINGS,
};

const REQUESTS: &str = "requests";
const STATUS: &str = "status";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub version: u32,
    pub id: String,
    pub input: String,
    #[serde(default)]
    pub playlist: Option<String>,
    #[serde(default)]
    pub playlist_from_source: bool,
    #[serde(default)]
    pub requested_by: String,
    #[serde(default)]
    pub requested_at: String,
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

impl RequestState {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// One item of a request that could not be downloaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    /// What the item was: a title, `Artist - Title`, or its URL.
    pub item: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: u32,
    pub id: String,
    pub input: String,
    pub state: RequestState,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub progress: Option<f64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub tracks: Vec<String>,
    /// Items finished so far (downloaded, already present, or failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_done: Option<u32>,
    /// Items the request covers, once the source is listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items_total: Option<u32>,
    /// Items that could not be downloaded (at most `MAX_FAILURES`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
    /// Things the user should know although the request went on, for example
    /// that a source was cut to the item limit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub updated_at: String,
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
            tracks: Vec::new(),
            items_done: None,
            items_total: None,
            failures: Vec::new(),
            warnings: Vec::new(),
            updated_at: crate::now_rfc3339(),
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
    pub fn new(
        input: &str,
        playlist: Option<String>,
        playlist_from_source: bool,
        device: &str,
    ) -> Self {
        Self {
            version: 1,
            id: new_id(),
            input: input.trim().to_owned(),
            playlist,
            playlist_from_source,
            requested_by: device.to_owned(),
            requested_at: crate::now_rfc3339(),
        }
    }

    pub fn write(&self, root: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        open_dir(root, REQUESTS, true)?.write_atomic(&file_name(&self.id), &json)
    }
}

impl Status {
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let mut s = self.clone();
        s.clamp();
        let json = serde_json::to_vec_pretty(&s).map_err(io::Error::other)?;
        open_dir(root, STATUS, true)?.write_atomic(&file_name(&self.id), &json)
    }

    /// Cuts every field to the contract's limits (`limits`): applied when a
    /// status file is read and before one is written.
    pub fn clamp(&mut self) {
        let cut = |s: &mut String, max| limits::cut_in_place(s, max);
        cut(&mut self.input, MAX_INPUT_BYTES);
        if let Some(t) = &mut self.title {
            cut(t, MAX_TEXT_BYTES);
        }
        if let Some(m) = &mut self.message {
            cut(m, MAX_MESSAGE_BYTES);
        }
        self.warnings.truncate(MAX_WARNINGS);
        for w in &mut self.warnings {
            cut(w, MAX_TEXT_BYTES);
        }
        self.failures.truncate(MAX_FAILURES);
        for f in &mut self.failures {
            cut(&mut f.item, MAX_TEXT_BYTES);
            cut(&mut f.error, MAX_TEXT_BYTES);
        }
        self.tracks.truncate(MAX_STATUS_TRACKS);
    }

    /// Records a warning, keeping at most `MAX_WARNINGS` of them.
    pub fn add_warning(&mut self, warning: String) {
        if self.warnings.len() < MAX_WARNINGS && !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    /// Records a failed item, keeping at most `MAX_FAILURES` of them.
    pub fn add_failure(&mut self, item: String, error: String) {
        if self.failures.len() < MAX_FAILURES {
            self.failures.push(Failure { item, error });
        }
    }
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
        .filter_map(|(name, _)| {
            name.strip_suffix(".json")
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect();
    ids.sort_by(|a, b| order_key(a).cmp(&order_key(b)));
    ids
}

fn read_json<T: for<'de> Deserialize<'de>>(dir: &Dir, id: &str) -> io::Result<T> {
    let bytes = dir.read(&file_name(id), MAX_JSON_BYTES)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn read_request(dir: &Dir, id: &str) -> io::Result<Request> {
    let mut r: Request = read_json(dir, id)?;
    r.id = id.to_owned();
    if let Some(p) = &mut r.playlist {
        limits::cut_in_place(p, MAX_REQUEST_PLAYLIST_BYTES);
    }
    limits::cut_in_place(&mut r.requested_by, MAX_TEXT_BYTES);
    Ok(r)
}

fn read_status_in(dir: &Dir, id: &str) -> io::Result<Status> {
    let mut s: Status = read_json(dir, id)?;
    s.id = id.to_owned();
    s.clamp();
    Ok(s)
}

/// The oldest `max` requests (by numeric id prefix), parsed. Unparseable or
/// unreadable files among them (including symlinks) are returned separately
/// with their id and error. The third value counts every request file, so
/// callers can tell how many were left out.
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

/// All parseable requests, oldest first by numeric id prefix. Unparseable
/// files are returned separately with their id and error.
pub fn list_requests(root: &Path) -> (Vec<Request>, Vec<(String, io::Error)>) {
    let (ok, bad, _) = list_oldest_requests(root, usize::MAX);
    (ok, bad)
}

/// The newest `MAX_LISTED_REQUESTS` parseable status files, oldest first
/// (contract: "Limits"), cut to the contract's limits.
pub fn list_statuses(root: &Path) -> Vec<Status> {
    list_newest_statuses(root, MAX_LISTED_REQUESTS)
}

/// The newest `max` parseable status files, oldest first.
pub fn list_newest_statuses(root: &Path, max: usize) -> Vec<Status> {
    let Ok(dir) = open_dir(root, STATUS, false) else {
        return Vec::new();
    };
    let ids = ids_in(&dir);
    let skip = ids.len().saturating_sub(max);
    ids.into_iter()
        .skip(skip)
        .filter_map(|id| read_status_in(&dir, &id).ok())
        .collect()
}

/// One download as clients list it: a request, its status, or both.
#[derive(Debug, Clone)]
pub struct Recent {
    pub id: String,
    pub request: Option<Request>,
    pub status: Option<Status>,
}

/// The newest `MAX_LISTED_REQUESTS` ids over requests and status files
/// together, newest first, each with whatever of the two could be read;
/// plus the number of distinct ids in both directories, so clients can
/// say how many more there are (contract: "Limits").
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
    let rows = ids
        .into_iter()
        .take(MAX_LISTED_REQUESTS)
        .map(|id| Recent {
            request: requests.as_ref().and_then(|d| read_request(d, &id).ok()),
            status: statuses.as_ref().and_then(|d| read_status_in(d, &id).ok()),
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

pub fn read_status(root: &Path, id: &str) -> Option<Status> {
    read_status_in(&open_dir(root, STATUS, false).ok()?, id).ok()
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

/// Client-side "Clear": deletes status files in a final state whose request
/// file is gone. Returns how many were removed.
pub fn clear_finished(root: &Path) -> usize {
    list_statuses(root)
        .into_iter()
        .filter(|s| s.state.is_final() && is_cancelled(root, &s.id))
        .filter(|s| remove_status(root, &s.id).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
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

    #[test]
    fn requests_ordered_by_numeric_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for id in [
            "1791265297123-ffffffff",
            "999999999999-00000000",
            "1791265297123-0000000a",
            "10000000000000-aaaaaaaa",
        ] {
            let mut r = Request::new("x", None, false, "test");
            r.id = id.into();
            r.write(root).unwrap();
        }
        fs::write(request_path(root, "broken"), b"{").unwrap();
        let (ok, bad) = list_requests(root);
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

        let mut s = Status::new(&r, RequestState::Cancelled);
        s.write(root).unwrap();
        let other = Request::new("y", None, false, "pc");
        other.write(root).unwrap();
        Status::new(&other, RequestState::Done).write(root).unwrap();
        // Only final statuses without a request file are cleared.
        assert_eq!(clear_finished(root), 1);
        s.state = RequestState::Downloading;
        assert_eq!(list_statuses(root).len(), 1);
    }

    #[test]
    fn json_shape_matches_contract() {
        let json = r#"{"version":1,"id":"1-00000000","input":"q","playlist":null,"playlistFromSource":true,"requestedBy":"pixel-8","requestedAt":"2026-10-06T05:01:37Z"}"#;
        let r: Request = serde_json::from_str(json).unwrap();
        assert!(r.playlist_from_source);
        let s = Status::new(&r, RequestState::Downloading);
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "downloading");
        assert!(v.get("updatedAt").is_some());
        assert!(v["progress"].is_null());
    }

    /// Audit finding 2: a symlinked `.tunemesh/status` or `requests`
    /// directory must not let the worker write or delete files elsewhere.
    #[test]
    fn symlinked_tunemesh_dirs_are_refused() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("settings.json"), b"{}").unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        fs::create_dir_all(root.join(TUNEMESH_DIR)).unwrap();
        symlink(outside.path(), status_dir(root)).unwrap();
        symlink(outside.path(), requests_dir(root)).unwrap();

        let mut r = Request::new("x", None, false, "pc");
        r.id = "settings".into();
        let s = Status::new(&r, RequestState::Failed);
        assert!(s.write(root).is_err());
        assert!(r.write(root).is_err());
        assert_eq!(
            fs::read(outside.path().join("settings.json")).unwrap(),
            b"{}"
        );
        assert!(list_requests(root).0.is_empty());
        assert!(list_requests(root).1.is_empty());
        assert!(list_statuses(root).is_empty());
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
    fn oldest_requests_are_limited() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        for id in ["3-00000000", "1-00000000", "2-00000000"] {
            let mut r = Request::new("x", None, false, "pc");
            r.id = id.into();
            r.write(root).unwrap();
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
        s.warnings.push("w".into());
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["itemsDone"], 1);
        assert_eq!(v["itemsTotal"], 3);
        assert_eq!(v["failures"].as_array().unwrap().len(), MAX_FAILURES);
        assert_eq!(v["failures"][0]["item"], "t0");
        assert_eq!(v["warnings"][0], "w");
        let back: Status = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    /// Status files are cut to the contract's limits when read and written.
    #[test]
    fn status_contents_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let r = Request::new("x", None, false, "t");
        let mut s = Status::new(&r, RequestState::Done);
        s.title = Some("t".repeat(5_000));
        s.message = Some("é".repeat(5_000));
        s.warnings = (0..80)
            .map(|i| format!("{i}{}", "w".repeat(2_000)))
            .collect();
        s.failures = (0..300)
            .map(|i| Failure {
                item: format!("{i}"),
                error: "e".repeat(2_000),
            })
            .collect();
        s.tracks = vec!["a.mp3".into(); 20_000];
        // Written as is (a hostile device would), then read back cut.
        let json = serde_json::to_vec(&s).unwrap();
        fs::create_dir_all(status_dir(root)).unwrap();
        fs::write(status_path(root, &r.id), json).unwrap();
        let back = read_status(root, &r.id).unwrap();
        assert_eq!(back.title.unwrap().len(), MAX_TEXT_BYTES);
        let m = back.message.unwrap();
        assert!(m.len() <= MAX_MESSAGE_BYTES && m.len() > MAX_MESSAGE_BYTES - 2);
        assert_eq!(back.warnings.len(), MAX_WARNINGS);
        assert!(back.warnings.iter().all(|w| w.len() == MAX_TEXT_BYTES));
        assert_eq!(back.failures.len(), MAX_FAILURES);
        assert_eq!(back.failures[0].error.len(), MAX_TEXT_BYTES);
        assert_eq!(back.tracks.len(), MAX_STATUS_TRACKS);
        // The writer cuts too.
        s.write(root).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(status_path(root, &r.id)).unwrap()).unwrap();
        assert_eq!(written["warnings"].as_array().unwrap().len(), MAX_WARNINGS);
        // Larger than 1 MiB: not read.
        fs::write(
            status_path(root, &r.id),
            vec![b' '; MAX_JSON_BYTES as usize + 1],
        )
        .unwrap();
        assert!(read_status(root, &r.id).is_none());
    }

    /// Clients read only the newest ids over requests and status files.
    #[test]
    fn recent_reads_the_newest_ids() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for i in 0..MAX_LISTED_REQUESTS + 50 {
            let mut r = Request::new(&format!("in {i}"), None, false, "t");
            r.id = format!("{}-{:08x}", 1_000 + i, i);
            r.write(root).unwrap();
            if i % 2 == 0 {
                Status::new(&r, RequestState::Queued).write(root).unwrap();
            }
        }
        // A status without a request, newest of all.
        let mut lone = Request::new("lone", None, false, "t");
        lone.id = "9999-00000000".into();
        Status::new(&lone, RequestState::Done).write(root).unwrap();
        let (rows, total) = recent(root);
        assert_eq!(total, MAX_LISTED_REQUESTS + 51);
        assert_eq!(rows.len(), MAX_LISTED_REQUESTS);
        assert_eq!(rows[0].id, "9999-00000000");
        assert!(rows[0].request.is_none() && rows[0].status.is_some());
        let newest = &rows[1];
        assert_eq!(
            newest.request.as_ref().unwrap().input,
            format!("in {}", MAX_LISTED_REQUESTS + 49)
        );
        assert!(
            rows.windows(2)
                .all(|w| order_key(&w[0].id) > order_key(&w[1].id))
        );
        assert_eq!(request_ids(root).len(), MAX_LISTED_REQUESTS + 50);
        assert_eq!(
            list_statuses(root).len(),
            (MAX_LISTED_REQUESTS + 50) / 2 + 1
        );
    }
}
