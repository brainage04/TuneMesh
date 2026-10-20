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

/// `MPV_ERROR_OPTION_NOT_FOUND` from mpv/client.h.
const MPV_ERROR_OPTION_NOT_FOUND: c_int = -5;

const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;
const FORMAT_NODE_MAP: c_int = 8;

/// `mpv_node` from mpv/client.h (only the string and map forms are built).
#[repr(C)]
struct Node {
    u: NodeValue,
    format: c_int,
}

#[repr(C)]
union NodeValue {
    string: *const c_char,
    list: *mut NodeList,
    int64: i64,
}

#[repr(C)]
struct NodeList {
    num: c_int,
    values: *mut Node,
    keys: *mut *const c_char,
}

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
    fn mpv_set_option_string(
        ctx: *mut MpvHandle,
        name: *const c_char,
        data: *const c_char,
    ) -> c_int;
    fn mpv_command(ctx: *mut MpvHandle, args: *mut *const c_char) -> c_int;
    fn mpv_command_node(ctx: *mut MpvHandle, args: *mut Node, result: *mut Node) -> c_int;
    fn mpv_free_node_contents(node: *mut Node);
    fn mpv_set_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_get_property(
        ctx: *mut MpvHandle,
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    ) -> c_int;
    fn mpv_observe_property(
        ctx: *mut MpvHandle,
        userdata: u64,
        name: *const c_char,
        format: c_int,
    ) -> c_int;
    fn mpv_wait_event(ctx: *mut MpvHandle, timeout: c_double) -> *mut RawEvent;
    fn mpv_error_string(error: c_int) -> *const c_char;
}

fn error_string(code: c_int) -> String {
    // SAFETY: mpv_error_string returns a static string for any code.
    unsafe { CStr::from_ptr(mpv_error_string(code)) }
        .to_string_lossy()
        .into_owned()
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

/// Audio-only playback without user config, scripts or key bindings.
const BASE_OPTIONS: &[(&str, &str)] = &[
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
    // Nothing next to a file is loaded automatically.
    ("autoload-files", "no"),
    ("cover-art-auto", "no"),
    ("audio-file-auto", "no"),
    ("sub-auto", "no"),
];

/// Library files arrive from every synced device, so a "song" may really be
/// a playlist or a reference file pointing at URLs. mpv only plays the local
/// file itself: libavformat demuxes it (mpv's own playlist, EDL, CUE and
/// archive demuxers are never tried), references inside files are not
/// followed and nested opens are limited to local files. If mpv rejects any
/// of these, playback stays unavailable.
const LOCAL_ONLY: &[(&str, &str)] = &[
    ("demuxer", "lavf"),
    ("access-references", "no"),
    ("demuxer-lavf-o", "protocol_whitelist=file"),
    ("load-unsafe-playlists", "no"),
];

pub struct Mpv {
    handle: *mut MpvHandle,
}

// SAFETY: the mpv client API is thread-safe for all functions used here.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    /// Creates an audio-only player instance that opens local files only.
    pub fn new() -> Result<Arc<Self>, String> {
        Self::create(BASE_OPTIONS, LOCAL_ONLY)
    }

    /// `required` options must all be accepted, `base` ones are best effort.
    fn create(base: &[(&str, &str)], required: &[(&str, &str)]) -> Result<Arc<Self>, String> {
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
        let options = base
            .iter()
            .map(|o| (o, false))
            .chain(required.iter().map(|o| (o, true)));
        for (&(k, v), must) in options {
            let (ck, cv) = (cstr(k), cstr(v));
            // SAFETY: valid handle and NUL-terminated strings.
            let rc = unsafe { mpv_set_option_string(handle, ck.as_ptr(), cv.as_ptr()) };
            if rc < 0 {
                let e = format!("mpv option {k}={v}: {}", error_string(rc));
                if must {
                    // SAFETY: handle is valid and not used afterwards.
                    unsafe { mpv_terminate_destroy(handle) };
                    return Err(e);
                }
                // Older libmpv (0.37 on Ubuntu 24.04) lacks some built-in
                // scripts; an unknown option there means nothing to turn off.
                if rc != MPV_ERROR_OPTION_NOT_FOUND {
                    eprintln!("tunemesh: {e}");
                }
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
        if rc < 0 {
            Err(format!("{}: {}", args.join(" "), error_string(rc)))
        } else {
            Ok(())
        }
    }

    /// Runs a command given by named arguments (`{"name": .., key: value}`).
    /// Named arguments keep working when mpv inserts positional ones: mpv
    /// 0.38 added `index` before `options` in `loadfile`, so the positional
    /// form that works there fails on 0.37 (Ubuntu 24.04).
    pub fn command_named(&self, name: &str, args: &[(&str, &str)]) -> Result<(), String> {
        let owned: Vec<(CString, CString)> = std::iter::once(("name", name))
            .chain(args.iter().copied())
            .map(|(k, v)| (cstr(k), cstr(v)))
            .collect();
        let mut keys: Vec<*const c_char> = owned.iter().map(|(k, _)| k.as_ptr()).collect();
        let mut values: Vec<Node> = owned
            .iter()
            .map(|(_, v)| Node {
                u: NodeValue { string: v.as_ptr() },
                format: FORMAT_STRING,
            })
            .collect();
        let mut list = NodeList {
            num: values.len() as c_int,
            values: values.as_mut_ptr(),
            keys: keys.as_mut_ptr(),
        };
        let mut arg = Node {
            u: NodeValue { list: &mut list },
            format: FORMAT_NODE_MAP,
        };
        let mut result = Node {
            u: NodeValue { int64: 0 },
            format: 0,
        };
        // SAFETY: the map and its strings outlive the call; mpv only reads
        // `arg` and fills `result`, which is freed with mpv's own function.
        let rc = unsafe {
            let rc = mpv_command_node(self.handle, &mut arg, &mut result);
            if rc >= 0 {
                mpv_free_node_contents(&mut result);
            }
            rc
        };
        if rc < 0 {
            let shown: Vec<String> = args.iter().map(|(k, v)| format!("{k}={v}")).collect();
            Err(format!("{name} {}: {}", shown.join(" "), error_string(rc)))
        } else {
            Ok(())
        }
    }

    /// `loadfile` with `flags` (`replace`, `append`, ...) and optional
    /// per-file options, compatible with libmpv 0.37 and newer.
    pub fn loadfile(&self, url: &str, flags: &str, options: Option<&str>) -> Result<(), String> {
        match options {
            Some(o) => self.command_named(
                "loadfile",
                &[("url", url), ("flags", flags), ("options", o)],
            ),
            None => self.command_named("loadfile", &[("url", url), ("flags", flags)]),
        }
    }

    pub fn set_flag(&self, name: &str, value: bool) {
        let name = cstr(name);
        let mut v: c_int = value.into();
        // SAFETY: FLAG takes a pointer to int.
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                FORMAT_FLAG,
                (&mut v as *mut c_int).cast(),
            )
        };
    }

    pub fn set_f64(&self, name: &str, value: f64) {
        let name = cstr(name);
        let mut v: c_double = value;
        // SAFETY: DOUBLE takes a pointer to double.
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                FORMAT_DOUBLE,
                (&mut v as *mut c_double).cast(),
            )
        };
    }

    pub fn set_str(&self, name: &str, value: &str) {
        let (name, value) = (cstr(name), cstr(value));
        let mut p = value.as_ptr();
        // SAFETY: STRING takes a pointer to char*.
        unsafe {
            mpv_set_property(
                self.handle,
                name.as_ptr(),
                FORMAT_STRING,
                (&mut p as *mut *const c_char).cast(),
            )
        };
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let name = cstr(name);
        let mut v: c_double = 0.0;
        // SAFETY: DOUBLE writes one double.
        let rc = unsafe {
            mpv_get_property(
                self.handle,
                name.as_ptr(),
                FORMAT_DOUBLE,
                (&mut v as *mut c_double).cast(),
            )
        };
        (rc >= 0).then_some(v)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let name = cstr(name);
        let mut v: i64 = 0;
        // SAFETY: INT64 writes one int64.
        let rc = unsafe {
            mpv_get_property(
                self.handle,
                name.as_ptr(),
                FORMAT_INT64,
                (&mut v as *mut i64).cast(),
            )
        };
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
                    Some(Event::StartFile {
                        entry_id: d.playlist_entry_id,
                    })
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
                    Some(Event::EndFile {
                        reason,
                        entry_id: d.playlist_entry_id,
                    })
                }
                EVENT_PROPERTY_CHANGE => {
                    let p = &*(ev.data as *const RawEventProperty);
                    if p.data.is_null() {
                        return None;
                    }
                    match (ev.reply_userdata, p.format) {
                        (PROP_PAUSE, FORMAT_FLAG) => {
                            Some(Event::Pause(*(p.data as *const c_int) != 0))
                        }
                        (PROP_DURATION, FORMAT_DOUBLE) => {
                            Some(Event::Duration(*(p.data as *const c_double)))
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    /// Loads a fake `.mp3` holding `body` (with `{URL}` replaced by a local
    /// listener's URL) and reports whether mpv connected to the listener.
    fn fetches(required: &[(&str, &str)], body: &str) -> bool {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/song.mp3", listener.local_addr().unwrap());
        let dir = std::env::temp_dir().join(format!(
            "tunemesh-mpv-test-{}-{}",
            std::process::id(),
            fastrand::u64(..)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Song [abc].mp3");
        std::fs::write(&file, body.replace("{URL}", &url)).unwrap();

        let base: Vec<_> = BASE_OPTIONS
            .iter()
            .copied()
            .chain([("ao", "null"), ("audio-fallback-to-null", "yes")])
            .collect();
        let mpv = Mpv::create(&base, required).unwrap();
        mpv.loadfile(file.to_str().unwrap(), "replace", None)
            .unwrap();
        let start = Instant::now();
        let mut connected = false;
        while start.elapsed() < Duration::from_secs(4) {
            if listener.accept().is_ok() {
                connected = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        mpv.quit();
        let _ = std::fs::remove_dir_all(&dir);
        connected
    }

    const M3U: &str = "#EXTM3U\n#EXTINF:1,Song\n{URL}\n";
    const HLS: &str = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\n{URL}\n#EXT-X-ENDLIST\n";

    #[test]
    fn a_playlist_disguised_as_a_song_fetches_nothing() {
        // Control: stock mpv does follow the URL, so the probe works.
        assert!(
            fetches(&[], M3U),
            "the unhardened control did not fetch; the test proves nothing"
        );
        assert!(!fetches(LOCAL_ONLY, M3U));
        assert!(!fetches(LOCAL_ONLY, HLS));
    }

    /// A silent 8 kHz mono WAV of `secs` seconds.
    fn wav(secs: u32) -> Vec<u8> {
        let data = 8000 * 2 * secs;
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data.to_le_bytes());
        v.resize(v.len() + data as usize, 0);
        v
    }

    #[test]
    fn a_track_swapped_for_a_symlink_after_opening_still_plays_the_original() {
        let base = std::env::temp_dir().join(format!("tunemesh-swap-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        std::fs::create_dir_all(root.join("Artist/Album")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let rel = "Artist/Album/Song [s].wav";
        std::fs::write(root.join(rel), wav(1)).unwrap();
        std::fs::write(outside.join("other.wav"), wav(5)).unwrap();

        let track = crate::player::open_track(&root, rel).unwrap();
        // The path now points outside the library.
        std::fs::remove_file(root.join(rel)).unwrap();
        std::os::unix::fs::symlink(outside.join("other.wav"), root.join(rel)).unwrap();
        assert!(
            crate::player::open_track(&root, rel).is_err(),
            "a symlinked track must be refused"
        );

        let base_opts: Vec<_> = BASE_OPTIONS
            .iter()
            .copied()
            .chain([
                ("ao", "null"),
                ("audio-fallback-to-null", "yes"),
                ("pause", "yes"),
            ])
            .collect();
        let mpv = Mpv::create(&base_opts, LOCAL_ONLY).unwrap();
        mpv.loadfile(&track.mpv_path, "replace", None).unwrap();
        let start = Instant::now();
        let mut duration = None;
        while start.elapsed() < Duration::from_secs(4) && duration.is_none() {
            duration = mpv.get_f64("duration");
            std::thread::sleep(Duration::from_millis(25));
        }
        mpv.quit();
        drop(track);
        std::fs::remove_dir_all(&base).unwrap();
        let d = duration.expect("mpv did not load the open track");
        assert!(
            (d - 1.0).abs() < 0.1,
            "played a {d} s file, not the 1 s original"
        );
    }

    #[test]
    fn playback_skips_a_queued_track_deleted_after_the_scan() {
        let root = std::env::temp_dir().join(format!("tunemesh-skip-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.wav"), wav(1)).unwrap();
        std::fs::write(root.join("b.wav"), wav(3)).unwrap();
        std::fs::write(root.join("c.wav"), wav(2)).unwrap();
        // b goes away while a plays.
        std::fs::remove_file(root.join("b.wav")).unwrap();
        let first = crate::player::open_track(&root, "a.wav").unwrap();
        let (index, path, next) =
            crate::player::first_playable(&root, vec![(1, "b.wav".into()), (2, "c.wav".into())])
                .expect("c is playable");
        assert_eq!((index, path.as_str()), (2, "c.wav"));

        let base: Vec<_> = BASE_OPTIONS
            .iter()
            .copied()
            .chain([("ao", "null"), ("audio-fallback-to-null", "yes")])
            .collect();
        let mpv = Mpv::create(&base, LOCAL_ONLY).unwrap();
        mpv.loadfile(&first.mpv_path, "replace", None).unwrap();
        mpv.loadfile(&next.mpv_path, "append", None).unwrap();
        let start = Instant::now();
        let mut reached = None;
        while start.elapsed() < Duration::from_secs(6) {
            if mpv.get_i64("playlist-pos") == Some(1)
                && let Some(d) = mpv.get_f64("duration")
            {
                reached = Some(d);
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        mpv.quit();
        drop((first, next));
        std::fs::remove_dir_all(&root).unwrap();
        let d = reached.expect("playback did not continue after the first track");
        assert!((d - 2.0).abs() < 0.1, "played a {d} s file, not c (2 s)");
    }

    /// The app's exact load call: replace with a start time, then append the
    /// next track. Run against libmpv 0.37 (Ubuntu 24.04) and newer.
    #[test]
    fn loadfile_replace_with_start_and_append_play() {
        let root = std::env::temp_dir().join(format!("tunemesh-load-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.wav"), wav(3)).unwrap();
        std::fs::write(root.join("b.wav"), wav(2)).unwrap();
        let a = crate::player::open_track(&root, "a.wav").unwrap();
        let b = crate::player::open_track(&root, "b.wav").unwrap();
        let base: Vec<_> = BASE_OPTIONS
            .iter()
            .copied()
            .chain([("ao", "null"), ("audio-fallback-to-null", "yes")])
            .collect();
        let mpv = Mpv::create(&base, LOCAL_ONLY).unwrap();
        mpv.loadfile(&a.mpv_path, "replace", Some("start=1.500"))
            .unwrap();
        mpv.loadfile(&b.mpv_path, "append", None).unwrap();
        let start = Instant::now();
        let mut first_pos = None;
        let mut reached_b = false;
        while start.elapsed() < Duration::from_secs(8) {
            if first_pos.is_none()
                && mpv.get_i64("playlist-pos") == Some(0)
                && let Some(t) = mpv.get_f64("time-pos")
            {
                first_pos = Some(t);
            }
            if mpv.get_i64("playlist-pos") == Some(1)
                && mpv.get_f64("time-pos").is_some_and(|t| t > 0.2)
            {
                reached_b = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        mpv.quit();
        drop((a, b));
        std::fs::remove_dir_all(&root).unwrap();
        let t = first_pos.expect("the first track never started");
        assert!((1.4..2.5).contains(&t), "started at {t} s, not at 1.5 s");
        assert!(
            reached_b,
            "playback did not continue into the appended track"
        );
    }

    #[test]
    fn a_failing_named_command_reports_the_error() {
        let mpv = Mpv::create(BASE_OPTIONS, LOCAL_ONLY).unwrap();
        let e = mpv.command_named("loadfile", &[("url", "/x"), ("flags", "no-such-flag")]);
        mpv.quit();
        assert!(e.is_err());
    }
}
