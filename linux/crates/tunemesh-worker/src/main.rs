//! TuneMesh download worker (contract: "Worker behaviour").
//!
//! Polls `<root>/.tunemesh/requests/`, processes requests oldest first, one
//! at a time, and reports progress in `<root>/.tunemesh/status/`. See
//! `--help` for the options.

use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};
use tunemesh_core::merge::{self, Bases};
use tunemesh_core::request::{self, Request, RequestState, Status};
use tunemesh_worker::config::{self, Args, Parsed, USAGE, VERSION};
use tunemesh_worker::job::{self, Ctx};
use tunemesh_worker::{ffshim, http, leader, proxy, rundir, selftest, signals, tools};

macro_rules! log {
    ($($arg:tt)*) => { eprintln!($($arg)*) };
}

/// How often statuses are checked for requests cancelled while no worker
/// was watching.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

enum Fatal {
    /// The command line or configuration is wrong: restarting will not help.
    Config(anyhow::Error),
    Other(anyhow::Error),
}

fn main() -> ExitCode {
    // Before anything can replace the binary (package upgrades).
    let _ = ffshim::exe();
    if let Some(program) = ffshim::invoked_as() {
        return ffshim::main(program);
    }
    tunemesh_core::fsx::raise_fd_limit();
    let args = match config::parse_args(std::env::args().skip(1)) {
        Ok(Parsed::Run(a)) => a,
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            println!("tunemesh-worker {VERSION}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("tunemesh-worker: {e:#}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.self_test {
        return selftest::main(args.work_dir);
    }
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Fatal::Config(e)) => {
            eprintln!("tunemesh-worker: {e:#}");
            ExitCode::from(2)
        }
        Err(Fatal::Other(e)) => {
            eprintln!("tunemesh-worker: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn setup(args: &Args) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let library = match &args.library {
        Some(l) => l.clone(),
        None => config::default_library()?,
    };
    if !library.is_dir() {
        bail!("library {} is not a directory", library.display());
    }
    let library = library.canonicalize()?;
    let work_dir = config::private_dir(
        &args
            .work_dir
            .clone()
            .unwrap_or_else(config::default_work_dir),
    )?;
    if work_dir.starts_with(&library) || library.starts_with(&work_dir) {
        bail!("the work directory and the library must not contain each other");
    }
    Ok((library, work_dir))
}

fn run(args: Args) -> Result<(), Fatal> {
    signals::install();
    let (library, work_dir) = setup(&args).map_err(Fatal::Config)?;
    // Worker-only devices have no app to write the ignore rule for the
    // temporary files moved into the library.
    match tunemesh_core::stignore::ensure(&library) {
        Ok(true) => log!("added the TuneMesh rule to {}/.stignore", library.display()),
        Ok(false) => {}
        Err(e) => log!("could not update .stignore: {e}"),
    }
    let run = rundir::RunDir::claim(&work_dir)
        .with_context(|| format!("setting up {}", work_dir.display()))
        .map_err(Fatal::Other)?;
    let proxy = proxy::Proxy::start()
        .context("starting the egress proxy")
        .map_err(Fatal::Other)?;
    let http = http::Http::new(&proxy.url()).map_err(|e| Fatal::Other(anyhow::anyhow!(e)))?;
    let device = args.device.clone().unwrap_or_else(config::host_name);
    log!(
        "tunemesh-worker {VERSION}: library {}, work dir {}, device {device:?}",
        library.display(),
        work_dir.display()
    );
    let leader = leader::Leader::new(&library, device, config::machine_id());
    let bases = Bases::new(&library);
    let mut ctx = Ctx {
        root: library,
        work_dir,
        run,
        limits: args.limits.clone(),
        proxy,
        http,
        tools: tools::Tools::new(args.yt_dlp.clone()),
        source_ids: job::SourceIds::default(),
        bases,
        unwritten: None,
    };

    let mut reported_bad: HashSet<String> = HashSet::new();
    let mut reported_pending = 0;
    let mut last_sweep: Option<Instant> = None;
    while !signals::stop_requested() {
        if !leader.may_work() {
            signals::sleep(args.poll);
            continue;
        }
        let root = ctx.root.clone();
        let (requests, bad, pending) = request::list_oldest_requests(&root, ctx.limits.max_pending);
        if pending > ctx.limits.max_pending && pending != reported_pending {
            log!(
                "{pending} requests waiting; looking at the oldest {} only",
                ctx.limits.max_pending
            );
        }
        reported_pending = pending;

        for (id, err) in bad {
            if reported_bad.insert(id.clone()) {
                log!("[{id}] invalid request file: {err}");
            }
            fail_invalid(&root, &id, &err.to_string());
        }

        if last_sweep.is_none_or(|t| t.elapsed() >= SWEEP_INTERVAL) {
            last_sweep = Some(Instant::now());
            sweep(&root, &ctx.bases);
        }

        // A final status that could not be written keeps its request: no
        // other request is taken until it is written.
        if !job::retry_unwritten(&mut ctx) {
            signals::sleep(args.poll);
            continue;
        }

        // Every pending request gets a status so clients show "queued".
        for r in &requests {
            if request::read_state(&root, &r.id).is_none()
                && let Err(e) = Status::new(r, RequestState::Queued).write(&root)
            {
                log!("[{}] writing status: {e}", r.id);
            }
        }

        match requests.into_iter().next() {
            Some(r) => job::process(&mut ctx, r),
            None => signals::sleep(args.poll),
        }
    }
    log!("tunemesh-worker: stopping");
    leader.release();
    Ok(())
}

/// Statuses whose request file is gone: unfinished ones were cancelled
/// while no worker was watching, final ones past the newest listed are
/// deleted (`request::sweep`, one status file at a time). Then playlist
/// conflict copies that are due are merged (the worker does not know its
/// Syncthing device ID, so it merges copies 10 minutes old).
fn sweep(root: &Path, bases: &Bases) {
    let done = request::sweep(root);
    for id in &done.cancelled {
        log!("[{id}] request file gone; marked cancelled");
    }
    if done.pruned > 0 {
        log!(
            "deleted {} finished status files no client lists",
            done.pruned
        );
    }
    for (id, e) in &done.errors {
        log!("[{id}] status file: {e}");
    }
    for (name, result) in merge::merge_conflicts(root, bases, None, SystemTime::now()) {
        match result {
            Ok(()) => log!("merged conflict copies into playlist {name:?}"),
            Err(e) => log!("merging conflict copies into playlist {name:?}: {e}"),
        }
    }
}

/// A request file that cannot be read or parsed (or is not a regular file)
/// can never be processed: report it and remove it so it does not block the
/// queue. Removing a symlink removes the link, never its target.
fn fail_invalid(root: &Path, id: &str, err: &str) {
    let mut req = Request::new("", None, false, "");
    req.id = id.into();
    let mut status = Status::new(&req, RequestState::Failed);
    status.message = Some(format!("Invalid request file: {err}").into());
    if status.write(root).is_ok() {
        let _ = request::cancel(root, id);
    }
}
