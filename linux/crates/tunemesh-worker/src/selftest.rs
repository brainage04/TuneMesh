//! `tunemesh-worker --self-test`: checks, inside whatever package the worker
//! runs from, that ffmpeg and ffprobe work through the confining shim
//! (`ffshim`) exactly as yt-dlp's postprocessing would use them, and that
//! the shim refuses a disguised HLS playlist. Needs no library or network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use crate::rundir::RunDir;
use crate::{config, ffshim, tools::Tools};

struct Checks {
    failed: bool,
}

impl Checks {
    fn report(&mut self, name: &str, result: Result<String, String>) -> bool {
        match result {
            Ok(detail) if detail.is_empty() => println!("ok   {name}"),
            Ok(detail) => println!("ok   {name}: {detail}"),
            Err(e) => {
                println!("FAIL {name}: {e}");
                self.failed = true;
            }
        }
        !self.failed
    }
}

/// Runs `program args` with the shim environment; stdout on success.
fn run(
    program: &Path,
    args: &[&str],
    env: &[(&'static str, std::ffi::OsString)],
) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .envs(env.iter().cloned())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned();
        Err(format!("{}: {last}", out.status))
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_owned()
}

pub fn main(work_dir: Option<PathBuf>) -> ExitCode {
    let mut c = Checks { failed: false };
    let work = config::private_dir(&work_dir.unwrap_or_else(config::default_work_dir))
        .map_err(|e| format!("{e:#}"));
    if !c.report(
        "work directory",
        work.as_ref()
            .map(|w| w.display().to_string())
            .map_err(Clone::clone),
    ) {
        return ExitCode::from(1);
    }
    let work = work.unwrap_or_default();
    let tools = Tools::new(None).ffmpeg();
    if !c.report(
        "ffmpeg found",
        tools
            .as_ref()
            .map(|(a, b)| format!("{}, {}", a.display(), b.display()))
            .map_err(Clone::clone),
    ) {
        return ExitCode::from(1);
    }
    let (ffmpeg, ffprobe) = tools.unwrap_or_default();
    let run_dir = match RunDir::claim(&work) {
        Ok(r) => r,
        Err(e) => {
            c.report("shim installed", Err(e.to_string()));
            return ExitCode::from(1);
        }
    };
    let job = run_dir.jobs().join("self-test");
    let setup = fs::create_dir_all(&job)
        .and_then(|()| job.canonicalize())
        .and_then(|job| ffshim::install(run_dir.path()).map(|shim| (job, shim)))
        .map_err(|e| e.to_string());
    if !c.report(
        "shim installed",
        setup
            .as_ref()
            .map(|(_, s)| s.display().to_string())
            .map_err(Clone::clone),
    ) {
        return ExitCode::from(1);
    }
    let (job, shim) = setup.unwrap_or_default();
    let env = ffshim::env(&ffmpeg, &ffprobe, &job);
    let (shim_ffmpeg, shim_ffprobe) = (shim.join("ffmpeg"), shim.join("ffprobe"));

    c.report(
        "shim ffmpeg -version",
        run(&shim_ffmpeg, &["-version"], &env).map(|o| first_line(&o)),
    );
    c.report(
        "shim ffprobe -version",
        run(&shim_ffprobe, &["-version"], &env).map(|o| first_line(&o)),
    );

    // A test tone made by the real ffmpeg, then remuxed and probed through
    // the shim as yt-dlp's ExtractAudio does (built-in codecs only).
    let tone = job.join("tone [selftest].flac");
    let remuxed = job.join("tone [selftest].mka");
    let (tone_in, remux_out) = (
        format!("file:{}", tone.display()),
        format!("file:{}", remuxed.display()),
    );
    let made = run(
        &ffmpeg,
        &[
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1",
            "-c:a",
            "flac",
            &tone_in,
        ],
        &[],
    );
    if c.report("generate test tone", made.map(|_| String::new())) {
        c.report(
            "confined remux",
            run(
                &shim_ffmpeg,
                &[
                    "-v", "error", "-y", "-i", &tone_in, "-vn", "-c:a", "copy", &remux_out,
                ],
                &env,
            )
            .map(|_| String::new()),
        );
        c.report(
            "confined probe",
            run(
                &shim_ffprobe,
                &[
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_name",
                    "-of",
                    "csv=p=0",
                    &remux_out,
                ],
                &env,
            )
            .and_then(|o| match o.trim() {
                "flac" => Ok("flac".to_owned()),
                other => Err(format!("unexpected codec {other:?}")),
            }),
        );
        // An HLS playlist saved as audio must not be opened.
        let disguised = job.join("x [selftest].m3u");
        let written = fs::write(
            &disguised,
            format!(
                "#EXTM3U\n#EXT-X-TARGETDURATION:1\n#EXTINF:1.0,\nfile://{}\n#EXT-X-ENDLIST\n",
                tone.display()
            ),
        );
        let refused = match written {
            Err(e) => Err(e.to_string()),
            Ok(()) => match run(
                &shim_ffprobe,
                &[
                    "-v",
                    "error",
                    "-show_streams",
                    &format!("file:{}", disguised.display()),
                ],
                &env,
            ) {
                Ok(_) => Err("ffprobe opened a disguised HLS playlist".to_owned()),
                Err(e) => Ok(e),
            },
        };
        c.report("disguised playlist refused", refused);
        c.report(
            "outside path refused",
            match run(
                &shim_ffprobe,
                &["-v", "error", "-show_streams", "file:/etc/hostname"],
                &env,
            ) {
                Ok(_) => Err("ffprobe opened a file outside the work directory".to_owned()),
                Err(e) => Ok(e),
            },
        );
    }
    drop(run_dir);
    if c.failed {
        println!("self-test failed");
        ExitCode::from(1)
    } else {
        println!("self-test passed");
        ExitCode::SUCCESS
    }
}
