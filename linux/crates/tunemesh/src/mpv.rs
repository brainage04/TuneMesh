//! A minimal binding to libmpv's client API (mpv/client.h, API 2.x), used
//! for audio only.

use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::ptr;
use std::sync::Arc;

#[repr(C)]
struct MpvHandle {
    _private: [u8; 0],
}

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventStartFile {
    playlist_entry_id: i64,
}

#[repr(C)]
struct RawEventEndFile {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}

const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_SHUTDOWN: c_int = 1;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PROPERTY_CHANGE: c_int = 22;

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut MpvHandle;
    fn mpv_initialize(ctx: *mut MpvHandle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut MpvHandle);
    fn mpv_set_option_string(ctx: *mut MpvHandle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_command(ctx: *mut MpvHandle, args: *mut *const c_char) -> c_int;
    fn mpv_set_property(ctx: *mut MpvHandle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_get_property(ctx: *mut MpvHandle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_observe_property(ctx: *mut MpvHandle, userdata: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: c_double) -> *mut RawEvent;
    fn mpv_error_string(error: c_int) -> *const c_char;
}

fn error_string(code: c_int) -> String {
    // SAFETY: mpv_error_string returns a static string for any code.
    unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy().into_owned()
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("no interior NUL")
}

#[derive(Debug)]
pub enum EndReason {
    Eof,
    Stop,
    Error(String),
    Other,
}

#[derive(Debug)]
pub enum Event {
    StartFile { entry_id: i64 },
    FileLoaded,
    EndFile { reason: EndReason, entry_id: i64 },
    Pause(bool),
    Duration(f64),
    Shutdown,
}

/// Observed property ids.
const PROP_PAUSE: u64 = 1;
const PROP_DURATION: u64 = 2;

pub struct Mpv {
    handle: *mut MpvHandle,
}

// SAFETY: the mpv client API is thread-safe for all functions used here.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    /// Creates an audio-only player instance.
    pub fn new() -> Result<Arc<Self>, String> {
        // libmpv refuses to start unless numbers are formatted the C way;
        // GTK has set the user's locale by now.
        // SAFETY: called on the main thread before any other thread uses
        // locale-dependent functions.
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        // SAFETY: plain constructor; null on failure.
        let handle = unsafe { mpv_create() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        for (k, v) in [
            ("config", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            ("ytdl", "no"),
            ("load-scripts", "no"),
            // Built-in scripts each run their own Lua VM and thread; none
            // matter for audio-only playback.
            ("load-stats-overlay", "no"),
            ("load-console", "no"),
            ("load-select", "no"),
            ("load-positioning", "no"),
            ("load-context-menu", "no"),
            ("load-commands", "no"),
            ("load-auto-profiles", "no"),
            ("input-terminal", "no"),
            ("video", "no"),
            ("audio-display", "no"),
            ("idle", "yes"),
            ("gapless-audio", "weak"),
            ("prefetch-playlist", "yes"),
            ("volume-max", "100"),
            ("audio-client-name", "TuneMesh"),
            ("title", "TuneMesh"),
            ("demuxer-max-bytes", "8MiB"),
            ("demuxer-max-back-bytes", "1MiB"),
        ] {
            let (k, v) = (cstr(k), cstr(v));
            // SAFETY: valid handle and NUL-terminated strings.
            let rc = unsafe { mpv_set_option_string(handle, k.as_ptr(), v.as_ptr()) };
            if rc < 0 {
                eprintln!("tunemesh: mpv option {k:?}: {}", error_string(rc));
            }
        }
        // SAFETY: valid, not yet initialised handle.
        let rc = unsafe { mpv_initialize(handle) };
        if rc < 0 {
            // SAFETY: handle is valid and not used afterwards.
            unsafe { mpv_terminate_destroy(handle) };
            return Err(format!("mpv_initialize: {}", error_string(rc)));
        }
        let mpv = Arc::new(Self { handle });
        mpv.observe(PROP_PAUSE, "pause", FORMAT_FLAG);
        mpv.observe(PROP_DURATION, "duration", FORMAT_DOUBLE);
        Ok(mpv)
    }

    fn observe(&self, id: u64, name: &str, format: c_int) {
        let name = cstr(name);
        // SAFETY: valid handle and string.
        unsafe { mpv_observe_property(self.handle, id, name.as_ptr(), format) };
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        // SAFETY: NULL-terminated array of valid strings that outlive the call.
        let rc = unsafe { mpv_command(self.handle, ptrs.as_mut_ptr()) };
        if rc < 0 { Err(format!("{}: {}", args.join(" "), error_string(rc))) } else { Ok(()) }
    }

    pub fn set_flag(&self, name: &str, value: bool) {
        let name = cstr(name);
        let mut v: c_int = value.into();
        // SAFETY: FLAG takes a pointer to int.
        unsafe { mpv_set_property(self.handle, name.as_ptr(), FORMAT_FLAG, (&mut v as *mut c_int).cast()) };
    }

    pub fn set_f64(&self, name: &str, value: f64) {
        let name = cstr(name);
        let mut v: c_double = value;
        // SAFETY: DOUBLE takes a pointer to double.
        unsafe { mpv_set_property(self.handle, name.as_ptr(), FORMAT_DOUBLE, (&mut v as *mut c_double).cast()) };
    }

    pub fn set_str(&self, name: &str, value: &str) {
        let (name, value) = (cstr(name), cstr(value));
        let mut p = value.as_ptr();
        // SAFETY: STRING takes a pointer to char*.
        unsafe { mpv_set_property(self.handle, name.as_ptr(), FORMAT_STRING, (&mut p as *mut *const c_char).cast()) };
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let name = cstr(name);
        let mut v: c_double = 0.0;
        // SAFETY: DOUBLE writes one double.
        let rc = unsafe { mpv_get_property(self.handle, name.as_ptr(), FORMAT_DOUBLE, (&mut v as *mut c_double).cast()) };
        (rc >= 0).then_some(v)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let name = cstr(name);
        let mut v: i64 = 0;
        // SAFETY: INT64 writes one int64.
        let rc = unsafe { mpv_get_property(self.handle, name.as_ptr(), FORMAT_INT64, (&mut v as *mut i64).cast()) };
        (rc >= 0).then_some(v)
    }

    /// Blocks until the next event. Call from one thread only.
    pub fn wait_event(&self) -> Option<Event> {
        // SAFETY: the returned event is valid until the next wait_event call
        // on this handle, and only this thread calls it.
        unsafe {
            let ev = &*mpv_wait_event(self.handle, -1.0);
            match ev.event_id {
                EVENT_SHUTDOWN => Some(Event::Shutdown),
                EVENT_START_FILE => {
                    let d = &*(ev.data as *const RawEventStartFile);
                    Some(Event::StartFile { entry_id: d.playlist_entry_id })
                }
                EVENT_FILE_LOADED => Some(Event::FileLoaded),
                EVENT_END_FILE => {
                    let d = &*(ev.data as *const RawEventEndFile);
                    let reason = match d.reason {
                        0 => EndReason::Eof,
                        2 => EndReason::Stop,
                        4 => EndReason::Error(error_string(d.error)),
                        _ => EndReason::Other,
                    };
                    Some(Event::EndFile { reason, entry_id: d.playlist_entry_id })
                }
                EVENT_PROPERTY_CHANGE => {
                    let p = &*(ev.data as *const RawEventProperty);
                    if p.data.is_null() {
                        return None;
                    }
                    match (ev.reply_userdata, p.format) {
                        (PROP_PAUSE, FORMAT_FLAG) => Some(Event::Pause(*(p.data as *const c_int) != 0)),
                        (PROP_DURATION, FORMAT_DOUBLE) => Some(Event::Duration(*(p.data as *const c_double))),
                        _ => None,
                    }
                }
                _ => None,
            }
        }
    }

    /// Asks the core to shut down; the event thread then sees `Shutdown`.
    pub fn quit(&self) {
        let _ = self.command(&["quit"]);
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        // SAFETY: last reference; no thread waits on the handle any more.
        unsafe { mpv_terminate_destroy(self.handle) };
    }
}
