//! Each worker process works in its own `<work dir>/run/<id>/` (job
//! directories, yt-dlp's temporary files, the ffmpeg shim), holding an
//! exclusive `flock` on `<id>/lock` for as long as it runs. Several workers
//! may share one work directory (the desktop app's child and a user service
//! both default to `~/.cache/tunemesh/worker`); each removes only run
//! directories whose lock nobody holds, so a waiting worker never deletes
//! the downloads of the active one. Caches (`cache/`,
//! `spotify-queries.json`) stay shared in the work directory.

use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A run directory still being set up; others leave it alone this long.
const SETUP_GRACE: Duration = Duration::from_secs(3600);
const NEW_PREFIX: &str = ".new-";

pub struct RunDir {
    path: PathBuf,
    _lock: File,
}

/// Takes an exclusive lock on `file` without waiting.
fn try_lock(file: &File) -> io::Result<bool> {
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(false)
    } else {
        Err(e)
    }
}

fn older_than(path: &Path, age: Duration) -> bool {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|a| a > age))
}

/// Removes run directories no running worker owns.
fn sweep(runs: &Path) {
    let Ok(entries) = fs::read_dir(runs) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name();
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if name.to_string_lossy().starts_with(NEW_PREFIX) {
            if older_than(&path, SETUP_GRACE) {
                let _ = fs::remove_dir_all(&path);
            }
            continue;
        }
        let owned = match File::open(path.join("lock")) {
            Ok(lock) => match try_lock(&lock) {
                // Holding the lock while deleting keeps a late starter out.
                Ok(true) => {
                    let _ = fs::remove_dir_all(&path);
                    continue;
                }
                Ok(false) => true,
                Err(_) => true,
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(_) => true,
        };
        if !owned {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

impl RunDir {
    /// Cleans up after workers that are gone, then creates and locks this
    /// worker's run directory below `work_dir`.
    pub fn claim(work_dir: &Path) -> io::Result<Self> {
        let runs = work_dir.join("run");
        fs::create_dir_all(&runs)?;
        sweep(&runs);
        // Set up under a name the sweep skips, locked, then renamed.
        let id = format!("{:016x}", fastrand::u64(..));
        let staging = runs.join(format!("{NEW_PREFIX}{id}"));
        fs::create_dir(&staging)?;
        let lock = File::create(staging.join("lock"))?;
        if !try_lock(&lock)? {
            return Err(io::Error::other("could not lock a new run directory"));
        }
        let path = runs.join(&id);
        fs::rename(&staging, &path)?;
        Ok(Self { path, _lock: lock })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn jobs(&self) -> PathBuf {
        self.path.join("jobs")
    }

    pub fn tmp(&self) -> PathBuf {
        self.path.join("tmp")
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review item 2: a second worker on the same work directory leaves the
    /// first one's files alone and cleans up only after it is gone.
    #[test]
    fn workers_sharing_a_work_dir_keep_their_files() {
        let work = tempfile::tempdir().unwrap();
        let a = RunDir::claim(work.path()).unwrap();
        let a_file = a.jobs().join("1/Artist/Album/Song [abc].opus");
        fs::create_dir_all(a_file.parent().unwrap()).unwrap();
        fs::write(&a_file, b"x").unwrap();
        fs::create_dir_all(a.tmp().join("_MEI1")).unwrap();

        // A crashed worker's leftovers: no lock held.
        let crashed = work.path().join("run/deadbeef");
        fs::create_dir_all(crashed.join("jobs/2")).unwrap();
        File::create(crashed.join("lock")).unwrap();

        let b = RunDir::claim(work.path()).unwrap();
        assert_ne!(a.path(), b.path());
        assert!(a_file.exists() && a.tmp().join("_MEI1").exists());
        assert!(!crashed.exists());

        let b_path = b.path().to_path_buf();
        drop(b);
        assert!(!b_path.exists());
        assert!(a_file.exists());
        let a_path = a.path().to_path_buf();
        drop(a);
        assert!(!a_path.exists());
    }
}
