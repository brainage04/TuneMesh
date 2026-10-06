//! Atomic file replacement: write `.~tunemesh-<random>.tmp` next to the
//! target, then rename it over the target.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Prefix of every temporary file TuneMesh writes (ignored by `.stignore`).
pub const TMP_PREFIX: &str = ".~tunemesh-";

/// A fresh temporary path in `dir`.
pub fn temp_path_in(dir: &Path) -> PathBuf {
    dir.join(format!("{TMP_PREFIX}{:08x}.tmp", fastrand::u32(..)))
}

/// Atomically replaces `target` with `contents`, creating parent directories.
pub fn write(target: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    fs::create_dir_all(dir)?;
    let tmp = temp_path_in(dir);
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Moves `from` to `to` without ever exposing a partial file at `to`: a plain
/// rename when both are on one filesystem, otherwise a copy into a temporary
/// file next to `to` that is then renamed into place.
pub fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    let dir = to
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    fs::create_dir_all(dir)?;
    match fs::rename(from, to) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
        Err(e) => return Err(e),
    }
    let tmp = temp_path_in(dir);
    let result = (|| {
        fs::copy(from, &tmp)?;
        File::open(&tmp)?.sync_all()?;
        fs::rename(&tmp, to)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
        return result;
    }
    fs::remove_file(from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_replaces_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("sub/file.txt");
        write(&target, b"one").unwrap();
        write(&target, b"two").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(dir.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec!["file.txt"]);
    }
}
