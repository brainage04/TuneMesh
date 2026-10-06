//! TuneMesh download worker (contract: "Worker behaviour").
//!
//! `tunemesh-worker --library <root> --work-dir <dir>` polls
//! `<root>/.tunemesh/requests/`, processes requests oldest first, one at a
//! time, and reports progress in `<root>/.tunemesh/status/`.

mod child;
mod job;
mod spotdl;
mod ytdlp;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;
use std::{fs, thread};

use anyhow::{Context, Result, bail};
use tunemesh_core::request::{self, RequestState, Status};

const USAGE: &str = "usage: tunemesh-worker --library <root> --work-dir <dir> [--poll-seconds <n>]";

macro_rules! log {
    ($($arg:tt)*) => { eprintln!($($arg)*) };
}
pub(crate) use log;

struct Args {
    library: PathBuf,
    work_dir: PathBuf,
    poll: Duration,
}

fn parse_args() -> Result<Args> {
    let mut library = None;
    let mut work_dir = None;
    let mut poll = Duration::from_secs(2);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--library" => library = args.next().map(PathBuf::from),
            "--work-dir" => work_dir = args.next().map(PathBuf::from),
            "--poll-seconds" => {
                let n: f64 = args.next().context(USAGE)?.parse().context("--poll-seconds")?;
                poll = Duration::from_secs_f64(n.max(0.1));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}\n{USAGE}"),
        }
    }
    let (Some(library), Some(work_dir)) = (library, work_dir) else { bail!(USAGE) };
    Ok(Args { library, work_dir, poll })
}

fn main() -> Result<()> {
    let args = parse_args()?;
    if !args.library.is_dir() {
        bail!("library {} is not a directory", args.library.display());
    }
    let library = args.library.canonicalize()?;
    fs::create_dir_all(&args.work_dir).with_context(|| format!("creating {}", args.work_dir.display()))?;
    let work_dir = args.work_dir.canonicalize()?;
    if work_dir.starts_with(&library) {
        bail!("the work directory must be outside the library");
    }
    log!("tunemesh-worker: library {}, work dir {}", library.display(), work_dir.display());

    let mut reported_bad: HashSet<String> = HashSet::new();
    loop {
        let (requests, bad) = request::list_requests(&library);

        for (id, err) in bad {
            if reported_bad.insert(id.clone()) {
                log!("[{id}] invalid request file: {err}");
            }
            fail_invalid(&library, &id, &err.to_string());
        }

        // Statuses left unfinished whose request file is gone were cancelled
        // while no worker was watching.
        for status in request::list_statuses(&library) {
            if !status.state.is_final() && request::is_cancelled(&library, &status.id) {
                log!("[{}] request file gone; marking cancelled", status.id);
                let mut s = status;
                s.state = RequestState::Cancelled;
                s.progress = None;
                s.updated_at = tunemesh_core::now_rfc3339();
                if let Err(e) = s.write(&library) {
                    log!("[{}] writing status: {e}", s.id);
                }
            }
        }

        // Every pending request gets a status so clients show "queued".
        for r in &requests {
            if request::read_status(&library, &r.id).is_none()
                && let Err(e) = Status::new(r, RequestState::Queued).write(&library) {
                    log!("[{}] writing status: {e}", r.id);
                }
        }

        match requests.into_iter().next() {
            Some(r) => job::process(&library, &work_dir, r),
            None => thread::sleep(args.poll),
        }
    }
}

/// A request file that is not valid JSON can never be processed: report it
/// and remove it so it does not block the queue.
fn fail_invalid(root: &std::path::Path, id: &str, err: &str) {
    let status = Status {
        version: 1,
        id: id.to_owned(),
        input: String::new(),
        state: RequestState::Failed,
        title: None,
        progress: None,
        message: Some(format!("Invalid request file: {err}")),
        tracks: Vec::new(),
        updated_at: tunemesh_core::now_rfc3339(),
    };
    if status.write(root).is_ok() {
        let _ = request::cancel(root, id);
    }
}
