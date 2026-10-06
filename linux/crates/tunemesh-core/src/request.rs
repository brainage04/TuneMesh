//! `.tunemesh/requests/<id>.json` and `.tunemesh/status/<id>.json`
//! (contract: "Download requests", "Download status").

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{TUNEMESH_DIR, atomic};

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
            updated_at: crate::now_rfc3339(),
        }
    }
}

pub fn requests_dir(root: &Path) -> PathBuf {
    root.join(TUNEMESH_DIR).join("requests")
}

pub fn status_dir(root: &Path) -> PathBuf {
    root.join(TUNEMESH_DIR).join("status")
}

pub fn request_path(root: &Path, id: &str) -> PathBuf {
    requests_dir(root).join(format!("{id}.json"))
}

pub fn status_path(root: &Path, id: &str) -> PathBuf {
    status_dir(root).join(format!("{id}.json"))
}

/// A new id: Unix milliseconds, `-`, 8 lowercase hex characters.
pub fn new_id() -> String {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{ms}-{:08x}", fastrand::u32(..))
}

/// Sort key: the numeric millisecond prefix, then the whole id.
pub fn order_key(id: &str) -> (u128, &str) {
    let prefix = id.split('-').next().unwrap_or("");
    (prefix.parse().unwrap_or(u128::MAX), id)
}

impl Request {
    pub fn new(input: &str, playlist: Option<String>, playlist_from_source: bool, device: &str) -> Self {
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
        atomic::write(&request_path(root, &self.id), &json)
    }
}

impl Status {
    pub fn write(&self, root: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        atomic::write(&status_path(root, &self.id), &json)
    }
}

/// Whether a request was cancelled: its request file no longer exists.
pub fn is_cancelled(root: &Path, id: &str) -> bool {
    !request_path(root, id).exists()
}

fn read_json_dir<T: for<'de> Deserialize<'de>>(dir: &Path) -> Vec<(String, io::Result<T>)> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with('.') || name.contains(".sync-conflict-") {
                return None;
            }
            let id = name.strip_suffix(".json")?.to_owned();
            let parsed = fs::read(e.path()).and_then(|b| serde_json::from_slice(&b).map_err(io::Error::other));
            Some((id, parsed))
        })
        .collect()
}

/// All parseable requests, oldest first by numeric id prefix. Unparseable
/// files are returned separately with their id and error.
pub fn list_requests(root: &Path) -> (Vec<Request>, Vec<(String, io::Error)>) {
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for (id, r) in read_json_dir::<Request>(&requests_dir(root)) {
        match r {
            Ok(mut req) => {
                req.id = id;
                ok.push(req);
            }
            Err(e) => bad.push((id, e)),
        }
    }
    ok.sort_by(|a, b| order_key(&a.id).cmp(&order_key(&b.id)));
    (ok, bad)
}

/// All parseable status files, oldest first.
pub fn list_statuses(root: &Path) -> Vec<Status> {
    let mut v: Vec<Status> = read_json_dir::<Status>(&status_dir(root))
        .into_iter()
        .filter_map(|(id, r)| r.ok().map(|mut s: Status| {
            s.id = id;
            s
        }))
        .collect();
    v.sort_by(|a, b| order_key(&a.id).cmp(&order_key(&b.id)));
    v
}

pub fn read_status(root: &Path, id: &str) -> Option<Status> {
    let bytes = fs::read(status_path(root, id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Client-side cancellation: deletes the request file.
pub fn cancel(root: &Path, id: &str) -> io::Result<()> {
    match fs::remove_file(request_path(root, id)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Client-side "Clear": deletes status files in a final state whose request
/// file is gone. Returns how many were removed.
pub fn clear_finished(root: &Path) -> usize {
    list_statuses(root)
        .into_iter()
        .filter(|s| s.state.is_final() && is_cancelled(root, &s.id))
        .filter(|s| fs::remove_file(status_path(root, &s.id)).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_format() {
        let id = new_id();
        let (ms, hex) = id.split_once('-').unwrap();
        assert!(ms.parse::<u64>().unwrap() > 1_700_000_000_000);
        assert_eq!(hex.len(), 8);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn requests_ordered_by_numeric_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for id in ["1791265297123-ffffffff", "999999999999-00000000", "1791265297123-0000000a", "10000000000000-aaaaaaaa"] {
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
            vec!["999999999999-00000000", "1791265297123-0000000a", "1791265297123-ffffffff", "10000000000000-aaaaaaaa"]
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
        let json = r#"{"version":1,"id":"1-00000000","input":"q","playlist":null,"playlistFromSource":true,"requestedBy":"thomass-s21","requestedAt":"2026-10-06T05:01:37Z"}"#;
        let r: Request = serde_json::from_str(json).unwrap();
        assert!(r.playlist_from_source);
        let s = Status::new(&r, RequestState::Downloading);
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "downloading");
        assert!(v.get("updatedAt").is_some());
        assert!(v["progress"].is_null());
    }
}
