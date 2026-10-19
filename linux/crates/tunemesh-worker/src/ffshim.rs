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
//! that every `file:` path on the command line is inside the work
//! directory. `--postprocessor-args` cannot do this: it does not reach the
//! ffprobe calls.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Creates `<work dir>/ffmpeg-shim/{ffmpeg,ffprobe}` and returns the
/// directory for `--ffmpeg-location`.
pub fn install(work_dir: &Path) -> io::Result<PathBuf> {
    let dir = work_dir.join("ffmpeg-shim");
    fs::create_dir_all(&dir)?;
    let exe = env::current_exe()?;
    for name in ["ffmpeg", "ffprobe"] {
        let link = dir.join(name);
        if fs::read_link(&link).is_ok_and(|t| t == exe) {
            continue;
        }
        let _ = fs::remove_file(&link);
        std::os::unix::fs::symlink(&exe, &link)?;
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

/// Whether `path` (absolute, or relative to the current directory) is
/// inside `root`. A path that does not exist yet (an output) counts by its
/// parent directory.
fn inside(path: &str, root: &Path) -> bool {
    let path = Path::new(path);
    let resolved = path.canonicalize().or_else(|_| {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        parent
            .canonicalize()
            .map(|p| p.join(path.file_name().unwrap_or_default()))
    });
    resolved.is_ok_and(|p| p.starts_with(root))
}

/// The real program's arguments: the confinement options before every
/// input of ffmpeg, or first for ffprobe. Refuses `file:` paths outside
/// `root`.
pub fn rewrite(program: &str, args: &[OsString], root: &Path) -> Result<Vec<OsString>, String> {
    for a in args {
        if let Some(path) = a.to_str().and_then(|s| s.strip_prefix("file:"))
            && !inside(path, root)
        {
            return Err(format!("refusing {path}: outside the work directory"));
        }
    }
    let mut out: Vec<OsString> = Vec::with_capacity(args.len() + 8);
    if program == "ffprobe" {
        out.extend(CONFINE.iter().map(OsString::from));
        out.extend(args.iter().cloned());
    } else {
        for a in args {
            if a == "-i" {
                out.extend(CONFINE.iter().map(OsString::from));
            }
            out.push(a.clone());
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
        assert_eq!(out.len(), 6);
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
}
