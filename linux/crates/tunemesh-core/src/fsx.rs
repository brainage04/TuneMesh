//! Symlink-safe file access below the library root.
//!
//! Any device that shares the Syncthing folder can put symbolic links into
//! it. Every file and directory TuneMesh opens below the root therefore goes
//! through a [`Dir`] handle and `openat` with `O_NOFOLLOW`: a symlink, or
//! anything that is not a regular file or directory where one is expected, is
//! refused instead of followed. The root itself is opened normally, since the
//! user chose it.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags};

use crate::atomic::TMP_PREFIX;

/// What a directory entry is, without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

/// Raises the open-file soft limit to the hard limit (often 1024 against
/// 524288), so library walks and the app's other files never compete for
/// the last descriptors. Call once at startup; errors are ignored.
pub fn raise_fd_limit() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let limit = getrlimit(Resource::Nofile);
    if limit.current != limit.maximum {
        let _ = setrlimit(
            Resource::Nofile,
            Rlimit {
                current: limit.maximum,
                maximum: limit.maximum,
            },
        );
    }
}

impl Kind {
    fn from_type(t: FileType) -> Self {
        match t {
            FileType::RegularFile => Kind::File,
            FileType::Directory => Kind::Dir,
            FileType::Symlink => Kind::Symlink,
            _ => Kind::Other,
        }
    }
}

/// Size and modification time of a directory entry (not following symlinks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    pub kind: Kind,
    pub len: u64,
    /// Seconds since the Unix epoch.
    pub mtime: i64,
    /// Nanoseconds within that second.
    pub mtime_nsec: u32,
}

/// An open directory; all operations are relative to it.
#[derive(Debug)]
pub struct Dir {
    fd: OwnedFd,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Refuses anything but a single, ordinary path component.
fn component(name: &str) -> io::Result<&str> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(invalid(format!("not a single file name: {name:?}")));
    }
    Ok(name)
}

/// Splits a relative `/`-separated path into checked components.
fn components(rel: &str) -> io::Result<Vec<&str>> {
    rel.split('/')
        .filter(|c| !c.is_empty())
        .map(component)
        .collect()
}

fn refused(name: &str, what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("refusing “{name}”: {what}"),
    )
}

/// Maps the errors `O_NOFOLLOW`/`O_DIRECTORY` give for a symlink or a
/// non-directory to a clear refusal.
fn map_open_err(e: rustix::io::Errno, name: &str, dir: bool) -> io::Error {
    if e == rustix::io::Errno::LOOP || (e == rustix::io::Errno::NOTDIR && dir) {
        refused(
            name,
            if dir {
                "not a directory (or a symbolic link)"
            } else {
                "a symbolic link"
            },
        )
    } else {
        let e = io::Error::from(e);
        io::Error::new(e.kind(), format!("{name}: {e}"))
    }
}

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

impl Dir {
    /// Opens the directory at `path`, following symlinks in `path` itself.
    pub fn open(path: &Path) -> io::Result<Self> {
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| {
            io::Error::new(
                io::Error::from(e).kind(),
                format!("{}: {}", path.display(), io::Error::from(e)),
            )
        })?;
        Ok(Self { fd })
    }

    fn child_dir(&self, name: &str, create: bool) -> io::Result<Self> {
        match rustix::fs::openat(&self.fd, name, DIR_FLAGS, Mode::empty()) {
            Ok(fd) => Ok(Self { fd }),
            Err(rustix::io::Errno::NOENT) if create => {
                match rustix::fs::mkdirat(&self.fd, name, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => return Err(map_open_err(e, name, true)),
                }
                rustix::fs::openat(&self.fd, name, DIR_FLAGS, Mode::empty())
                    .map(|fd| Self { fd })
                    .map_err(|e| map_open_err(e, name, true))
            }
            Err(e) => Err(map_open_err(e, name, true)),
        }
    }

    /// Opens the subdirectory `rel` (`/`-separated, relative). Every
    /// component must be a real directory; with `create`, missing ones are
    /// created.
    pub fn sub(&self, rel: &str, create: bool) -> io::Result<Self> {
        let mut dir = self.try_clone()?;
        for c in components(rel)? {
            dir = dir.child_dir(c, create)?;
        }
        Ok(dir)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            fd: self.fd.try_clone()?,
        })
    }

    /// Takes an exclusive `flock` on this directory, waiting for it; held
    /// until this `Dir` (and every clone of its descriptor) is closed.
    pub fn lock_exclusive(&self) -> io::Result<()> {
        loop {
            match rustix::fs::flock(&self.fd, rustix::fs::FlockOperation::LockExclusive) {
                Err(rustix::io::Errno::INTR) => continue,
                r => return r.map_err(io::Error::from),
            }
        }
    }

    /// The directory holding `rel` and the final component's name.
    fn parent_of<'a>(&self, rel: &'a str, create: bool) -> io::Result<(Self, &'a str)> {
        let comps = components(rel)?;
        let (name, dirs) = comps
            .split_last()
            .ok_or_else(|| invalid(format!("empty path: {rel:?}")))?;
        let mut dir = self.try_clone()?;
        for c in dirs {
            dir = dir.child_dir(c, create)?;
        }
        Ok((dir, name))
    }

    /// Opens the regular file at `rel` for reading; symlinks anywhere on the
    /// way, FIFOs and devices are refused.
    pub fn open_file(&self, rel: &str) -> io::Result<File> {
        let (dir, name) = self.parent_of(rel, false)?;
        // O_NONBLOCK keeps a FIFO from blocking the open; it is refused below.
        let fd = rustix::fs::openat(
            &dir.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOCTTY,
            Mode::empty(),
        )
        .map_err(|e| map_open_err(e, name, false))?;
        let stat = rustix::fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(refused(name, "not a regular file"));
        }
        let flags = rustix::fs::fcntl_getfl(&fd)?;
        rustix::fs::fcntl_setfl(&fd, flags - OFlags::NONBLOCK)?;
        Ok(File::from(fd))
    }

    /// Reads the regular file `rel`, refusing files larger than `max` bytes.
    pub fn read(&self, rel: &str, max: u64) -> io::Result<Vec<u8>> {
        let file = self.open_file(rel)?;
        let mut bytes = Vec::new();
        file.take(max + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{rel}: larger than {max} bytes"),
            ));
        }
        Ok(bytes)
    }

    /// What `name` is, without following a symlink.
    pub fn kind(&self, name: &str) -> io::Result<Kind> {
        let stat = rustix::fs::statat(&self.fd, component(name)?, AtFlags::SYMLINK_NOFOLLOW)?;
        Ok(Kind::from_type(FileType::from_raw_mode(stat.st_mode)))
    }

    /// Kind, size and modification time of `name`, without following a
    /// symlink.
    pub fn metadata(&self, name: &str) -> io::Result<Meta> {
        let st = rustix::fs::statat(&self.fd, component(name)?, AtFlags::SYMLINK_NOFOLLOW)?;
        Ok(Meta {
            kind: Kind::from_type(FileType::from_raw_mode(st.st_mode)),
            len: st.st_size as u64,
            mtime: st.st_mtime as i64,
            mtime_nsec: st.st_mtime_nsec as u32,
        })
    }

    /// Opens the subdirectory `name` of this directory; a symlink (or
    /// anything but a directory) is refused.
    pub fn child(&self, name: &str) -> io::Result<Self> {
        self.child_dir(component(name)?, false)
    }

    /// Whether `rel` exists as a regular file (no symlinks on the way).
    pub fn is_file(&self, rel: &str) -> bool {
        self.parent_of(rel, false)
            .and_then(|(d, n)| d.kind(n))
            .is_ok_and(|k| k == Kind::File)
    }

    /// Size of the regular file `rel`.
    pub fn file_len(&self, rel: &str) -> io::Result<u64> {
        Ok(self.open_file(rel)?.metadata()?.len())
    }

    /// Entry names (valid UTF-8 only) with their kinds, `.` and `..` excluded.
    pub fn entries(&self) -> io::Result<Vec<(String, Kind)>> {
        self.entries_up_to(usize::MAX).map(|(v, _)| v)
    }

    /// At most `max` entries of this directory (in directory order), and
    /// whether that was all of them.
    pub fn entries_up_to(&self, max: usize) -> io::Result<(Vec<(String, Kind)>, bool)> {
        let mut out = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.fd)? {
            let entry = entry?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if out.len() == max {
                return Ok((out, false));
            }
            let Ok(name) = std::str::from_utf8(bytes) else {
                continue;
            };
            let kind = match entry.file_type() {
                FileType::Unknown => match self.kind(name) {
                    Ok(k) => k,
                    Err(_) => continue,
                },
                t => Kind::from_type(t),
            };
            out.push((name.to_owned(), kind));
        }
        Ok((out, true))
    }

    /// Atomically replaces the file `name` in this directory with
    /// `contents`: writes `.~tunemesh-<random>.tmp`, then renames it over
    /// `name`. A symlink at `name` is replaced, never followed.
    pub fn write_atomic(&self, name: &str, contents: &[u8]) -> io::Result<()> {
        let name = component(name)?;
        let (tmp, mut file) = self.create_temp()?;
        let result = (|| {
            file.write_all(contents)?;
            file.sync_all()?;
            drop(file);
            rustix::fs::renameat(&self.fd, tmp.as_str(), &self.fd, name).map_err(io::Error::from)
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.fd, tmp.as_str(), AtFlags::empty());
        }
        result
    }

    /// Writes `rel` atomically, creating missing parent directories.
    pub fn write_atomic_at(&self, rel: &str, contents: &[u8]) -> io::Result<()> {
        let (dir, name) = self.parent_of(rel, true)?;
        dir.write_atomic(name, contents)
    }

    fn create_temp(&self) -> io::Result<(String, File)> {
        loop {
            let tmp = format!("{TMP_PREFIX}{:08x}.tmp", fastrand::u32(..));
            match rustix::fs::openat(
                &self.fd,
                tmp.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o666),
            ) {
                Ok(fd) => return Ok((tmp, File::from(fd))),
                Err(rustix::io::Errno::EXIST) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Removes the directory entry `name` (a symlink itself, never its target).
    pub fn remove_file(&self, name: &str) -> io::Result<()> {
        rustix::fs::unlinkat(&self.fd, component(name)?, AtFlags::empty()).map_err(io::Error::from)
    }

    /// Removes the empty subdirectory `name` (a symlink is refused).
    pub fn remove_dir(&self, name: &str) -> io::Result<()> {
        rustix::fs::unlinkat(&self.fd, component(name)?, AtFlags::REMOVEDIR)
            .map_err(io::Error::from)
    }

    /// Renames `from` to `to` inside this directory, replacing `to`.
    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        rustix::fs::renameat(&self.fd, component(from)?, &self.fd, component(to)?)
            .map_err(io::Error::from)
    }

    /// Moves the file at `from` (outside the library) to `name` in this
    /// directory without replacing anything there and without ever exposing
    /// a partial file: a rename on one filesystem, otherwise a copy into a
    /// temporary file that is then renamed into place. Fails with
    /// `AlreadyExists` when `name` exists.
    pub fn move_in(&self, from: &Path, name: &str) -> io::Result<()> {
        let name = component(name)?;
        match rename_noreplace(rustix::fs::CWD, from.as_os_str(), &self.fd, name.as_ref()) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
            Err(e) => return Err(e),
        }
        let mut source = File::from(
            rustix::fs::open(
                from,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(io::Error::from)?,
        );
        if !source.metadata()?.is_file() {
            return Err(refused(&from.display().to_string(), "not a regular file"));
        }
        let (tmp, mut file) = self.create_temp()?;
        let result = (|| {
            io::copy(&mut source, &mut file)?;
            file.sync_all()?;
            drop(file);
            rename_noreplace(&self.fd, tmp.as_ref(), &self.fd, name.as_ref())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.fd, tmp.as_str(), AtFlags::empty());
            return result;
        }
        std::fs::remove_file(from)
    }

    /// Bytes available to unprivileged users on this directory's filesystem.
    pub fn free_space(&self) -> io::Result<u64> {
        let s = rustix::fs::fstatvfs(&self.fd)?;
        Ok(s.f_bavail.saturating_mul(s.f_frsize))
    }
}

/// `renameat2(RENAME_NOREPLACE)`, with a check-then-rename fallback for
/// filesystems that do not support the flag.
fn rename_noreplace(
    from_dir: impl std::os::fd::AsFd,
    from: &OsStr,
    to_dir: impl std::os::fd::AsFd,
    to: &OsStr,
) -> io::Result<()> {
    match rustix::fs::renameat_with(&from_dir, from, &to_dir, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS) => {
            match rustix::fs::statat(&to_dir, to, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(_) => Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{} exists", Path::new(to).display()),
                )),
                Err(rustix::io::Errno::NOENT) => {
                    rustix::fs::renameat(&from_dir, from, &to_dir, to).map_err(io::Error::from)
                }
                Err(e) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn refuses_symlinks_on_the_way_and_at_the_end() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), b"key").unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = Dir::open(lib.path()).unwrap();
        symlink(outside.path(), lib.path().join("link")).unwrap();
        symlink(outside.path().join("secret"), lib.path().join("file")).unwrap();
        fs::create_dir(lib.path().join("real")).unwrap();
        fs::write(lib.path().join("real/x"), b"ok").unwrap();

        assert!(root.read("link/secret", 100).is_err());
        assert!(root.read("file", 100).is_err());
        assert_eq!(root.read("real/x", 100).unwrap(), b"ok");
        assert!(root.read("real/x", 1).is_err());
        assert!(root.sub("link", true).is_err());
        assert!(root.write_atomic_at("link/new", b"x").is_err());
        assert!(!outside.path().join("new").exists());
        assert!(!root.is_file("file"));
        assert!(root.is_file("real/x"));
        assert!(root.read("../etc/passwd", 100).is_err());
        assert!(root.remove_dir("link").is_err());
        assert!(outside.path().exists());
        fs::create_dir(lib.path().join("empty")).unwrap();
        root.remove_dir("empty").unwrap();

        // Replacing a symlink replaces the link, not its target.
        root.write_atomic("file", b"new").unwrap();
        assert_eq!(fs::read(outside.path().join("secret")).unwrap(), b"key");
        assert!(
            fs::symlink_metadata(lib.path().join("file"))
                .unwrap()
                .is_file()
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn move_in_never_replaces() {
        let work = tempfile::tempdir().unwrap();
        let lib = tempfile::tempdir().unwrap();
        let root = Dir::open(lib.path()).unwrap();
        let src = work.path().join("a");
        fs::write(&src, b"1").unwrap();
        fs::write(lib.path().join("b"), b"old").unwrap();
        assert_eq!(
            root.move_in(&src, "b").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        root.move_in(&src, "c").unwrap();
        assert_eq!(fs::read(lib.path().join("c")).unwrap(), b"1");
        assert!(!src.exists());
        let names: Vec<_> = root
            .entries()
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names.len(), 2);
        assert!(root.free_space().unwrap() > 0);
    }

    #[test]
    #[cfg_attr(miri, ignore = "file system")]
    fn entries_up_to_stops_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("f{i}")), b"").unwrap();
        }
        let d = Dir::open(dir.path()).unwrap();
        let (some, complete) = d.entries_up_to(4).unwrap();
        assert_eq!((some.len(), complete), (4, false));
        let (all, complete) = d.entries_up_to(10).unwrap();
        assert_eq!((all.len(), complete), (10, true));
    }
}
