//! `.tunemesh/worker.json`: which device downloads for the library
//! (contract: "One downloader per library").

use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::TUNEMESH_DIR;
use crate::bounded::Bounded;
use crate::bounded_struct;
use crate::fsx::Dir;
use crate::limits::{MAX_DEVICE_BYTES, MAX_TIMESTAMP_BYTES, MAX_WORKER_ID_BYTES};

pub const FILE_NAME: &str = "worker.json";
/// The active worker rewrites its heartbeat this often.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
/// A heartbeat younger than this means that worker is active.
pub const FRESH_FOR: Duration = Duration::from_secs(180);

/// `machine`, `instance` and `workerVersion`.
pub type WorkerId = Bounded<MAX_WORKER_ID_BYTES>;

bounded_struct! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct WorkerInfo {
        pub version: u32,
        /// Device name of the downloading host, for display.
        pub device: Bounded<MAX_DEVICE_BYTES>,
        /// Hashed id of the machine and its process-id namespace. Only a worker
        /// with the same `machine` may treat this heartbeat as stale because
        /// `pid` no longer exists; empty in files from older workers.
        #[serde(default)]
        pub machine: WorkerId,
        /// Random per-process id, telling two workers on one device apart.
        #[serde(default)]
        pub instance: WorkerId,
        /// Process id of that worker on `device`.
        #[serde(default)]
        pub pid: u32,
        /// `tunemesh-worker --version` of that worker.
        #[serde(default)]
        pub worker_version: WorkerId,
        /// RFC 3339 time of the last heartbeat.
        pub heartbeat_at: Bounded<MAX_TIMESTAMP_BYTES>,
    }
}

impl WorkerInfo {
    /// Whether the heartbeat is within `FRESH_FOR` of now, in either
    /// direction (clocks of different devices drift a little).
    pub fn is_fresh(&self) -> bool {
        self.is_fresh_at(SystemTime::now())
    }

    pub fn is_fresh_at(&self, now: SystemTime) -> bool {
        let Ok(at) = humantime::parse_rfc3339_weak(&self.heartbeat_at) else {
            return false;
        };
        match now.duration_since(at) {
            Ok(age) => age < FRESH_FOR,
            Err(ahead) => ahead.duration() < FRESH_FOR,
        }
    }
}

fn open_dir(root: &Path, create: bool) -> io::Result<Dir> {
    Dir::open(root)?.sub(TUNEMESH_DIR, create)
}

/// The current `worker.json`, if present and valid.
pub fn read(root: &Path) -> Option<WorkerInfo> {
    let bytes = open_dir(root, false)
        .ok()?
        .read(FILE_NAME, crate::limits::MAX_JSON_BYTES)
        .ok()?;
    parse(&bytes)
}

/// A `worker.json` file's contents, if valid.
pub fn parse(bytes: &[u8]) -> Option<WorkerInfo> {
    serde_json::from_slice(bytes).ok()
}

/// The device downloading for this library right now, if any.
pub fn active(root: &Path) -> Option<WorkerInfo> {
    read(root).filter(WorkerInfo::is_fresh)
}

pub fn write(root: &Path, info: &WorkerInfo) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(info).map_err(io::Error::other)?;
    open_dir(root, true)?.write_atomic(FILE_NAME, &json)
}

pub fn remove(root: &Path) -> io::Result<()> {
    open_dir(root, false)?.remove_file(FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(at: &str) -> WorkerInfo {
        WorkerInfo {
            version: 1,
            device: "pc".into(),
            machine: "m".into(),
            instance: "abc".into(),
            pid: 7,
            worker_version: "0.1.0".into(),
            heartbeat_at: at.into(),
        }
    }

    #[test]
    fn freshness() {
        let now = humantime::parse_rfc3339("2026-10-06T05:10:00Z").unwrap();
        assert!(info("2026-10-06T05:08:00Z").is_fresh_at(now));
        assert!(!info("2026-10-06T05:06:59Z").is_fresh_at(now));
        assert!(info("2026-10-06T05:11:00Z").is_fresh_at(now));
        assert!(!info("2026-10-06T05:20:00Z").is_fresh_at(now));
        assert!(!info("garbage").is_fresh_at(now));
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).is_none());
        let i = info(&crate::now_rfc3339());
        write(dir.path(), &i).unwrap();
        assert_eq!(active(dir.path()), Some(i));
        let v: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join(".tunemesh/worker.json")).unwrap(),
        )
        .unwrap();
        assert!(v.get("heartbeatAt").is_some() && v.get("workerVersion").is_some());
        remove(dir.path()).unwrap();
        assert!(read(dir.path()).is_none());
    }

    /// Every field is cut when read (contract: "Limits").
    #[test]
    fn fields_are_cut() {
        let long = "x".repeat(10_000);
        let json = serde_json::json!({
            "version": 1, "device": long, "machine": long, "instance": long,
            "pid": 1, "workerVersion": long, "heartbeatAt": long,
        });
        let w = parse(json.to_string().as_bytes()).unwrap();
        assert_eq!(w.device.len(), MAX_DEVICE_BYTES);
        for f in [&w.machine, &w.instance, &w.worker_version] {
            assert_eq!(f.len(), MAX_WORKER_ID_BYTES);
        }
        assert_eq!(w.heartbeat_at.len(), MAX_TIMESTAMP_BYTES);
        assert!(parse(br#"{"version":1,"device":"d","heartbeatAt":"t","pid":-1}"#).is_none());
    }
}
