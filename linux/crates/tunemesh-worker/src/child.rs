//! Runs a downloader process, streaming its output lines and killing its
//! whole process group when the request is cancelled.

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

pub enum End {
    Exited(ExitStatus),
    Cancelled,
}

pub struct Outcome {
    pub end: End,
    /// The last stderr lines, oldest first.
    pub stderr_tail: Vec<String>,
}

impl Outcome {
    /// The most useful error line: the last `ERROR:` line, else the last line.
    pub fn error_message(&self) -> String {
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
                End::Cancelled => "cancelled".to_owned(),
            },
        }
    }
}

fn pump(stream: impl Read + Send + 'static, kind: fn(String) -> (Stream, String), tx: mpsc::Sender<(Stream, String)>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line).trim_end_matches('\r').to_owned();
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

/// Runs `cmd`, calling `on_line` for every output line, and polls
/// `cancelled` about once a second.
pub fn run(
    cmd: &mut Command,
    mut on_line: impl FnMut(Stream, &str),
    cancelled: impl Fn() -> bool,
) -> io::Result<Outcome> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    pump(child.stdout.take().expect("piped stdout"), |l| (Stream::Out, l), tx.clone());
    pump(child.stderr.take().expect("piped stderr"), |l| (Stream::Err, l), tx);

    let mut tail: Vec<String> = Vec::new();
    let mut last_check = Instant::now();
    let mut was_cancelled = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
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
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if !was_cancelled && last_check.elapsed() >= Duration::from_secs(1) {
            last_check = Instant::now();
            if cancelled() {
                was_cancelled = true;
                signal_group(pid, libc::SIGTERM);
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut exited = false;
                while !exited && Instant::now() < deadline {
                    exited = child.try_wait()?.is_some();
                    if !exited {
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                if !exited {
                    signal_group(pid, libc::SIGKILL);
                }
            }
        }
    }
    let status = child.wait()?;
    Ok(Outcome { end: if was_cancelled { End::Cancelled } else { End::Exited(status) }, stderr_tail: tail })
}
