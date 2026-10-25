//! The ffmpeg/ffprobe shim's argument filter. Whatever arguments it is
//! given, a command it lets through, read the way ffmpeg and ffprobe read
//! their command line, opens every input with the confinement options and
//! only names `file:` paths inside the job directory.
#![no_main]

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use tunemesh_worker::ffshim::{FORMATS, rewrite};

/// Options with whether they take a value: the shim's list
/// (`ffshim::OPTIONS`), the confinement options, and a few it refuses.
const OPTIONS: &[(&str, bool)] = &[
    ("y", false),
    ("n", false),
    ("hide_banner", false),
    ("nostdin", false),
    ("version", false),
    ("bsfs", false),
    ("loglevel", true),
    ("v", true),
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
    ("metadata", true),
    ("disposition", true),
    ("ignore_unknown", false),
    ("id3v2_version", true),
    ("write_id3v1", true),
    ("movflags", true),
    ("update", true),
    ("attach", true),
    ("show_format", false),
    ("show_streams", false),
    ("show_entries", true),
    ("select_streams", true),
    ("print_format", true),
    ("of", true),
    ("format_whitelist", true),
    ("protocol_whitelist", true),
    ("filter_script", true),
    ("progress", true),
];

/// ffmpeg's `find_option`: the name, or the name followed by `:<spec>`.
fn find(arg: &[u8]) -> Option<(&'static str, bool)> {
    let name = arg.strip_prefix(b"-").filter(|n| !n.is_empty())?;
    let found = OPTIONS.iter().find(|(o, _)| {
        name.strip_prefix(o.as_bytes())
            .is_some_and(|rest| rest.is_empty() || rest[0] == b':')
    });
    Some(*found.unwrap_or(&("?", false)))
}

/// The job directory: a file, a subdirectory and a symlink out of it.
static ROOT: LazyLock<PathBuf> = LazyLock::new(|| {
    let dir = std::env::temp_dir().join(format!("tunemesh-fuzz-ffshim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("d")).unwrap();
    std::fs::write(dir.join("a.m4a"), b"").unwrap();
    std::os::unix::fs::symlink("/etc", dir.join("l")).unwrap();
    dir.canonicalize().unwrap()
});

fn root() -> &'static Path {
    &ROOT
}

/// Whether `arg` is `file:` plus a path inside the root, by its text
/// (`..` resolved, the symlink `l` counting as outside).
fn file_inside(arg: &[u8]) -> bool {
    let Some(p) = arg.strip_prefix(b"file:") else {
        return false;
    };
    let p = Path::new(std::ffi::OsStr::from_bytes(p));
    if !p.is_absolute() {
        return false;
    }
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(n) => parts.push(n),
            Component::ParentDir => {
                parts.pop();
            }
            _ => {}
        }
    }
    let resolved: PathBuf = std::iter::once(std::ffi::OsStr::new("/"))
        .chain(parts)
        .collect();
    let Ok(rest) = resolved.strip_prefix(root()) else {
        return false;
    };
    rest.components().next() != Some(Component::Normal("l".as_ref()))
}

#[derive(Arbitrary, Debug)]
enum Arg {
    /// An option from `OPTIONS`, optionally with a `:<spec>` suffix.
    Option(u8, Option<String>),
    /// A path below the root, as `file:<root>/<rest>`.
    Inside(String),
    Raw(Vec<u8>),
}

#[derive(Arbitrary, Debug)]
struct Input {
    ffprobe: bool,
    args: Vec<Arg>,
}

fuzz_target!(|i: Input| {
    let root = root();
    let args: Vec<OsString> = i
        .args
        .iter()
        .map(|a| match a {
            Arg::Option(n, spec) => {
                let (name, _) = OPTIONS[usize::from(*n) % OPTIONS.len()];
                match spec {
                    Some(s) => format!("-{name}:{s}").into(),
                    None => format!("-{name}").into(),
                }
            }
            Arg::Inside(rest) => format!("file:{}/{rest}", root.display()).into(),
            Arg::Raw(b) => OsString::from_vec(b.clone()),
        })
        .collect();
    let program = if i.ffprobe { "ffprobe" } else { "ffmpeg" };
    let Ok(out) = rewrite(program, &args, root) else {
        return;
    };

    // Read the command as the real program does.
    let (mut format, mut protocol) = (None::<&[u8]>, None::<&[u8]>);
    let mut inputs = 0;
    let mut it = out.iter().map(|a| a.as_bytes());
    while let Some(a) = it.next() {
        let Some((name, takes_value)) = (if a == b"-" { None } else { find(a) }) else {
            // A positional argument: an output (ffmpeg) or the input (ffprobe).
            assert!(
                a == b"-" || file_inside(a),
                "{program} gets {:?}",
                String::from_utf8_lossy(a)
            );
            if i.ffprobe {
                inputs += 1;
            } else {
                (format, protocol) = (None, None);
            }
            continue;
        };
        assert!(
            name != "?",
            "{program} gets unknown option {:?}",
            String::from_utf8_lossy(a)
        );
        if !takes_value {
            continue;
        }
        let value = it
            .next()
            .unwrap_or_else(|| panic!("{program}: -{name} without a value"));
        match name {
            "format_whitelist" => format = Some(value),
            "protocol_whitelist" => protocol = Some(value),
            "i" => {
                // `-` (ffprobe's positional stdin) is a pipe the confinement refuses.
                assert!(
                    value == b"-" || file_inside(value),
                    "{program} opens {:?}",
                    String::from_utf8_lossy(value)
                );
                if i.ffprobe {
                    inputs += 1;
                } else {
                    // ffmpeg: per-input options apply to this input only.
                    assert_eq!(format, Some(FORMATS.as_bytes()), "{out:?}");
                    assert_eq!(protocol, Some(&b"file"[..]), "{out:?}");
                    (format, protocol) = (None, None);
                }
            }
            "attach" => assert!(file_inside(value)),
            _ => {}
        }
    }
    if i.ffprobe {
        // ffprobe: one input; its options are global (the last one wins).
        assert!(inputs <= 1, "{out:?}");
        if inputs == 1 {
            assert_eq!(format, Some(FORMATS.as_bytes()), "{out:?}");
            assert_eq!(protocol, Some(&b"file"[..]), "{out:?}");
        }
    }
});
