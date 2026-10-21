//! Playback: the queue (context, shuffle, repeat, user-queued tracks) on top
//! of libmpv. mpv's own playlist only ever holds the current track and the
//! next one, so gapless playback works while TuneMesh owns the queue.

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use gtk::glib;
use serde::{Deserialize, Serialize};

use crate::config;
use crate::mpv::{EndReason, Event, Mpv};

/// An open library track and the path mpv opens it by.
///
/// Library files arrive from every synced device, and a file can be swapped
/// for a symlink after the scan. So the app opens each track itself, below
/// the root without following symlinks, and mpv opens `/proc/self/fd/N`:
/// that names the inode the app opened, whatever happens to the path later.
/// The `File` must stay open until mpv has opened the track.
pub struct OpenTrack {
    file: File,
    pub mpv_path: String,
}

pub fn open_track(root: &Path, rel: &str) -> std::io::Result<OpenTrack> {
    let file = tunemesh_core::fsx::Dir::open(root)?.open_file(rel)?;
    let mpv_path = format!("/proc/self/fd/{}", file.as_raw_fd());
    Ok(OpenTrack { file, mpv_path })
}

/// The first of `candidates` (queue index, path) that opens safely.
pub(crate) fn first_playable(
    root: &Path,
    candidates: Vec<(usize, String)>,
) -> Option<(usize, String, OpenTrack)> {
    candidates
        .into_iter()
        .find_map(|(i, path)| match open_track(root, &path) {
            Ok(t) => Some((i, path, t)),
            Err(e) => {
                eprintln!("tunemesh: skipping {path:?}: {e}");
                None
            }
        })
}

impl OpenTrack {
    fn into_file(self) -> File {
        self.file
    }
}

/// Files of entries mpv dropped are kept open at most this long: until
/// mpv reports progress, and never more than this many.
const MAX_RETIRED: usize = 2;

/// The open files behind mpv's entries.
///
/// mpv opens `/proc/self/fd/N` some time after `loadfile`, so a file of an
/// entry mpv dropped stays open until mpv has moved on (a file loaded, an
/// entry ended); otherwise its number could be reused by an unrelated file
/// before mpv opens it. At most current + next + `MAX_RETIRED` are open.
#[derive(Default)]
struct TrackFiles {
    current: Option<File>,
    next: Option<File>,
    retired: std::collections::VecDeque<File>,
}

impl TrackFiles {
    fn retire(&mut self, f: Option<File>) {
        if let Some(f) = f {
            if self.retired.len() == MAX_RETIRED {
                self.retired.pop_front();
            }
            self.retired.push_back(f);
        }
    }
    fn retire_current(&mut self) {
        let f = self.current.take();
        self.retire(f);
    }
    fn retire_next(&mut self) {
        let f = self.next.take();
        self.retire(f);
    }
    /// The preloaded next entry became the current one.
    fn advance(&mut self) {
        self.retire_current();
        self.current = self.next.take();
    }
    /// mpv loaded a file or ended an entry: dropped entries are done with.
    fn mpv_moved_on(&mut self) {
        self.retired.clear();
    }
    /// mpv ended playback (end of queue or a failed load): nothing it
    /// plays needs a file any more.
    fn release_all(&mut self) {
        *self = Self::default();
    }
    #[cfg(test)]
    fn open_count(&self) -> usize {
        self.retired.len() + usize::from(self.current.is_some()) + usize::from(self.next.is_some())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueItem {
    pub path: String,
    /// Added with "Play next"/"Add to queue" rather than from the context.
    #[serde(default)]
    pub user: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Track,
    Playing,
    Position,
    Queue,
    Options,
    /// The user (or an MPRIS client) jumped to a position.
    Seeked,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    items: Vec<QueueItem>,
    current: Option<usize>,
    position: f64,
    shuffle: bool,
    repeat: Repeat,
    volume: f64,
    context: Vec<String>,
    context_name: String,
}

#[derive(Default)]
struct State {
    items: Vec<QueueItem>,
    current: Option<usize>,
    /// The context in its original order (for un-shuffling).
    context: Vec<String>,
    context_name: String,
    shuffle: bool,
    repeat: Repeat,
    volume: f64,
    playing: bool,
    position: f64,
    duration: f64,
    /// Whether mpv has the current track loaded.
    loaded: bool,
    current_entry: Option<i64>,
    /// mpv playlist entry id, queue index and path of the preloaded next track.
    next_entry: Option<(i64, usize, String)>,
}

impl State {
    /// Queue indices that may follow the current track, in play order:
    /// the rest of the queue, then (with repeat all) from the start again.
    fn upcoming(&self) -> Vec<usize> {
        let Some(cur) = self.current else {
            return Vec::new();
        };
        let mut v: Vec<usize> = (cur + 1..self.items.len()).collect();
        if self.repeat == Repeat::All {
            v.extend(0..=cur.min(self.items.len().saturating_sub(1)));
        }
        v
    }

    fn next_index(&self) -> Option<usize> {
        let cur = self.current?;
        if cur + 1 < self.items.len() {
            Some(cur + 1)
        } else if self.repeat == Repeat::All && !self.items.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    /// Indices after the current one that the user queued, in order.
    fn upcoming_user(&self) -> Vec<QueueItem> {
        let start = self.current.map_or(0, |c| c + 1);
        self.items[start.min(self.items.len())..]
            .iter()
            .filter(|i| i.user)
            .cloned()
            .collect()
    }

    /// Drops `paths` from the queue and the context. Returns whether the
    /// current item was dropped; the current item is then the first kept item
    /// that followed it (wrapping to the start with `Repeat::All`), or none.
    fn remove_paths(&mut self, paths: &[String]) -> bool {
        let cur = self.current;
        let current_gone = cur
            .and_then(|c| self.items.get(c))
            .is_some_and(|i| paths.contains(&i.path));
        let mut new_cur = None;
        let mut kept = Vec::with_capacity(self.items.len());
        for (i, item) in std::mem::take(&mut self.items).into_iter().enumerate() {
            if paths.contains(&item.path) {
                continue;
            }
            let is_current = cur.is_some_and(|c| if current_gone { i > c } else { i == c });
            if new_cur.is_none() && is_current {
                new_cur = Some(kept.len());
            }
            kept.push(item);
        }
        if current_gone && new_cur.is_none() && self.repeat == Repeat::All && !kept.is_empty() {
            new_cur = Some(0);
        }
        self.items = kept;
        self.current = new_cur;
        self.context.retain(|p| !paths.contains(p));
        current_gone
    }
}

type Listener = Rc<dyn Fn(Change)>;

pub struct Player {
    mpv: Option<Arc<Mpv>>,
    pub init_error: Option<String>,
    root: RefCell<PathBuf>,
    st: RefCell<State>,
    listeners: RefCell<Vec<Listener>>,
    tick: RefCell<Option<glib::SourceId>>,
    files: RefCell<TrackFiles>,
    save_scheduled: Cell<bool>,
    ticks_since_save: Cell<u32>,
    me: Weak<Player>,
}

fn state_file() -> PathBuf {
    config::state_dir().join("state.json")
}

impl Player {
    pub fn new(root: PathBuf) -> Rc<Self> {
        let (mpv, init_error) = match Mpv::new() {
            Ok(m) => (Some(m), None),
            Err(e) => {
                eprintln!("tunemesh: {e}");
                (None, Some(e))
            }
        };
        let player = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            mpv,
            init_error,
            root: RefCell::new(root),
            st: RefCell::new(State {
                volume: 70.0,
                ..Default::default()
            }),
            listeners: RefCell::new(Vec::new()),
            tick: RefCell::new(None),
            files: RefCell::new(TrackFiles::default()),
            save_scheduled: Cell::new(false),
            ticks_since_save: Cell::new(0),
        });
        player.restore();
        if let Some(mpv) = &player.mpv {
            mpv.set_f64("volume", player.st.borrow().volume);
            let (tx, rx) = async_channel::unbounded::<Event>();
            let thread_mpv = mpv.clone();
            std::thread::Builder::new()
                .name("mpv-events".into())
                .spawn(move || {
                    loop {
                        let Some(ev) = thread_mpv.wait_event() else {
                            continue;
                        };
                        let shutdown = matches!(ev, Event::Shutdown);
                        if tx.send_blocking(ev).is_err() || shutdown {
                            break;
                        }
                    }
                })
                .expect("spawn mpv event thread");
            let weak = Rc::downgrade(&player);
            glib::spawn_future_local(async move {
                while let Ok(ev) = rx.recv().await {
                    let Some(p) = weak.upgrade() else { break };
                    p.handle_event(ev);
                }
            });
        }
        player
    }

    pub fn set_root(&self, root: PathBuf) {
        *self.root.borrow_mut() = root;
    }

    pub fn connect(&self, f: impl Fn(Change) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn emit(&self, changes: &[Change]) {
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for c in changes {
            for l in &listeners {
                l(*c);
            }
            if matches!(c, Change::Track | Change::Queue | Change::Options) {
                self.schedule_save();
            }
        }
    }

    fn open(&self, rel: &str) -> Option<OpenTrack> {
        match open_track(&self.root.borrow(), rel) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("tunemesh: cannot play {rel:?}: {e}");
                None
            }
        }
    }

    // ---- read access -------------------------------------------------

    pub fn current_path(&self) -> Option<String> {
        let st = self.st.borrow();
        st.current
            .and_then(|c| st.items.get(c))
            .map(|i| i.path.clone())
    }
    pub fn current_index(&self) -> Option<usize> {
        self.st.borrow().current
    }
    pub fn is_playing(&self) -> bool {
        self.st.borrow().playing
    }
    pub fn position(&self) -> f64 {
        self.st.borrow().position
    }
    pub fn duration(&self) -> f64 {
        self.st.borrow().duration
    }
    pub fn shuffle(&self) -> bool {
        self.st.borrow().shuffle
    }
    pub fn repeat(&self) -> Repeat {
        self.st.borrow().repeat
    }
    pub fn volume(&self) -> f64 {
        self.st.borrow().volume
    }
    pub fn context_name(&self) -> String {
        self.st.borrow().context_name.clone()
    }
    pub fn items(&self) -> Vec<QueueItem> {
        self.st.borrow().items.clone()
    }
    pub fn has_track(&self) -> bool {
        self.current_path().is_some()
    }

    // ---- mpv plumbing ------------------------------------------------

    fn entry_id_at(&self, index: i64) -> Option<i64> {
        self.mpv.as_ref()?.get_i64(&format!("playlist/{index}/id"))
    }

    fn apply_loop(&self) {
        if let Some(mpv) = &self.mpv {
            let one = self.st.borrow().repeat == Repeat::One;
            mpv.set_str("loop-file", if one { "inf" } else { "no" });
        }
    }

    /// Loads the current item into mpv, replacing whatever played.
    fn load_current(&self, start: f64, play: bool) {
        let Some(mpv) = self.mpv.clone() else { return };
        let Some(path) = self.current_path() else {
            return;
        };
        self.apply_loop();
        mpv.set_flag("pause", !play);
        let start_opt = format!("start={start:.3}");
        {
            let mut files = self.files.borrow_mut();
            files.retire_current();
            files.retire_next();
        }
        let Some(track) = self.open(&path) else {
            // Unreadable or not a regular file: show it, stopped.
            let _ = mpv.command(&["stop"]);
            {
                let mut st = self.st.borrow_mut();
                st.loaded = false;
                st.current_entry = None;
                st.next_entry = None;
                st.position = 0.0;
                st.duration = 0.0;
                st.playing = false;
            }
            self.update_tick();
            self.emit(&[
                Change::Track,
                Change::Playing,
                Change::Position,
                Change::Queue,
            ]);
            return;
        };
        if let Err(e) = mpv.loadfile(&track.mpv_path, "replace", Some(&start_opt)) {
            eprintln!("tunemesh: {e}");
        }
        self.files.borrow_mut().current = Some(track.into_file());
        {
            let mut st = self.st.borrow_mut();
            st.loaded = true;
            st.current_entry = None;
            st.next_entry = None;
            st.position = start;
            st.duration = 0.0;
            st.playing = play;
        }
        let id = self.entry_id_at(0);
        self.st.borrow_mut().current_entry = id;
        self.append_next();
        self.update_tick();
        self.emit(&[
            Change::Track,
            Change::Playing,
            Change::Position,
            Change::Queue,
        ]);
    }

    /// Appends the next item to mpv's playlist (which holds only the current
    /// track at this point).
    fn append_next(&self) {
        let Some(mpv) = self.mpv.clone() else { return };
        let candidates: Vec<(usize, String)> = {
            let st = self.st.borrow();
            st.upcoming()
                .into_iter()
                .map(|i| (i, st.items[i].path.clone()))
                .collect()
        };
        // Tracks deleted or replaced since the scan are skipped, so
        // playback goes on with the next one that opens.
        let Some((index, path, track)) = first_playable(&self.root.borrow(), candidates) else {
            return;
        };
        if let Err(e) = mpv.loadfile(&track.mpv_path, "append", None) {
            eprintln!("tunemesh: {e}");
            return;
        }
        self.files.borrow_mut().next = Some(track.into_file());
        let count = mpv.get_i64("playlist-count").unwrap_or(0);
        if let Some(id) = self.entry_id_at(count - 1) {
            self.st.borrow_mut().next_entry = Some((id, index, path));
        }
    }

    /// Re-aligns mpv's preloaded next track after the queue changed.
    fn sync_next(&self) {
        if !self.st.borrow().loaded {
            return;
        }
        self.catch_up();
        let up_to_date = {
            let mut st = self.st.borrow_mut();
            let want = st.next_index();
            let want_path = want.map(|i| st.items[i].path.clone());
            match (&mut st.next_entry, want, want_path) {
                (Some((_, idx, p)), Some(i), Some(wp)) if *p == wp => {
                    *idx = i;
                    true
                }
                (None, None, _) => true,
                _ => false,
            }
        };
        if !up_to_date {
            if let Some(mpv) = &self.mpv {
                let _ = mpv.command(&["playlist-clear"]);
            }
            self.files.borrow_mut().retire_next();
            self.st.borrow_mut().next_entry = None;
            self.append_next();
        }
    }

    /// Handles an advance mpv already made but whose event is not processed yet.
    fn catch_up(&self) {
        let Some(mpv) = &self.mpv else { return };
        let next_id = self.st.borrow().next_entry.as_ref().map(|n| n.0);
        let Some(next_id) = next_id else { return };
        if let Some(pos) = mpv.get_i64("playlist-pos").filter(|p| *p >= 0)
            && self.entry_id_at(pos) == Some(next_id)
        {
            self.advanced();
        }
    }

    /// The preloaded next track became the current one.
    fn advanced(&self) {
        {
            let mut st = self.st.borrow_mut();
            let Some((id, index, _)) = st.next_entry.take() else {
                return;
            };
            st.current = Some(index);
            st.current_entry = Some(id);
            st.position = 0.0;
            st.duration = 0.0;
        }
        self.files.borrow_mut().advance();
        if let Some(mpv) = &self.mpv {
            let _ = mpv.command(&["playlist-clear"]);
        }
        self.append_next();
        self.emit(&[Change::Track, Change::Position, Change::Queue]);
    }

    fn handle_event(&self, ev: Event) {
        match ev {
            Event::StartFile { entry_id } => {
                let is_next = self
                    .st
                    .borrow()
                    .next_entry
                    .as_ref()
                    .is_some_and(|n| n.0 == entry_id);
                if is_next {
                    self.advanced();
                }
            }
            Event::EndFile { reason, entry_id } => {
                self.files.borrow_mut().mpv_moved_on();
                let (is_current, has_next) = {
                    let st = self.st.borrow();
                    (st.current_entry == Some(entry_id), st.next_entry.is_some())
                };
                if !is_current {
                    return;
                }
                match reason {
                    EndReason::Error(e) => {
                        eprintln!("tunemesh: cannot play {:?}: {e}", self.current_path());
                        if !has_next {
                            self.ended();
                        }
                    }
                    EndReason::Eof if !has_next => self.ended(),
                    _ => {}
                }
            }
            Event::Pause(paused) => {
                let changed = {
                    let mut st = self.st.borrow_mut();
                    let playing = st.loaded && !paused;
                    let changed = st.playing != playing;
                    st.playing = playing;
                    changed
                };
                if changed {
                    self.update_tick();
                    self.emit(&[Change::Playing]);
                    self.save();
                }
            }
            Event::Duration(d) => {
                self.st.borrow_mut().duration = d;
                self.emit(&[Change::Position]);
            }
            // mpv has opened its newest file: older ones are done with.
            Event::FileLoaded => self.files.borrow_mut().mpv_moved_on(),
            Event::Shutdown => {}
        }
    }

    /// The last track finished: stop at its start, paused.
    fn ended(&self) {
        // Called on mpv's end-file for the current entry: it is done with
        // every file, including one that failed to load.
        self.files.borrow_mut().release_all();
        {
            let mut st = self.st.borrow_mut();
            st.loaded = false;
            st.playing = false;
            st.position = 0.0;
            st.current_entry = None;
            st.next_entry = None;
        }
        self.update_tick();
        self.emit(&[Change::Playing, Change::Position]);
        self.save();
    }

    fn update_tick(&self) {
        let playing = self.st.borrow().playing;
        let mut tick = self.tick.borrow_mut();
        if playing && tick.is_none() {
            let weak: Weak<Self> = self.weak_self();
            *tick = Some(glib::timeout_add_local(
                Duration::from_millis(500),
                move || {
                    let Some(p) = weak.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    p.on_tick();
                    glib::ControlFlow::Continue
                },
            ));
        } else if !playing && let Some(id) = tick.take() {
            id.remove();
        }
    }

    fn on_tick(&self) {
        let Some(mpv) = &self.mpv else { return };
        if let Some(pos) = mpv.get_f64("time-pos") {
            self.st.borrow_mut().position = pos;
            self.emit(&[Change::Position]);
        }
        let n = self.ticks_since_save.get() + 1;
        if n >= 20 {
            self.save();
            self.ticks_since_save.set(0);
        } else {
            self.ticks_since_save.set(n);
        }
    }

    fn weak_self(&self) -> Weak<Self> {
        self.me.clone()
    }

    // ---- commands ----------------------------------------------------

    /// Plays `tracks` as a context starting at `start`. User-queued tracks
    /// stay queued right after the started track.
    pub fn play_tracks(
        &self,
        name: &str,
        tracks: Vec<String>,
        start: usize,
        shuffle: Option<bool>,
    ) {
        if tracks.is_empty() {
            return;
        }
        let start = start.min(tracks.len() - 1);
        {
            let mut st = self.st.borrow_mut();
            if let Some(s) = shuffle {
                st.shuffle = s;
            }
            let user = st.upcoming_user();
            st.context = tracks.clone();
            st.context_name = name.to_owned();
            let ctx = |p: &String| QueueItem {
                path: p.clone(),
                user: false,
            };
            if st.shuffle {
                let mut rest: Vec<QueueItem> = tracks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != start)
                    .map(|(_, p)| ctx(p))
                    .collect();
                fastrand::shuffle(&mut rest);
                let first = if shuffle == Some(true) && start == 0 {
                    // "Shuffle play" starts on a random track.
                    let i = fastrand::usize(..tracks.len());
                    rest = tracks
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != i)
                        .map(|(_, p)| ctx(p))
                        .collect();
                    fastrand::shuffle(&mut rest);
                    ctx(&tracks[i])
                } else {
                    ctx(&tracks[start])
                };
                st.items = std::iter::once(first).chain(user).chain(rest).collect();
                st.current = Some(0);
            } else {
                let mut items: Vec<QueueItem> = tracks[..=start].iter().map(ctx).collect();
                items.extend(user);
                items.extend(tracks[start + 1..].iter().map(ctx));
                st.items = items;
                st.current = Some(start);
            }
        }
        self.load_current(0.0, true);
        self.emit(&[Change::Options]);
    }

    pub fn toggle(&self) {
        if self.is_playing() {
            self.pause()
        } else {
            self.play()
        }
    }

    pub fn play(&self) {
        let (loaded, has, pos) = {
            let st = self.st.borrow();
            (st.loaded, st.current.is_some(), st.position)
        };
        if !loaded {
            if has {
                self.load_current(pos, true);
            }
            return;
        }
        if let Some(mpv) = &self.mpv {
            mpv.set_flag("pause", false);
        }
    }

    pub fn pause(&self) {
        if let Some(mpv) = &self.mpv
            && self.st.borrow().loaded
        {
            mpv.set_flag("pause", true);
        }
    }

    pub fn next(&self) {
        let next = {
            let st = self.st.borrow();
            let cur = st.current;
            match cur {
                Some(c) if c + 1 < st.items.len() => Some(c + 1),
                Some(_) if st.repeat != Repeat::Off && !st.items.is_empty() => Some(0),
                _ => None,
            }
        };
        if let Some(i) = next {
            self.st.borrow_mut().current = Some(i);
            self.load_current(0.0, true);
        }
    }

    pub fn previous(&self) {
        let (pos, cur, repeat, len) = {
            let st = self.st.borrow();
            (st.position, st.current, st.repeat, st.items.len())
        };
        match cur {
            Some(c) if pos <= 3.0 && c > 0 => {
                self.st.borrow_mut().current = Some(c - 1);
                self.load_current(0.0, true);
            }
            Some(0) if pos <= 3.0 && repeat == Repeat::All && len > 1 => {
                self.st.borrow_mut().current = Some(len - 1);
                self.load_current(0.0, true);
            }
            Some(_) => self.seek(0.0),
            None => {}
        }
    }

    pub fn seek(&self, secs: f64) {
        let secs = secs.max(0.0);
        let loaded = self.st.borrow().loaded;
        if loaded && let Some(mpv) = &self.mpv {
            let _ = mpv.command(&["seek", &format!("{secs:.3}"), "absolute"]);
        }
        self.st.borrow_mut().position = secs;
        self.emit(&[Change::Position, Change::Seeked]);
    }

    /// Sets the volume (0-100). NaN and infinities (possible from D-Bus
    /// callers) are ignored: they would reach mpv and be saved as `null`,
    /// which makes the next start lose the queue.
    pub fn set_volume(&self, v: f64) {
        let Some(v) = sane_volume(v) else { return };
        if (self.st.borrow().volume - v).abs() < 0.01 {
            return;
        }
        self.st.borrow_mut().volume = v;
        if let Some(mpv) = &self.mpv {
            mpv.set_f64("volume", v);
        }
        self.emit(&[Change::Options]);
    }

    pub fn set_shuffle(&self, on: bool) {
        {
            let mut st = self.st.borrow_mut();
            if st.shuffle == on {
                return;
            }
            st.shuffle = on;
            let Some(cur) = st.current else {
                drop(st);
                self.emit(&[Change::Options]);
                return;
            };
            let current = st.items[cur].clone();
            let user = st.upcoming_user();
            let ctx: Vec<QueueItem> = st
                .context
                .iter()
                .map(|p| QueueItem {
                    path: p.clone(),
                    user: false,
                })
                .collect();
            if on {
                let mut rest = ctx;
                if let Some(i) = rest.iter().position(|i| i.path == current.path) {
                    rest.remove(i);
                }
                fastrand::shuffle(&mut rest);
                st.items = std::iter::once(current).chain(user).chain(rest).collect();
                st.current = Some(0);
            } else {
                match (!current.user)
                    .then(|| ctx.iter().position(|i| i.path == current.path))
                    .flatten()
                {
                    Some(p) => {
                        let mut items: Vec<QueueItem> = ctx[..=p].to_vec();
                        items.extend(user);
                        items.extend(ctx[p + 1..].iter().cloned());
                        st.items = items;
                        st.current = Some(p);
                    }
                    None => {
                        st.items = std::iter::once(current).chain(user).chain(ctx).collect();
                        st.current = Some(0);
                    }
                }
            }
        }
        self.sync_next();
        self.emit(&[Change::Options, Change::Queue]);
    }

    pub fn set_repeat(&self, r: Repeat) {
        self.st.borrow_mut().repeat = r;
        self.apply_loop();
        self.sync_next();
        self.emit(&[Change::Options, Change::Queue]);
    }

    pub fn cycle_repeat(&self) {
        self.set_repeat(match self.repeat() {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        });
    }

    /// Inserts tracks right after the current one.
    pub fn play_next(&self, paths: &[String]) {
        self.insert_user(paths, false);
    }

    /// Appends tracks after the other user-queued tracks.
    pub fn add_to_queue(&self, paths: &[String]) {
        self.insert_user(paths, true);
    }

    fn insert_user(&self, paths: &[String], after_queued: bool) {
        if paths.is_empty() {
            return;
        }
        let start_now = {
            let mut st = self.st.borrow_mut();
            let new: Vec<QueueItem> = paths
                .iter()
                .map(|p| QueueItem {
                    path: p.clone(),
                    user: true,
                })
                .collect();
            match st.current {
                None => {
                    st.items = new;
                    st.current = Some(0);
                    st.context.clear();
                    st.context_name = "Queue".into();
                    true
                }
                Some(cur) => {
                    let mut at = cur + 1;
                    if after_queued {
                        while at < st.items.len() && st.items[at].user {
                            at += 1;
                        }
                    }
                    st.items.splice(at..at, new);
                    false
                }
            }
        };
        if start_now {
            self.load_current(0.0, true);
        } else {
            self.sync_next();
            self.emit(&[Change::Queue]);
        }
    }

    /// Plays the queue item at `index`.
    pub fn jump_to(&self, index: usize) {
        if index < self.st.borrow().items.len() {
            self.st.borrow_mut().current = Some(index);
            self.load_current(0.0, true);
        }
    }

    pub fn remove_at(&self, index: usize) {
        {
            let mut st = self.st.borrow_mut();
            let Some(cur) = st.current else { return };
            if index == cur || index >= st.items.len() {
                return;
            }
            st.items.remove(index);
            if index < cur {
                st.current = Some(cur - 1);
            }
        }
        self.sync_next();
        self.emit(&[Change::Queue]);
    }

    /// Moves the queue item at `from` to position `to` (both after current).
    pub fn move_item(&self, from: usize, to: usize) {
        {
            let mut st = self.st.borrow_mut();
            let Some(cur) = st.current else { return };
            let len = st.items.len();
            if from <= cur || to <= cur || from >= len || to >= len || from == to {
                return;
            }
            let item = st.items.remove(from);
            st.items.insert(to, item);
        }
        self.sync_next();
        self.emit(&[Change::Queue]);
    }

    /// Drops deleted tracks from the queue; when the current one goes, plays
    /// on from the next track that is not deleted too.
    pub fn remove_paths(&self, paths: &[String]) {
        // Settle an advance mpv already made before queue indices change.
        self.catch_up();
        let (current_gone, loaded, play) = {
            let mut st = self.st.borrow_mut();
            let (loaded, play) = (st.loaded, st.playing);
            (st.remove_paths(paths), loaded, play)
        };
        if current_gone {
            if loaded && self.st.borrow().current.is_some() {
                self.load_current(0.0, play);
            } else {
                if loaded && let Some(mpv) = &self.mpv {
                    let _ = mpv.command(&["stop"]);
                }
                {
                    let mut files = self.files.borrow_mut();
                    files.retire_current();
                    files.retire_next();
                }
                let mut st = self.st.borrow_mut();
                st.loaded = false;
                st.playing = false;
                st.position = 0.0;
                st.current_entry = None;
                st.next_entry = None;
            }
        }
        self.update_tick();
        self.sync_next();
        self.emit(&[Change::Track, Change::Playing, Change::Queue]);
    }

    // ---- persistence -------------------------------------------------

    fn restore(&self) {
        let Ok(bytes) = std::fs::read(state_file()) else {
            return;
        };
        let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) else {
            return;
        };
        let mut st = self.st.borrow_mut();
        st.current = saved.current.filter(|c| *c < saved.items.len());
        st.items = saved.items;
        st.position = saved.position.max(0.0);
        st.shuffle = saved.shuffle;
        st.repeat = saved.repeat;
        st.volume = saved.volume.clamp(0.0, 100.0);
        st.context = saved.context;
        st.context_name = saved.context_name;
    }

    pub fn save(&self) {
        let saved = {
            let st = self.st.borrow();
            Saved {
                items: st.items.clone(),
                current: st.current,
                position: st.position,
                shuffle: st.shuffle,
                repeat: st.repeat,
                volume: st.volume,
                context: st.context.clone(),
                context_name: st.context_name.clone(),
            }
        };
        match serde_json::to_vec(&saved) {
            Ok(json) => {
                if let Err(e) = tunemesh_core::atomic::write(&state_file(), &json) {
                    eprintln!("tunemesh: saving state: {e}");
                }
            }
            Err(e) => eprintln!("tunemesh: saving state: {e}"),
        }
    }

    fn schedule_save(&self) {
        if self.save_scheduled.replace(true) {
            return;
        }
        let weak = self.weak_self();
        glib::timeout_add_local_once(Duration::from_secs(1), move || {
            if let Some(p) = weak.upgrade() {
                p.save_scheduled.set(false);
                p.save();
            }
        });
    }

    /// Saves state and stops mpv.
    pub fn shutdown(&self) {
        if let Some(mpv) = &self.mpv
            && let Some(pos) = mpv.get_f64("time-pos")
        {
            self.st.borrow_mut().position = pos;
        }
        self.save();
        if let Some(mpv) = &self.mpv {
            mpv.quit();
        }
    }
}

fn sane_volume(v: f64) -> Option<f64> {
    v.is_finite().then(|| v.clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_ignores_non_finite_values() {
        assert_eq!(sane_volume(f64::NAN), None);
        assert_eq!(sane_volume(f64::INFINITY), None);
        assert_eq!(sane_volume(f64::NEG_INFINITY), None);
        assert_eq!(sane_volume(250.0), Some(100.0));
        assert_eq!(sane_volume(-3.0), Some(0.0));
        assert_eq!(sane_volume(42.5), Some(42.5));
    }

    fn state(paths: &[&str], current: usize, repeat: Repeat) -> State {
        State {
            items: paths
                .iter()
                .map(|p| QueueItem {
                    path: (*p).to_owned(),
                    user: false,
                })
                .collect(),
            current: Some(current),
            context: paths.iter().map(|p| (*p).to_owned()).collect(),
            repeat,
            ..Default::default()
        }
    }

    fn paths(st: &State) -> Vec<&str> {
        st.items.iter().map(|i| i.path.as_str()).collect()
    }

    fn remove(st: &mut State, gone: &[&str]) -> bool {
        st.remove_paths(&gone.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn deleting_the_playing_album_skips_every_deleted_track() {
        // Playing a1 and deleting its whole album: playback continues at b1,
        // not at a2, which is about to be deleted too.
        let mut st = state(&["a1", "a2", "a3", "b1"], 0, Repeat::Off);
        assert!(remove(&mut st, &["a1", "a2", "a3"]));
        assert_eq!(paths(&st), vec!["b1"]);
        assert_eq!(st.current, Some(0));
        assert_eq!(st.context, vec!["b1"]);
    }

    #[test]
    fn current_track_kept_while_others_go() {
        let mut st = state(&["a", "b", "c", "d"], 2, Repeat::Off);
        assert!(!remove(&mut st, &["a", "d"]));
        assert_eq!(paths(&st), vec!["b", "c"]);
        assert_eq!(st.current, Some(1));
    }

    #[test]
    fn nothing_left_after_the_current_track() {
        let mut st = state(&["a", "b", "c"], 1, Repeat::Off);
        assert!(remove(&mut st, &["b", "c"]));
        assert_eq!(paths(&st), vec!["a"]);
        assert_eq!(st.current, None);

        let mut st = state(&["a", "b", "c"], 1, Repeat::All);
        assert!(remove(&mut st, &["b", "c"]));
        assert_eq!(st.current, Some(0));

        let mut st = state(&["a"], 0, Repeat::All);
        assert!(remove(&mut st, &["a"]));
        assert!(st.items.is_empty());
        assert_eq!(st.current, None);
    }

    /// Descriptors of this process that point at `path`.
    fn fds_on(path: &std::path::Path) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .filter(|e| std::fs::read_link(e.path()).is_ok_and(|t| t == path))
            .count()
    }

    #[test]
    fn failed_loads_do_not_pile_up_open_files() {
        let dir = std::env::temp_dir().join(format!("tunemesh-fd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.mp3");
        std::fs::write(&bad, "not audio").unwrap();
        let rel = "bad.mp3";
        let mut files = TrackFiles::default();
        // Each attempt: load_current retires the old files and opens the
        // track; mpv answers with end-file (error) and playback ends.
        for _ in 0..20 {
            files.retire_current();
            files.retire_next();
            files.current = Some(open_track(&dir, rel).unwrap().into_file());
            assert!(files.open_count() <= 1 + MAX_RETIRED);
            files.mpv_moved_on();
            files.release_all();
        }
        assert_eq!(files.open_count(), 0);
        assert_eq!(fds_on(&bad), 0);
        // Without any answer from mpv, the files stay bounded.
        for _ in 0..20 {
            files.retire_current();
            files.retire_next();
            files.current = Some(open_track(&dir, rel).unwrap().into_file());
            files.next = Some(open_track(&dir, rel).unwrap().into_file());
        }
        assert_eq!(files.open_count(), 2 + MAX_RETIRED);
        assert_eq!(fds_on(&bad), 2 + MAX_RETIRED);
        drop(files);
        assert_eq!(fds_on(&bad), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn upcoming_follows_the_queue_and_repeat() {
        let st = state(&["a", "b", "c", "d"], 1, Repeat::Off);
        assert_eq!(st.upcoming(), vec![2, 3]);
        let st = state(&["a", "b", "c", "d"], 1, Repeat::All);
        assert_eq!(st.upcoming(), vec![2, 3, 0, 1]);
        let st = state(&["a", "b"], 1, Repeat::Off);
        assert!(st.upcoming().is_empty());
    }
}
