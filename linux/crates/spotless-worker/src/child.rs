//! Runs a downloader process, streaming its output lines, and kills its
//! whole process group when the request is cancelled, the worker stops, the
//! run exceeds its wall-clock limit or a check (such as the size of the
//! scratch directory) fails.

use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub enum Stream {
    Out,
    Err,
}

/// Why a run was stopped before it ended by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The request file was deleted.
    Cancelled,
    /// The worker is shutting down or handing over to another device.
    Interrupted,
    /// A limit was hit; the text says which.
    Limit(String),
}

pub enum End {
    Exited(ExitStatus),
    Stopped(Stop),
}

pub struct Outcome {
    pub end: End,
    /// The last stderr lines, oldest first.
    pub stderr_tail: Vec<String>,
}

impl Outcome {
    /// The most useful error line: the last `ERROR:` line, else the last line.
    pub fn error_message(&self) -> String {
        if let End::Stopped(Stop::Limit(why)) = &self.end {
            return why.clone();
        }
        let line = self
            .stderr_tail
            .iter()
            .rev()
            .find(|l| l.contains("ERROR"))
            .or_else(|| self.stderr_tail.iter().rev().find(|l| !l.trim().is_empty()));
        match line {
            Some(l) => {
                let l = l.trim();
                let l = l.strip_prefix("ERROR:").unwrap_or(l).trim();
                // yt-dlp appends Python exception reprs; they add nothing for a person.
                let l = l.split(" (caused by ").next().unwrap_or(l);
                l.to_owned()
            }
            None => match &self.end {
                End::Exited(s) => format!("downloader exited with {s}"),
                End::Stopped(_) => "stopped".to_owned(),
            },
        }
    }
}

fn pump(
    stream: impl Read + Send + 'static,
    kind: fn(String) -> (Stream, String),
    tx: mpsc::Sender<(Stream, String)>,
) {
    thread::spawn(move || {
        // Lines are capped so a child cannot make the worker buffer without bound.
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = Vec::new();
            match reader.by_ref().take(64 * 1024).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&line)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            if tx.send(kind(line)).is_err() {
                break;
            }
        }
    });
}

fn signal_group(pid: u32, signal: i32) {
    // SAFETY: kill(2) on a process group we created; no memory is touched.
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

/// Runs `cmd` for at most `timeout`, calling `on_line` for every output
/// line and `check` about twice a second; when `check` returns a reason, or
/// time runs out, the process group gets SIGTERM, then SIGKILL 5 s later.
pub fn run(
    cmd: &mut Command,
    timeout: Duration,
    mut on_line: impl FnMut(Stream, &str),
    mut check: impl FnMut() -> Option<Stop>,
) -> io::Result<Outcome> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let started = Instant::now();
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    pump(
        child.stdout.take().expect("piped stdout"),
        |l| (Stream::Out, l),
        tx.clone(),
    );
    pump(
        child.stderr.take().expect("piped stderr"),
        |l| (Stream::Err, l),
        tx,
    );

    let mut tail: Vec<String> = Vec::new();
    let mut last_check = Instant::now();
    let mut stopped: Option<Stop> = None;
    let mut kill_at: Option<Instant> = None;
    let mut output_open = true;
    let mut exited = None;
    // Runs until the process has exited and its output is closed; the time
    // limit, `check` and the SIGTERM-to-SIGKILL escalation apply the whole
    // time, also when the process closes its output early.
    while output_open || exited.is_none() {
        if output_open {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok((stream, line)) => {
                    if let Stream::Err = stream {
                        if tail.len() == 20 {
                            tail.remove(0);
                        }
                        tail.push(line.clone());
                    }
                    on_line(stream, &line);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => output_open = false,
            }
        } else {
            thread::sleep(Duration::from_millis(50));
        }
        if exited.is_none() {
            exited = child.try_wait()?;
        }
        if stopped.is_none() && last_check.elapsed() >= Duration::from_millis(500) {
            last_check = Instant::now();
            let reason = check().or_else(|| {
                (started.elapsed() >= timeout).then(|| {
                    Stop::Limit(format!(
                        "Took longer than {}",
                        humantime::format_duration(timeout)
                    ))
                })
            });
            if let Some(reason) = reason {
                stopped = Some(reason);
                signal_group(pid, libc::SIGTERM);
                kill_at = Some(Instant::now() + Duration::from_secs(5));
            }
        }
        if let Some(at) = kill_at
            && Instant::now() >= at
        {
            signal_group(pid, libc::SIGKILL);
            kill_at = None;
        }
    }
    let status = child.wait()?;
    // Grandchildren (ffmpeg) that outlived yt-dlp go too.
    if stopped.is_some() {
        signal_group(pid, libc::SIGKILL);
    }
    Ok(Outcome {
        end: match stopped {
            Some(s) => End::Stopped(s),
            None => End::Exited(status),
        },
        stderr_tail: tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_out_and_kills_the_group() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30 & sleep 30; echo never"]);
        let start = Instant::now();
        let mut lines = Vec::new();
        let out = run(
            &mut cmd,
            Duration::from_secs(1),
            |_, l| lines.push(l.to_owned()),
            || None,
        )
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(matches!(out.end, End::Stopped(Stop::Limit(_))));
        assert!(out.error_message().starts_with("Took longer than"));
        assert!(lines.is_empty());
    }

    #[test]
    fn check_stops_the_run() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo hi; exec sleep 30"]);
        let mut n = 0;
        let out = run(
            &mut cmd,
            Duration::from_secs(60),
            |_, _| {},
            || {
                n += 1;
                (n > 1).then_some(Stop::Cancelled)
            },
        )
        .unwrap();
        assert!(matches!(out.end, End::Stopped(Stop::Cancelled)));
    }

    /// A child that closes stdout and stderr early still gets its time limit.
    #[test]
    fn times_out_after_output_is_closed() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec 1>&- 2>&-; sleep 30"]);
        let start = Instant::now();
        let out = run(&mut cmd, Duration::from_secs(1), |_, _| {}, || None).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        assert!(matches!(out.end, End::Stopped(Stop::Limit(_))));
    }

    /// ... and can still be cancelled.
    #[test]
    fn cancels_after_output_is_closed() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exec 1>&- 2>&-; sleep 30"]);
        let start = Instant::now();
        let out = run(
            &mut cmd,
            Duration::from_secs(60),
            |_, _| {},
            || (start.elapsed() > Duration::from_millis(800)).then_some(Stop::Cancelled),
        )
        .unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
        assert!(matches!(out.end, End::Stopped(Stop::Cancelled)));
    }

    /// SIGTERM is ignored: SIGKILL follows 5 s later.
    #[test]
    fn escalates_to_sigkill() {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "trap '' TERM; exec 1>&- 2>&-; while :; do sleep 1; done",
        ]);
        let start = Instant::now();
        let out = run(&mut cmd, Duration::from_secs(1), |_, _| {}, || None).unwrap();
        let took = start.elapsed();
        assert!(
            took >= Duration::from_secs(5) && took < Duration::from_secs(10),
            "{took:?}"
        );
        assert!(matches!(out.end, End::Stopped(Stop::Limit(_))));
    }

    #[test]
    fn reports_exit_and_error_lines() {
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            "echo 'ERROR: [youtube] x: Video unavailable (caused by Foo)' >&2; exit 1",
        ]);
        let out = run(&mut cmd, Duration::from_secs(60), |_, _| {}, || None).unwrap();
        assert!(matches!(out.end, End::Exited(s) if !s.success()));
        assert_eq!(out.error_message(), "[youtube] x: Video unavailable");
    }
}
