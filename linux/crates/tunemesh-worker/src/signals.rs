//! SIGTERM/SIGINT: stop cleanly. The handler only sets a flag; the main loop
//! and running downloads poll it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

pub fn install() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
}

pub fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Sleeps up to `d`, returning early when a stop is requested.
pub fn sleep(d: Duration) {
    let end = Instant::now() + d;
    while !stop_requested() {
        let now = Instant::now();
        if now >= end {
            break;
        }
        thread::sleep((end - now).min(Duration::from_millis(100)));
    }
}
