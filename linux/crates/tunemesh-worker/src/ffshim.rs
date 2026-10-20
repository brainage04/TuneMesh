//! Confines the ffmpeg and ffprobe runs of yt-dlp's postprocessors.
//!
//! A downloaded "audio" file can really be an HLS playlist, an ffconcat
//! script or another format whose demuxer opens further files: a site may
//! serve `#EXTM3U` with `file:///home/...` entries as `audio/mpeg`. When
//! ffprobe or ffmpeg then open the download, that demuxer reads local files,
//! and the result would land in the library and sync to every device.
//!
//! yt-dlp therefore gets `--ffmpeg-location <work dir>/ffmpeg-shim`, where
//! `ffmpeg` and `ffprobe` are symlinks to this executable. Started under
//! those names, the worker execs the real program with, for every input,
//! `-format_whitelist` (plain audio, video and image containers only, none
//! that follow references) and `-protocol_whitelist file`, after checking
//! that every input is a `file:` path inside the job directory. `--postprocessor-args` cannot do this: it does not reach the
//! ffprobe calls.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::{env, fs, io};

/// Demuxers ffmpeg may use on files yt-dlp hands it. None of them opens
/// other files or URLs (`mov` ignores data references unless
/// `enable_drefs` is set). `ffmetadata` reads the chapter files yt-dlp
/// writes itself.
pub const FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,ogg,mp3,flac,wav,aac,aiff,mpegts,flv,\
                           ffmetadata,image2,jpeg_pipe,png_pipe,webp_pipe,gif";

const ENV_FFMPEG: &str = "TUNEMESH_REAL_FFMPEG";
const ENV_FFPROBE: &str = "TUNEMESH_REAL_FFPROBE";
const ENV_ROOT: &str = "TUNEMESH_SHIM_ROOT";

const CONFINE: [&str; 4] = ["-format_whitelist", FORMATS, "-protocol_whitelist", "file"];

/// Which program the process was started as, if it is a shim.
pub fn invoked_as() -> Option<&'static str> {
    let argv0 = env::args_os().next()?;
    match Path::new(&argv0).file_name()?.as_bytes() {
        b"ffmpeg" => Some("ffmpeg"),
        b"ffprobe" => Some("ffprobe"),
        _ => None,
    }
}

/// `/proc/self/exe` after the binary was replaced (a package upgrade).
fn strip_deleted(p: PathBuf) -> PathBuf {
    match p.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
        Some(b) => PathBuf::from(OsStr::from_bytes(b)),
        None => p,
    }
}

/// This executable's path, resolved once (call early, before an upgrade
/// can replace the file).
pub fn exe() -> io::Result<&'static Path> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(|| env::current_exe().ok().map(strip_deleted))
        .as_deref()
        .ok_or_else(|| io::Error::other("cannot find this executable"))
}

/// Creates `<dir>/ffmpeg-shim/{ffmpeg,ffprobe}` and returns the directory
/// for `--ffmpeg-location`. A link that already resolves to a file is kept.
pub fn install(dir: &Path) -> io::Result<PathBuf> {
    let dir = dir.join("ffmpeg-shim");
    fs::create_dir_all(&dir)?;
    for name in ["ffmpeg", "ffprobe"] {
        let link = dir.join(name);
        if fs::metadata(&link).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(exe()?, &link)?;
    }
    Ok(dir)
}

/// Environment a yt-dlp command needs for the shim.
pub fn env(ffmpeg: &Path, ffprobe: &Path, root: &Path) -> [(&'static str, OsString); 3] {
    [
        (ENV_FFMPEG, ffmpeg.into()),
        (ENV_FFPROBE, ffprobe.into()),
        (ENV_ROOT, root.into()),
    ]
}

/// Whether `path` is inside `root`. A path that does not exist yet (an
/// output) counts by its parent directory.
fn inside(path: &Path, root: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let resolved = path.canonicalize().or_else(|_| {
        let parent = path.parent().unwrap_or(Path::new("/"));
        parent
            .canonicalize()
            .map(|p| p.join(path.file_name().unwrap_or_default()))
    });
    resolved.is_ok_and(|p| p.starts_with(root))
}

/// The path of a `file:` argument (raw bytes, any encoding).
fn file_path(arg: &OsStr) -> Option<&Path> {
    arg.as_bytes()
        .strip_prefix(b"file:")
        .map(|b| Path::new(OsStr::from_bytes(b)))
}

/// Requires `arg` to be a `file:` path inside `root`.
fn check_input(arg: &OsStr, root: &Path) -> Result<(), String> {
    match file_path(arg) {
        Some(p) if inside(p, root) => Ok(()),
        Some(p) => Err(format!(
            "refusing {}: outside the work directory",
            p.display()
        )),
        None => Err(format!(
            "refusing input {}: only file: paths in the work directory",
            arg.to_string_lossy()
        )),
    }
}

/// The real program's arguments: the confinement options before every
/// input. Every input must be a `file:` path inside `root`: each `-i` value
/// for ffmpeg; for ffprobe the last argument, passed on as `-i`, so any other
/// positional argument makes ffprobe fail ("input already specified").
/// Any other `file:` argument (outputs, attachments) must be inside `root`
/// too. An ffprobe call whose arguments are all options (`-version`) has no
/// input and passes unchanged.
pub fn rewrite(program: &str, args: &[OsString], root: &Path) -> Result<Vec<OsString>, String> {
    for a in args {
        if let Some(p) = file_path(a)
            && !inside(p, root)
        {
            return Err(format!(
                "refusing {}: outside the work directory",
                p.display()
            ));
        }
    }
    let mut out: Vec<OsString> = Vec::with_capacity(args.len() + 8);
    if program == "ffprobe" {
        if args.iter().all(|a| a.as_bytes().starts_with(b"-")) {
            return Ok(args.to_vec());
        }
        let (input, rest) = args.split_last().ok_or("no arguments")?;
        check_input(input, root)?;
        out.extend(CONFINE.iter().map(OsString::from));
        out.extend(rest.iter().cloned());
        out.push("-i".into());
        out.push(input.clone());
    } else {
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "-i" {
                let input = it.next().ok_or("-i without a value")?;
                check_input(input, root)?;
                out.extend(CONFINE.iter().map(OsString::from));
                out.push(a.clone());
                out.push(input.clone());
            } else {
                out.push(a.clone());
            }
        }
    }
    Ok(out)
}

/// Runs as the `ffmpeg` or `ffprobe` shim; returns only on error.
pub fn main(program: &str) -> std::process::ExitCode {
    let real = env::var_os(if program == "ffmpeg" {
        ENV_FFMPEG
    } else {
        ENV_FFPROBE
    });
    let root = env::var_os(ENV_ROOT)
        .map(PathBuf::from)
        .and_then(|r| r.canonicalize().ok());
    let (Some(real), Some(root)) = (real, root) else {
        eprintln!(
            "tunemesh-worker ({program} shim): {ENV_FFMPEG}/{ENV_FFPROBE}/{ENV_ROOT} are not set"
        );
        return std::process::ExitCode::from(2);
    };
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    match rewrite(program, &args, &root) {
        Ok(args) => {
            let e = Command::new(real).args(args).exec();
            eprintln!("tunemesh-worker ({program} shim): {e}");
        }
        Err(e) => eprintln!("tunemesh-worker ({program} shim): {e}"),
    }
    std::process::ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn adds_whitelists_before_each_input() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path().canonicalize().unwrap();
        let a = format!("file:{}/a.webm", r.display());
        let b = format!("file:{}/a.meta", r.display());
        let out = rewrite(
            "ffmpeg",
            &os(&["-y", "-i", &a, "-i", &b, "-c", "copy", &a]),
            &r,
        )
        .unwrap();
        let s: Vec<_> = out.iter().map(|x| x.to_str().unwrap()).collect();
        assert_eq!(s.iter().filter(|x| **x == "-format_whitelist").count(), 2);
        let first_i = s.iter().position(|x| *x == "-i").unwrap();
        assert_eq!(s[first_i - 4], "-format_whitelist");
        assert_eq!(s[first_i - 1], "file");
        let out = rewrite("ffprobe", &os(&["-show_streams", &a]), &r).unwrap();
        assert_eq!(out[0], "-format_whitelist");
        assert_eq!(out[4..], os(&["-show_streams", "-i", &a])[..]);
        assert_eq!(
            rewrite("ffprobe", &os(&["-version"]), &r).unwrap(),
            os(&["-version"])
        );
        // Version checks pass through.
        assert_eq!(
            rewrite("ffmpeg", &os(&["-bsfs"]), &r).unwrap(),
            os(&["-bsfs"])
        );
    }

    #[test]
    fn refuses_paths_outside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path().canonicalize().unwrap();
        assert!(rewrite("ffprobe", &os(&["-show_streams", "file:/etc/passwd"]), &r).is_err());
        let sneaky = format!("file:{}/../x.mp3", r.display());
        assert!(rewrite("ffmpeg", &os(&["-i", &sneaky, "out.mp3"]), &r).is_err());
        std::os::unix::fs::symlink("/etc", r.join("etc")).unwrap();
        let via_link = format!("file:{}/etc/passwd", r.display());
        assert!(rewrite("ffmpeg", &os(&["-i", &via_link]), &r).is_err());
    }

    /// Review item 8: bare paths and non-UTF-8 paths are inputs too.
    #[test]
    fn refuses_bare_and_non_utf8_inputs() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let r = root.path().canonicalize().unwrap();
        let inside_bare = format!("{}/a.mp3", r.display());
        assert!(rewrite("ffmpeg", &os(&["-i", "/etc/passwd", "out.opus"]), &r).is_err());
        assert!(rewrite("ffmpeg", &os(&["-i", &inside_bare]), &r).is_err());
        assert!(rewrite("ffprobe", &os(&["-show_streams", "/etc/passwd"]), &r).is_err());
        assert!(rewrite("ffmpeg", &os(&["-i"]), &r).is_err());
        let mut raw = b"file:/etc/\xff".to_vec();
        raw.extend_from_slice(b"/x");
        let raw = OsString::from_vec(raw);
        assert!(rewrite("ffmpeg", &[OsString::from("-i"), raw.clone()], &r).is_err());
        assert!(rewrite("ffprobe", &[raw], &r).is_err());
        let mut in_root = format!("file:{}/", r.display()).into_bytes();
        in_root.extend_from_slice(b"\xff.mp3");
        assert!(
            rewrite(
                "ffmpeg",
                &[OsString::from("-i"), OsString::from_vec(in_root)],
                &r
            )
            .is_ok()
        );
    }

    /// Review item 3: after an upgrade `/proc/self/exe` ends in
    /// " (deleted)"; a working link is never replaced.
    #[test]
    fn shim_links_survive_upgrades() {
        assert_eq!(
            strip_deleted(PathBuf::from("/usr/bin/tunemesh-worker (deleted)")),
            PathBuf::from("/usr/bin/tunemesh-worker")
        );
        assert_eq!(strip_deleted(PathBuf::from("/a/b")), PathBuf::from("/a/b"));
        let d = tempfile::tempdir().unwrap();
        let shim = d.path().join("ffmpeg-shim");
        fs::create_dir_all(&shim).unwrap();
        let other = d.path().join("other-binary");
        fs::write(&other, b"x").unwrap();
        std::os::unix::fs::symlink(&other, shim.join("ffmpeg")).unwrap();
        std::os::unix::fs::symlink(d.path().join("gone (deleted)"), shim.join("ffprobe")).unwrap();
        install(d.path()).unwrap();
        assert_eq!(fs::read_link(shim.join("ffmpeg")).unwrap(), other);
        assert_eq!(fs::read_link(shim.join("ffprobe")).unwrap(), exe().unwrap());
    }
}
