//! Command line, defaults and the directories the worker uses.

use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{env, fs};

use anyhow::{Context, Result, bail};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "\
usage: tunemesh-worker [options]

  --library <dir>          library root (default: library_path from
                           $XDG_CONFIG_HOME/tunemesh/config.toml, else ~/Music/TuneMesh)
  --work-dir <dir>         scratch directory outside the library, owned by this
                           user with mode 0700 (default: $XDG_CACHE_HOME/tunemesh/worker)
  --yt-dlp <path>          yt-dlp to run (default: $TUNEMESH_YTDLP, else a managed
                           copy in $XDG_DATA_HOME/tunemesh/yt-dlp)
  --device <name>          name shown to other devices (default: host name)
  --max-items <n>          most items one request downloads (default: 5000)
  --max-file-size <size>   largest single download, e.g. 500M (default: 500M)
  --max-duration <time>    longest track, e.g. 4h (default: 4h)
  --child-timeout <time>   wall-clock limit per yt-dlp run (default: 30m)
  --max-pending <n>        requests looked at per scan, oldest first (default: 500)
  --min-free-space <size>  free space to keep on the library's and work
                           directory's filesystems (default: 2G)
  --poll-seconds <n>       request scan interval (default: 2)
  --self-test              check ffmpeg/ffprobe through the worker's confinement
                           (in --work-dir) and exit; needs no library
  --version                print the version and exit
  -h, --help               print this help and exit

environment: TUNEMESH_YTDLP, TUNEMESH_DENO, TUNEMESH_FFMPEG_DIR";

#[derive(Debug, Clone)]
pub struct Limits {
    pub max_items: usize,
    pub max_file_size: u64,
    pub max_duration: Duration,
    pub child_timeout: Duration,
    pub max_pending: usize,
    pub min_free_space: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_items: 5000,
            max_file_size: 500 * MIB,
            max_duration: Duration::from_secs(4 * 3600),
            child_timeout: Duration::from_secs(30 * 60),
            max_pending: 500,
            min_free_space: 2 * 1024 * MIB,
        }
    }
}

const MIB: u64 = 1024 * 1024;

#[derive(Debug)]
pub struct Args {
    pub library: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    pub yt_dlp: Option<PathBuf>,
    pub device: Option<String>,
    pub poll: Duration,
    pub limits: Limits,
    /// `--self-test`: check the ffmpeg setup and exit.
    pub self_test: bool,
}

pub enum Parsed {
    Run(Args),
    Help,
    Version,
}

/// `500M`, `2G`, `1.5G`, `800K` or plain bytes (binary multiples).
pub fn parse_size(s: &str) -> Result<u64> {
    let s = s.trim();
    let (num, mult) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => {
            let mult = match c.to_ascii_uppercase() {
                'K' => 1024,
                'M' => MIB,
                'G' => 1024 * MIB,
                'T' => 1024 * 1024 * MIB,
                _ => bail!("unknown size unit in {s:?}"),
            };
            (&s[..i], mult)
        }
        _ => (s, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .with_context(|| format!("bad size {s:?}"))?;
    if !n.is_finite() || n < 0.0 {
        bail!("bad size {s:?}");
    }
    Ok((n * mult as f64) as u64)
}

pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Parsed> {
    let mut a = Args {
        library: None,
        work_dir: None,
        yt_dlp: None,
        device: None,
        poll: Duration::from_secs(2),
        limits: Limits::default(),
        self_test: false,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = || -> Result<String> {
            inline
                .clone()
                .or_else(|| args.next())
                .with_context(|| format!("{flag} needs a value"))
        };
        let duration = |v: String| -> Result<Duration> {
            humantime::parse_duration(&v)
                .with_context(|| format!("{flag}: bad duration {v:?} (e.g. 30m, 4h)"))
        };
        match flag.as_str() {
            "--library" => a.library = Some(PathBuf::from(value()?)),
            "--work-dir" => a.work_dir = Some(PathBuf::from(value()?)),
            "--yt-dlp" => a.yt_dlp = Some(PathBuf::from(value()?)),
            "--device" => a.device = Some(value()?),
            "--max-items" => a.limits.max_items = value()?.parse().context("--max-items")?,
            "--max-file-size" => a.limits.max_file_size = parse_size(&value()?)?,
            "--max-duration" => a.limits.max_duration = duration(value()?)?,
            "--child-timeout" => a.limits.child_timeout = duration(value()?)?,
            "--max-pending" => a.limits.max_pending = value()?.parse().context("--max-pending")?,
            "--min-free-space" => a.limits.min_free_space = parse_size(&value()?)?,
            "--poll-seconds" => {
                let n: f64 = value()?.parse().context("--poll-seconds")?;
                a.poll = Duration::from_secs_f64(n.clamp(0.1, 3600.0));
            }
            "--self-test" => a.self_test = true,
            "--version" | "-V" => return Ok(Parsed::Version),
            "-h" | "--help" => return Ok(Parsed::Help),
            other => bail!("unknown argument {other:?}"),
        }
    }
    if a.limits.max_items == 0 || a.limits.max_pending == 0 {
        bail!("--max-items and --max-pending must be at least 1");
    }
    Ok(Parsed::Run(a))
}

/// The user's home: the real one inside a snap, else `$HOME`.
pub fn home() -> PathBuf {
    env::var_os("SNAP_REAL_HOME")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
}

pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn cache_home() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

/// Expands a leading `~/` like the desktop app does.
fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

#[derive(serde::Deserialize)]
struct AppConfig {
    library_path: Option<String>,
}

/// `library_path` from the desktop app's `config.toml`, else
/// `~/Music/TuneMesh`.
pub fn default_library() -> Result<PathBuf> {
    let file = config_home().join("tunemesh").join("config.toml");
    match fs::read_to_string(&file) {
        Ok(text) => {
            let c: AppConfig =
                toml::from_str(&text).with_context(|| format!("reading {}", file.display()))?;
            Ok(c.library_path
                .map_or_else(|| home().join("Music/TuneMesh"), |p| expand_home(&p)))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(home().join("Music/TuneMesh")),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

pub fn default_work_dir() -> PathBuf {
    cache_home().join("tunemesh").join("worker")
}

/// Creates `dir` with mode 0700 if missing, then refuses it unless it is a
/// real directory owned by this user that no one else can access: another
/// local user who controls the work directory could swap files between the
/// download and the move into the library.
pub fn private_dir(dir: &Path) -> Result<PathBuf> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e).with_context(|| format!("creating {}", dir.display())),
    }
    let meta = fs::symlink_metadata(dir).with_context(|| format!("checking {}", dir.display()))?;
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() {
        bail!(
            "{} is not a directory (or is a symbolic link)",
            dir.display()
        );
    }
    if meta.uid() != uid {
        bail!(
            "refusing {}: it is owned by uid {}, not by this user ({uid})",
            dir.display(),
            meta.uid()
        );
    }
    if meta.mode() & 0o077 != 0 {
        bail!(
            "refusing {}: other users can access it (mode {:o}); run `chmod 700 {}`",
            dir.display(),
            meta.mode() & 0o777,
            dir.display()
        );
    }
    Ok(dir.canonicalize()?)
}

/// An id for this machine and process-id namespace, written to
/// `worker.json` as `machine`. Two workers may judge each other's liveness by
/// process id only when their ids match: the device name alone is not
/// unique, and a sandbox (Flatpak, containers) has its own process ids.
/// The id is a hash, so the synced file does not reveal `/etc/machine-id`.
pub fn machine_id() -> String {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(compute_machine_id).clone()
}

/// Computed once per process: when the random fallback cannot be stored (no
/// writable data directory, as in a build sandbox), every call would
/// otherwise give a new id.
fn compute_machine_id() -> String {
    let base = ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| stored_random_id(&data_home().join("tunemesh").join("worker-id")));
    let pid_ns = fs::read_link("/proc/self/ns/pid")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    hash_id(&base, &pid_ns)
}

fn hash_id(base: &str, pid_ns: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("tunemesh-worker\0{base}\0{pid_ns}"));
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// A random id kept in `file` (mode 0600), created on first use. When it
/// cannot be stored, a fresh one per process: liveness is then never judged
/// by process id, which is the safe side.
fn stored_random_id(file: &Path) -> String {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Ok(id) = fs::read_to_string(file)
        && id.trim().len() == 32
    {
        return id.trim().to_owned();
    }
    let id = format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..));
    let stored = file.parent().is_some_and(|d| fs::create_dir_all(d).is_ok())
        && fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(file)
            .and_then(|mut f| f.write_all(id.as_bytes()))
            .is_ok();
    if !stored {
        // Another worker may have written it meanwhile.
        if let Ok(existing) = fs::read_to_string(file)
            && existing.trim().len() == 32
        {
            return existing.trim().to_owned();
        }
    }
    id
}

/// This host's name.
pub fn host_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into `buf`.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_owned();
    if ok && !name.is_empty() {
        name
    } else {
        "unknown device".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(s: &str) -> Result<Parsed> {
        parse_args(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_flags_and_defaults() {
        let Parsed::Run(a) = args(
            "--library /l --work-dir=/w --max-items 2 --max-file-size 1.5G --max-duration 90m",
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(a.library.as_deref(), Some(Path::new("/l")));
        assert_eq!(a.work_dir.as_deref(), Some(Path::new("/w")));
        assert_eq!(a.limits.max_items, 2);
        assert_eq!(a.limits.max_file_size, 1536 * MIB);
        assert_eq!(a.limits.max_duration, Duration::from_secs(5400));
        assert_eq!(a.limits.child_timeout, Duration::from_secs(1800));
        assert!(matches!(args("--version").unwrap(), Parsed::Version));
        assert!(args("--bogus").is_err());
        assert!(args("--max-items 0").is_err());
        assert!(args("--library").is_err());
        assert_eq!(parse_size("800K").unwrap(), 800 * 1024);
        assert_eq!(parse_size("12345").unwrap(), 12345);
        assert!(parse_size("5X").is_err());
    }

    #[test]
    fn machine_ids() {
        assert_eq!(machine_id(), machine_id());
        assert_eq!(machine_id().len(), 32);
        assert_ne!(hash_id("a", "pid:[1]"), hash_id("a", "pid:[2]"));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub/worker-id");
        let id = stored_random_id(&file);
        assert_eq!(stored_random_id(&file), id);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn work_dir_must_be_private() {
        let base = tempfile::tempdir().unwrap();
        let fresh = base.path().join("a/work");
        private_dir(&fresh).unwrap();
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let open = base.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(private_dir(&open).is_err());

        let link = base.path().join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(private_dir(&link).is_err());
    }
}
