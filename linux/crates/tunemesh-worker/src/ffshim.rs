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
//! that every input is a `file:` path inside the job directory. Only the
//! options yt-dlp's postprocessing uses are passed on (`OPTIONS`).
//! `--postprocessor-args` cannot do this: it does not reach the ffprobe
//! calls.

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

/// ffmpeg and ffprobe options the shim passes on: those yt-dlp's
/// postprocessors, the worker's `--postprocessor-args` and `--self-test`
/// use, with whether they take a value. Lookup is by name without the
/// leading `-` and without a `:stream` suffix. Anything else is refused:
/// many options read or write files (`-attach`, `-filter_script`,
/// `-passlogfile`, `-progress`, `-/opt` ...), and options such as
/// `-format_whitelist` would undo the confinement.
const OPTIONS: &[(&str, bool)] = &[
    // Global and informational.
    ("y", false),
    ("n", false),
    ("hide_banner", false),
    ("nostdin", false),
    ("version", false),
    ("bsfs", false),
    ("loglevel", true),
    ("v", true),
    // Inputs and stream selection.
    ("i", true),
    ("f", true),
    ("pattern_type", true),
    ("map", true),
    ("map_metadata", true),
    ("vn", false),
    ("an", false),
    ("dn", false),
    ("sn", false),
    ("ss", true),
    ("t", true),
    // Codecs and filters.
    ("c", true),
    ("codec", true),
    ("acodec", true),
    ("vcodec", true),
    ("b", true),
    ("q", true),
    ("qmin", true),
    ("qmax", true),
    ("vbr", true),
    ("bsf", true),
    ("vf", true),
    ("aspect", true),
    ("vtag", true),
    ("force_key_frames", true),
    ("strict", true),
    // Metadata and muxing.
    ("metadata", true),
    ("disposition", true),
    ("ignore_unknown", false),
    ("id3v2_version", true),
    ("write_id3v1", true),
    ("movflags", true),
    ("update", true),
    ("attach", true),
    // ffprobe output.
    ("show_format", false),
    ("show_streams", false),
    ("show_entries", true),
    ("select_streams", true),
    ("print_format", true),
    ("of", true),
];

/// Options whose value is a file the program opens.
const FILE_OPTIONS: &[&str] = &["i", "attach"];

/// The option name of `arg` (`-c:v` -> `c`), if it is an option.
fn option_name(arg: &OsStr) -> Option<&[u8]> {
    let b = arg.as_bytes().strip_prefix(b"-")?;
    if b.is_empty() {
        return None; // "-": stdin/stdout
    }
    Some(b.split(|c| *c == b':').next().unwrap_or(b))
}

/// One argument as ffmpeg and ffprobe read their command line.
#[derive(Debug, Clone, Copy)]
enum Arg<'a> {
    /// An option as given (`-c:v`), its name (`c`) and its value, if it
    /// takes one.
    Option {
        name: &'static str,
        arg: &'a OsString,
        value: Option<&'a OsString>,
    },
    /// An output (ffmpeg), the input (ffprobe), or `-`.
    Positional(&'a OsString),
}

impl Arg<'_> {
    fn is_input_option(&self) -> bool {
        matches!(self, Arg::Option { name: "i", .. })
    }

    fn push_to(&self, out: &mut Vec<OsString>) {
        match *self {
            Arg::Option { arg, value, .. } => {
                out.extend([Some(arg), value].into_iter().flatten().cloned())
            }
            Arg::Positional(a) => out.push(a.clone()),
        }
    }
}

/// Splits `args` the way ffmpeg does (an option takes the next argument as
/// its value whatever it looks like; `-i:x` is `-i`) and checks them:
/// unknown options are refused, every value of a file-taking option and
/// every positional argument (outputs, ffprobe's input) must be a `file:`
/// path inside `root`.
fn check_args<'a>(args: &'a [OsString], root: &Path) -> Result<Vec<Arg<'a>>, String> {
    let mut out = Vec::with_capacity(args.len());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match option_name(a) {
            Some(name) => {
                let known = OPTIONS.iter().find(|(o, _)| o.as_bytes() == name);
                let Some(&(name, takes_value)) = known else {
                    return Err(format!(
                        "refusing option {}: not one yt-dlp's postprocessing uses",
                        a.to_string_lossy()
                    ));
                };
                let value = if takes_value {
                    let value = it
                        .next()
                        .ok_or_else(|| format!("-{name} without a value"))?;
                    if FILE_OPTIONS.contains(&name) {
                        check_input(value, root)?;
                    }
                    Some(value)
                } else {
                    None
                };
                out.push(Arg::Option {
                    name,
                    arg: a,
                    value,
                });
            }
            None => {
                if a != "-" {
                    check_input(a, root)?;
                }
                out.push(Arg::Positional(a));
            }
        }
    }
    Ok(out)
}

/// The real program's arguments, after `check_args`. ffmpeg gets the
/// confinement options right before every input option (`-i`, `-i:x`), so
/// they come after any option the caller gave for that input. ffprobe gets
/// the caller's options, then the confinement options, then `-i <input>`
/// (its input is the last argument). A call with no input (`-version`)
/// passes unchanged.
pub fn rewrite(program: &str, args: &[OsString], root: &Path) -> Result<Vec<OsString>, String> {
    let parsed = check_args(args, root)?;
    let positional = parsed
        .iter()
        .filter(|a| matches!(a, Arg::Positional(_)))
        .count();
    let mut out: Vec<OsString> = Vec::with_capacity(args.len() + 8);
    if program == "ffprobe" {
        if parsed.iter().any(Arg::is_input_option) {
            return Err("ffprobe takes its input as the last argument, not -i".to_owned());
        }
        if positional == 0 {
            return Ok(args.to_vec()); // -version
        }
        let Some((Arg::Positional(input), rest)) = parsed.split_last() else {
            return Err("ffprobe takes exactly one input, as the last argument".to_owned());
        };
        if positional > 1 {
            return Err("ffprobe takes exactly one input, as the last argument".to_owned());
        }
        for a in rest {
            a.push_to(&mut out);
        }
        out.extend(CONFINE.iter().map(OsString::from));
        out.push("-i".into());
        out.push((*input).clone());
    } else {
        for a in &parsed {
            if a.is_input_option() {
                out.extend(CONFINE.iter().map(OsString::from));
            }
            a.push_to(&mut out);
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
        // Caller options first, then the shim's (the last value wins).
        assert_eq!(
            out,
            os(&[
                "-show_streams",
                "-format_whitelist",
                FORMATS,
                "-protocol_whitelist",
                "file",
                "-i",
                &a
            ])
        );
        // `-v error`: "error" is a value, not the input.
        let out = rewrite("ffprobe", &os(&["-v", "error", "-show_streams", &a]), &r).unwrap();
        assert_eq!(out[..3], os(&["-v", "error", "-show_streams"])[..]);
        assert_eq!(out.last().unwrap(), a.as_str());
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

    /// Only options yt-dlp's postprocessing uses pass; `-attach` and
    /// outputs must be in the job directory.
    #[test]
    fn allows_only_known_options() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path().canonicalize().unwrap();
        let a = format!("file:{}/a.m4a", r.display());
        let out = format!("file:{}/b.m4a", r.display());
        let thumb = format!("file:{}/a.jpg", r.display());
        // What EmbedThumbnail, FFmpegMetadata and ThumbnailsConvertor run.
        let ok = os(&[
            "-y",
            "-loglevel",
            "repeat+info",
            "-i",
            &a,
            "-i",
            &thumb,
            "-map",
            "0",
            "-map",
            "1",
            "-dn",
            "-ignore_unknown",
            "-c",
            "copy",
            "-disposition:v:0",
            "attached_pic",
            "-metadata:s:v",
            "comment=Cover (front)",
            "-map",
            "-0:s",
            "-c:v",
            "mjpeg",
            "-qmin",
            "1",
            "-q:v",
            "2",
            "-vf",
            "crop=1:1",
            "-movflags",
            "+faststart",
            "-attach",
            &thumb,
            &out,
        ]);
        assert!(
            rewrite("ffmpeg", &ok, &r).is_ok(),
            "{:?}",
            rewrite("ffmpeg", &ok, &r)
        );
        for bad in [
            &["-i", &a, "-attach", "/etc/passwd", &out][..],
            &["-i", &a, "-attach", "file:/etc/passwd", &out],
            &["-i", &a, "-filter_script", "/etc/passwd", &out],
            &["-i", &a, "-/af", "/etc/passwd", &out],
            &["-i", &a, "-passlogfile", "/tmp/x", &out],
            &["-i", &a, "-progress", "/tmp/x", &out],
            &["-format_whitelist", "hls", "-i", &a, &out],
            &["-i", &a, "/tmp/out.m4a"],
        ] {
            assert!(rewrite("ffmpeg", &os(bad), &r).is_err(), "{bad:?}");
        }
        assert!(rewrite("ffprobe", &os(&["-format_whitelist", "hls", &a]), &r).is_err());
        assert!(rewrite("ffprobe", &os(&["-show_streams", "-i", &a]), &r).is_err());
    }

    /// Found by `fuzz/fuzz_targets/ffshim_args.rs`: ffmpeg reads `-i:x` as
    /// `-i`, and an option's value is never an option. `-i:x` got no
    /// confinement (ffprobe passed it through unchanged), and a value
    /// `-i` made the shim insert the confinement options where they shift
    /// every argument after them.
    #[test]
    fn inputs_are_found_as_ffmpeg_reads_them() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path().canonicalize().unwrap();
        let a = format!("file:{}/a.m3u8", r.display());
        let out = format!("file:{}/b.m4a", r.display());
        assert!(rewrite("ffprobe", &os(&["-i:0", &a]), &r).is_err());
        assert!(rewrite("ffprobe", &os(&["-show_streams", "-i:v", &a]), &r).is_err());
        assert_eq!(
            rewrite("ffmpeg", &os(&["-i:0", &a, &out]), &r).unwrap(),
            os(&[
                "-format_whitelist",
                FORMATS,
                "-protocol_whitelist",
                "file",
                "-i:0",
                &a,
                &out
            ])
        );
        let value = os(&["-metadata", "-i", &out]);
        assert_eq!(rewrite("ffmpeg", &value, &r).unwrap(), value);
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
