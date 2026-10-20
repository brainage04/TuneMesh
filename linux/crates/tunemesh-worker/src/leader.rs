//! One downloader per library (contract: "One downloader per library").
//!
//! The active worker rewrites `.tunemesh/worker.json` every minute. A worker
//! that finds another worker's heartbeat younger than three minutes stays
//! idle and checks again on every poll; it takes over once that heartbeat
//! goes stale (or at once when the other worker runs on this machine, in the
//! same process-id namespace, and its process is gone). An active worker that finds someone else's fresh
//! heartbeat in the file (two workers started at once, and Syncthing kept
//! the other's write) stops after handing its current request back to the
//! queue.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tunemesh_core::worker::{self, HEARTBEAT_INTERVAL, WorkerInfo};

use crate::{config, log, signals};

static LOST: AtomicBool = AtomicBool::new(false);

/// Whether this worker lost the downloader role while working.
pub fn lost() -> bool {
    LOST.load(Ordering::SeqCst)
}

struct State {
    root: PathBuf,
    me: WorkerInfo,
    active: bool,
    last_beat: Option<Instant>,
    /// Device last reported as the downloader, to log changes only.
    waiting_for: Option<String>,
}

#[derive(Clone)]
pub struct Leader(Arc<Mutex<State>>);

fn pid_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 || pid == std::process::id() {
        return false;
    }
    // SAFETY: signal 0 only checks that the process exists.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    r == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn random_instance() -> String {
    format!("{:016x}", fastrand::u64(..))
}

impl State {
    fn is_me(&self, other: &WorkerInfo) -> bool {
        other.instance == self.me.instance
    }

    /// Whether `other` (fresh) keeps this worker from downloading.
    fn blocks(&self, other: &WorkerInfo) -> bool {
        if self.is_me(other) || !other.is_fresh() {
            return false;
        }
        // Another worker on this machine whose process is gone has crashed.
        // Device names are not unique, so only the machine id counts; a
        // heartbeat without one is honoured until it goes stale.
        other.machine.is_empty() || other.machine != self.me.machine || pid_alive(other.pid)
    }

    fn write_beat(&mut self) {
        self.me.heartbeat_at = tunemesh_core::now_rfc3339();
        if let Err(e) = worker::write(&self.root, &self.me) {
            log!("writing worker.json: {e}");
        }
        self.last_beat = Some(Instant::now());
    }

    /// Heartbeat while active: yields when another worker's beat is in the
    /// file.
    fn beat(&mut self) {
        if let Some(other) = worker::read(&self.root)
            && self.blocks(&other)
        {
            log!(
                "{} ({}) is now the downloader; stopping downloads here",
                other.device,
                other.instance
            );
            self.active = false;
            self.waiting_for = Some(other.device);
            LOST.store(true, Ordering::SeqCst);
            return;
        }
        self.write_beat();
    }
}

impl Leader {
    pub fn new(root: &Path, device: String, machine: String) -> Self {
        let me = WorkerInfo {
            version: 1,
            device,
            machine,
            instance: random_instance(),
            pid: std::process::id(),
            worker_version: config::VERSION.to_owned(),
            heartbeat_at: String::new(),
        };
        let leader = Self(Arc::new(Mutex::new(State {
            root: root.to_path_buf(),
            me,
            active: false,
            last_beat: None,
            waiting_for: None,
        })));
        let l = leader.clone();
        thread::Builder::new()
            .name("heartbeat".into())
            .spawn(move || {
                while !signals::stop_requested() {
                    signals::sleep(Duration::from_secs(5));
                    l.tick();
                }
            })
            .expect("spawning the heartbeat thread");
        leader
    }

    /// One heartbeat-thread step: beats when due, never after a stop was
    /// requested or the role released.
    fn tick(&self) {
        let mut s = self.lock();
        if !signals::stop_requested()
            && s.active
            && s.last_beat
                .is_none_or(|t| t.elapsed() >= HEARTBEAT_INTERVAL)
        {
            s.beat();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether this worker may process requests now; takes over the role when
    /// no other worker holds it.
    pub fn may_work(&self) -> bool {
        let mut s = self.lock();
        if s.active {
            return true;
        }
        if let Some(other) = worker::read(&s.root)
            && s.blocks(&other)
        {
            if s.waiting_for.as_deref() != Some(other.device.as_str()) {
                log!("downloads are handled by {}; waiting", other.device);
                s.waiting_for = Some(other.device);
            }
            return false;
        }
        log!("this worker ({}) is now the downloader", s.me.device);
        s.active = true;
        s.waiting_for = None;
        LOST.store(false, Ordering::SeqCst);
        s.write_beat();
        true
    }

    /// On exit: removes `worker.json` if it is still ours.
    pub fn release(&self) {
        let mut s = self.lock();
        // The heartbeat thread must not write the file again.
        s.active = false;
        if worker::read(&s.root).is_some_and(|w| s.is_me(&w)) {
            let _ = worker::remove(&s.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(device: &str, pid: u32, at: String) -> WorkerInfo {
        other_on(device, "machine-b", pid, at)
    }

    fn other_on(device: &str, machine: &str, pid: u32, at: String) -> WorkerInfo {
        WorkerInfo {
            version: 1,
            device: device.into(),
            machine: machine.into(),
            instance: "other".into(),
            pid,
            worker_version: "0".into(),
            heartbeat_at: at,
        }
    }

    #[test]
    fn defers_to_a_fresh_worker_and_takes_over_stale_or_dead_ones() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let leader = Leader::new(root, "pc".into(), "machine-a".into());

        worker::write(root, &other("laptop", 1, tunemesh_core::now_rfc3339())).unwrap();
        assert!(!leader.may_work());
        // A stale heartbeat (an hour old) is taken over.
        worker::write(root, &other("laptop", 1, "2020-01-01T00:00:00Z".into())).unwrap();
        assert!(leader.may_work());
        assert_eq!(worker::read(root).unwrap().device, "pc");

        // Another device's beat appearing while active makes this one yield.
        worker::write(root, &other("laptop", 1, tunemesh_core::now_rfc3339())).unwrap();
        leader.lock().beat();
        assert!(lost());
        assert!(!leader.may_work());

        // Same machine, process gone: taken over at once.
        let dead = 0x3fff_fff0; // above any pid_max
        worker::write(
            root,
            &other_on("pc", "machine-a", dead, tunemesh_core::now_rfc3339()),
        )
        .unwrap();
        assert!(leader.may_work());
        assert!(!lost());
        leader.release();
        assert!(worker::read(root).is_none());
    }

    /// Two machines that share a device name: a fresh heartbeat whose pid
    /// does not exist here belongs to the other machine and is honoured, as
    /// is one from an older worker without a machine id.
    #[test]
    fn same_device_name_on_another_machine_blocks() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let leader = Leader::new(root, "fedora".into(), "machine-a".into());
        let dead = 0x3fff_fff0;
        worker::write(
            root,
            &other_on("fedora", "machine-b", dead, tunemesh_core::now_rfc3339()),
        )
        .unwrap();
        assert!(!leader.may_work());
        worker::write(
            root,
            &other_on("fedora", "", dead, tunemesh_core::now_rfc3339()),
        )
        .unwrap();
        assert!(!leader.may_work());
        assert_eq!(worker::read(root).unwrap().machine, "");
    }

    /// Review item 9: after `release` the heartbeat thread writes nothing.
    #[test]
    fn no_heartbeat_after_release() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let leader = Leader::new(root, "pc".into(), "machine-a".into());
        assert!(leader.may_work());
        leader.release();
        assert!(worker::read(root).is_none());
        leader.lock().last_beat = None;
        leader.tick();
        assert!(worker::read(root).is_none());
    }
}
