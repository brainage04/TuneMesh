//! A downloaded "audio" file that is really an HLS playlist pointing at a
//! file outside the work directory must not let yt-dlp's ffprobe/ffmpeg
//! postprocessing copy that file (the worker's ffmpeg shim, `src/ffshim.rs`).
//!
//! A site serving `a.m3u` as `audio/mpeg` makes yt-dlp's generic extractor
//! save it as `<title> [<id>].m3u`. ffmpeg 7.1 and later only detect HLS
//! by extension or MIME type; older versions (Ubuntu 24.04 has 6.1) detect
//! it in a file named `.mp3` too. The test uses `.m3u` so the unconfined
//! control works on every version.
//!
//! Needs ffmpeg and ffprobe on `PATH`; skipped (with a message) otherwise.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn run(program: &Path, args: &[&str], env: &[(&str, &Path)]) -> Output {
    let mut c = Command::new(program);
    c.args(args).stdin(std::process::Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// The shim directory the worker creates: `ffmpeg` and `ffprobe` symlinks
/// to the worker executable.
fn shim_dir(base: &Path) -> PathBuf {
    let dir = base.join("ffmpeg-shim");
    fs::create_dir_all(&dir).unwrap();
    for name in ["ffmpeg", "ffprobe"] {
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_tunemesh-worker"), dir.join(name)).unwrap();
    }
    dir
}

#[test]
fn disguised_hls_cannot_read_outside_files() {
    let (Some(ffmpeg), Some(ffprobe)) = (which("ffmpeg"), which("ffprobe")) else {
        eprintln!("skipped: ffmpeg/ffprobe not on PATH");
        return;
    };
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret.mp3");
    let made = run(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-c:a",
            "libmp3lame",
            secret.to_str().unwrap(),
        ],
        &[],
    );
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    let work = tempfile::tempdir().unwrap();
    let job = work.path().join("jobs/1");
    fs::create_dir_all(&job).unwrap();
    let job = job.canonicalize().unwrap();
    let disguised = job.join("x [abc].m3u");
    fs::write(
        &disguised,
        format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:3\n#EXTINF:3.0,\nfile://{}\n#EXT-X-ENDLIST\n",
            secret.display()
        ),
    )
    .unwrap();
    let input = format!("file:{}", disguised.display());
    let out = job.join("x [abc].opus");
    let output = format!("file:{}", out.display());
    // What yt-dlp's ExtractAudio and metadata postprocessors run.
    let probe_args = [
        "-hide_banner",
        "-show_format",
        "-show_streams",
        "-print_format",
        "json",
        input.as_str(),
    ];
    let convert_args = [
        "-y",
        "-loglevel",
        "error",
        "-i",
        input.as_str(),
        "-vn",
        "-c:a",
        "libopus",
        output.as_str(),
    ];

    // Unconfined, ffmpeg follows the playlist and copies the outside file:
    // the attack this test guards against.
    let probe = run(&ffprobe, &probe_args, &[]);
    assert!(String::from_utf8_lossy(&probe.stdout).contains("\"codec_type\": \"audio\""));
    let convert = run(&ffmpeg, &convert_args, &[]);
    assert!(
        convert.status.success() && out.exists(),
        "control: {}",
        String::from_utf8_lossy(&convert.stderr)
    );
    fs::remove_file(&out).unwrap();

    // Through the worker's shim, both refuse.
    let shim = shim_dir(work.path());
    let env = [
        ("TUNEMESH_REAL_FFMPEG", ffmpeg.as_path()),
        ("TUNEMESH_REAL_FFPROBE", ffprobe.as_path()),
        ("TUNEMESH_SHIM_ROOT", job.as_path()),
    ];
    let probe = run(&shim.join("ffprobe"), &probe_args, &env);
    assert!(
        !probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stdout)
    );
    assert!(!String::from_utf8_lossy(&probe.stdout).contains("codec_type"));
    assert!(
        String::from_utf8_lossy(&probe.stderr).contains("whitelist"),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let convert = run(&shim.join("ffmpeg"), &convert_args, &env);
    assert!(!convert.status.success());
    assert!(!out.exists() || fs::metadata(&out).unwrap().len() == 0);

    // A genuine audio file in the job directory still converts.
    let real = job.join("y [def].mp3");
    fs::copy(&secret, &real).unwrap();
    let real_in = format!("file:{}", real.display());
    let ok = run(
        &shim.join("ffmpeg"),
        &[
            "-y",
            "-loglevel",
            "error",
            "-i",
            &real_in,
            "-vn",
            "-c:a",
            "libopus",
            &output,
        ],
        &env,
    );
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let probe = run(
        &shim.join("ffprobe"),
        &["-v", "error", "-show_streams", &output],
        &env,
    );
    assert!(
        probe.status.success()
            && String::from_utf8_lossy(&probe.stdout).contains("codec_name=opus")
    );

    // And the shim refuses inputs outside the job directory outright.
    let direct = format!("file:{}", secret.display());
    let refused = run(&shim.join("ffprobe"), &["-show_streams", &direct], &env);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("outside the work directory"));
}

/// `--self-test` passes where ffmpeg is installed and needs no library.
#[test]
fn self_test_passes() {
    if which("ffmpeg").is_none() || which("ffprobe").is_none() {
        eprintln!("skipped: ffmpeg/ffprobe not on PATH");
        return;
    }
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("w");
    let out = Command::new(env!("CARGO_BIN_EXE_tunemesh-worker"))
        .args(["--self-test", "--work-dir", dir.to_str().unwrap()])
        .env_remove("TUNEMESH_FFMPEG_DIR")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{text}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains("FAIL") && text.contains("disguised playlist refused"),
        "{text}"
    );
    assert_eq!(fs::read_dir(dir.join("run")).unwrap().count(), 0);
}
