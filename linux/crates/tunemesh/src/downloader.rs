//! The built-in downloader (Settings: "This device downloads songs"): runs
//! `tunemesh-worker` as a child process, restarts it with a growing delay
//! when it fails and stops it when the app quits.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::glib;
use tunemesh_core::worker::WorkerInfo;

use crate::config;

pub const WORKER_BIN: &str = "tunemesh-worker";

/// The worker exits with 2 when it refuses its arguments or configuration;
/// restarting cannot help then.
const EXIT_USAGE: i32 = 2;
/// A run at least this long resets the restart delay.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const MAX_DELAY_SECS: u64 = 60;
/// How long a stopped worker gets before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Off,
    /// No `tunemesh-worker` next to the app or on `PATH`.
    Missing,
    Running,
    /// It exited unexpectedly; the next start is due at `at`.
    Restarting {
        at: Instant,
        reason: String,
    },
    /// It refused to run (bad arguments or configuration); not restarted.
    Failed(String),
}

type Listener = Rc<dyn Fn()>;

pub struct Downloader {
    me: Weak<Downloader>,
    root: RefCell<PathBuf>,
    enabled: Cell<bool>,
    pid: Cell<Option<i32>>,
    /// Set while a stop requested by the app is in progress.
    stopping: Cell<bool>,
    /// Start again once the current process has exited (library changed).
    restart_when_stopped: Cell<bool>,
    started: Cell<Option<Instant>>,
    failures: Cell<u32>,
    retry: RefCell<Option<glib::SourceId>>,
    state: RefCell<State>,
    last_line: Arc<Mutex<String>>,
    listeners: RefCell<Vec<Listener>>,
}

/// The worker binary: next to this executable, else on `PATH`.
pub fn find_worker() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|e| Some(e.parent()?.join(WORKER_BIN)));
    if let Some(p) = beside.filter(|p| is_executable(p)) {
        return Some(p);
    }
    glib::find_program_in_path(WORKER_BIN)
}

fn is_executable(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Creates the work directory with mode 0700 (and tightens an existing one).
fn prepare_work_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

impl Downloader {
    pub fn new(root: PathBuf, enabled: bool) -> Rc<Self> {
        let d = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            root: RefCell::new(root),
            enabled: Cell::new(false),
            pid: Cell::new(None),
            stopping: Cell::new(false),
            restart_when_stopped: Cell::new(false),
            started: Cell::new(None),
            failures: Cell::new(0),
            retry: RefCell::new(None),
            state: RefCell::new(State::Off),
            last_line: Arc::new(Mutex::new(String::new())),
            listeners: RefCell::new(Vec::new()),
        });
        d.set_enabled(enabled);
        d
    }

    pub fn connect(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn set_state(&self, state: State) {
        *self.state.borrow_mut() = state;
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for l in listeners {
            l();
        }
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.set(on);
        self.failures.set(0);
        if on {
            if self.pid.get().is_none() {
                self.start();
            }
        } else {
            self.restart_when_stopped.set(false);
            self.stop();
            if self.pid.get().is_none() {
                self.set_state(State::Off);
            }
        }
    }

    /// Points the worker at another library folder.
    pub fn set_root(&self, root: PathBuf) {
        *self.root.borrow_mut() = root;
        if self.pid.get().is_some() {
            self.restart_when_stopped.set(true);
            self.stop();
        } else if self.enabled.get() {
            self.failures.set(0);
            self.start();
        }
    }

    fn start(&self) {
        if let Some(id) = self.retry.borrow_mut().take() {
            id.remove();
        }
        if !self.enabled.get() || self.pid.get().is_some() {
            return;
        }
        let Some(bin) = find_worker() else {
            self.set_state(State::Missing);
            return;
        };
        let work_dir = config::worker_dir();
        if let Err(e) = prepare_work_dir(&work_dir) {
            self.set_state(State::Failed(format!(
                "Could not create {}: {e}",
                work_dir.display()
            )));
            return;
        }
        let mut cmd = Command::new(&bin);
        cmd.arg("--library")
            .arg(&*self.root.borrow())
            .arg("--work-dir")
            .arg(&work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped());
        // SAFETY: prctl is async-signal-safe. The worker gets SIGTERM when
        // the app dies without stopping it (crash, SIGKILL).
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.set_state(State::Failed(format!(
                    "Could not start {}: {e}",
                    bin.display()
                )));
                return;
            }
        };
        let pid = child.id() as i32;
        if let Some(stderr) = child.stderr.take() {
            let last = self.last_line.clone();
            let _ = std::thread::Builder::new()
                .name("worker-log".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                        // The worker prefixes its own lines.
                        eprintln!("{line}");
                        if !line.trim().is_empty() {
                            *last.lock().unwrap() = line;
                        }
                    }
                });
        }
        // The child watch reaps the process; `child` must not wait for it.
        drop(child);
        self.pid.set(Some(pid));
        self.stopping.set(false);
        self.started.set(Some(Instant::now()));
        self.last_line.lock().unwrap().clear();
        let weak = self.me.clone();
        glib::child_watch_add_local(glib::Pid(pid), move |_, status| {
            if let Some(d) = weak.upgrade() {
                d.on_exit(pid, status);
            }
        });
        self.set_state(State::Running);
    }

    fn on_exit(&self, pid: i32, status: i32) {
        if self.pid.get() != Some(pid) {
            return;
        }
        self.pid.set(None);
        let requested = self.stopping.replace(false);
        if requested {
            if self.enabled.get() && self.restart_when_stopped.replace(false) {
                self.failures.set(0);
                self.start();
            } else {
                self.set_state(State::Off);
            }
            return;
        }
        let code = libc::WIFEXITED(status).then(|| libc::WEXITSTATUS(status));
        let last = self.last_line.lock().unwrap().clone();
        let how = match code {
            Some(c) => format!("stopped unexpectedly (exit status {c})"),
            None => format!("stopped unexpectedly (signal {})", libc::WTERMSIG(status)),
        };
        let reason = if last.is_empty() {
            how
        } else {
            format!("{how}; its last message was “{last}”")
        };
        eprintln!("tunemesh: {WORKER_BIN} {reason}");
        if code == Some(EXIT_USAGE) {
            let msg = if last.is_empty() {
                String::new()
            } else {
                format!(": “{last}”")
            };
            self.set_state(State::Failed(format!(
                "The downloader refused to start{msg}. Turn it off and on again after fixing this."
            )));
            return;
        }
        if !self.enabled.get() {
            self.set_state(State::Off);
            return;
        }
        let healthy = self
            .started
            .get()
            .is_some_and(|t| t.elapsed() >= HEALTHY_RUN);
        let failures = if healthy { 1 } else { self.failures.get() + 1 };
        self.failures.set(failures);
        let delay = Duration::from_secs((1u64 << failures.min(6)).min(MAX_DELAY_SECS));
        let weak = self.me.clone();
        let id = glib::timeout_add_local_once(delay, move || {
            if let Some(d) = weak.upgrade() {
                d.retry.borrow_mut().take();
                d.start();
            }
        });
        *self.retry.borrow_mut() = Some(id);
        self.set_state(State::Restarting {
            at: Instant::now() + delay,
            reason,
        });
    }

    /// Asks the worker to stop (SIGTERM); kills it if it is still there
    /// after a grace period. Returns at once.
    pub fn stop(&self) {
        if let Some(id) = self.retry.borrow_mut().take() {
            id.remove();
        }
        let Some(pid) = self.pid.get() else { return };
        self.stopping.set(true);
        // SAFETY: plain syscall on our own child.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let weak = self.me.clone();
        glib::timeout_add_local_once(STOP_GRACE, move || {
            if let Some(d) = weak.upgrade()
                && d.pid.get() == Some(pid)
            {
                // SAFETY: still our unreaped child, so the pid is not reused.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        });
    }

    /// Stops the worker and waits for it to exit (used when quitting).
    pub fn stop_blocking(&self) {
        self.enabled.set(false);
        if let Some(id) = self.retry.borrow_mut().take() {
            id.remove();
        }
        let Some(pid) = self.pid.get() else { return };
        self.stopping.set(true);
        // SAFETY: plain syscalls on our own child.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = Instant::now() + STOP_GRACE;
        while !exited(pid) {
            if Instant::now() >= deadline {
                eprintln!("tunemesh: {WORKER_BIN} did not stop in time; killing it");
                // SAFETY: unreaped child, pid not reused.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether this device's worker has requests to finish: it is on (running
    /// or about to restart), requests are waiting and no other device is the
    /// active downloader.
    pub fn has_pending_work(&self) -> bool {
        if !self.enabled.get()
            || !matches!(
                *self.state.borrow(),
                State::Running | State::Restarting { .. }
            )
        {
            return false;
        }
        let root = self.root.borrow().clone();
        let (requests, _) = tunemesh_core::request::list_requests(&root);
        !requests.is_empty() && active_worker(&root).is_none_or(|w| is_this_device(&w))
    }

    pub fn pending_count(&self) -> usize {
        tunemesh_core::request::list_requests(&self.root.borrow())
            .0
            .len()
    }
}

/// Whether `pid` has exited, without reaping it (the child watch does).
fn exited(pid: i32) -> bool {
    // SAFETY: zeroed siginfo_t is a valid out-parameter for waitid.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: valid pointer; WNOWAIT leaves the child reapable.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    // SAFETY: si_pid is set by waitid when a child changed state.
    rc != 0 || unsafe { info.si_pid() } == pid
}

// ---- `.tunemesh/worker.json` --------------------------------------------

pub use tunemesh_core::worker::active as active_worker;

fn is_this_device(w: &WorkerInfo) -> bool {
    w.device == config::device_name()
}

/// What the Downloads page, Settings and the setup assistant say about who
/// downloads, and whether it is a problem the user should see in red.
pub fn summary(root: &Path, state: &State) -> (String, bool) {
    let active = active_worker(root);
    let mut text = match &active {
        Some(w) if is_this_device(w) => {
            format!("Downloads are handled by this device ({}).", w.device)
        }
        Some(w) => format!("Downloads are handled by {}.", w.device),
        None => "No device is downloading right now.".to_owned(),
    };
    let extra = match (state, &active) {
        (State::Off, None) => Some((
            "Requests wait until a device runs the downloader; turn on “This device downloads songs” in Settings to download here.".to_owned(),
            false,
        )),
        (State::Off, Some(_)) => None,
        (State::Running, Some(w)) if is_this_device(w) => None,
        (State::Running, Some(w)) => Some((format!("The downloader on this device waits and takes over if {} stops.", w.device), false)),
        (State::Running, None) => Some(("The downloader is starting on this device.".to_owned(), false)),
        (State::Missing, _) => Some((
            format!("The downloader program ({WORKER_BIN}) is not installed: it was not found next to TuneMesh or on PATH."),
            true,
        )),
        (State::Restarting { at, reason }, _) => {
            let secs = at.saturating_duration_since(Instant::now()).as_secs().max(1);
            Some((format!("The downloader {reason}. Restarting in {secs} s."), true))
        }
        (State::Failed(e), _) => Some((e.clone(), true)),
    };
    let mut error = false;
    if let Some((t, e)) = extra {
        text.push(' ');
        text.push_str(&t);
        error = e;
    }
    (text, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_dir_is_private() {
        let dir = std::env::temp_dir()
            .join(format!("tunemesh-workdir-test-{}", std::process::id()))
            .join("worker");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_work_dir(&dir).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
