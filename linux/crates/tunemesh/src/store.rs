//! The in-memory library and playlists, kept in sync with the folder:
//! cached scan on startup, background rescans, debounced file watching.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use gtk::glib;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tunemesh_core::fsx::Dir;
use tunemesh_core::merge::{self, Bases};
use tunemesh_core::playlist::{self, LIKED_SONGS, Summary, TrackInfo};
use tunemesh_core::{Library, PLAYLISTS_DIR, Playlist, TUNEMESH_DIR, Track, library};

use crate::config;

/// How long after the first file change a burst of changes is handled.
const DEBOUNCE: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreChange {
    /// Tracks, albums or artists changed.
    Library,
    /// A playlist (not Liked Songs) changed, was created, renamed or deleted.
    Playlists,
    Liked,
    /// Something below `.tunemesh/` changed.
    Downloads,
    Scanning,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    root: PathBuf,
    tracks: Vec<Track>,
}

#[derive(Serialize)]
struct CacheRef<'a> {
    root: &'a Path,
    tracks: &'a [Track],
}

/// What a library thread sends to the main loop.
enum Built {
    /// A new library to show (from the cache or a scan).
    Library(Arc<Library>),
    /// The scan is finished.
    Done,
}

fn cache_file() -> PathBuf {
    config::cache_dir().join("library.json")
}

type Listener = Rc<dyn Fn(StoreChange)>;

/// A playlist edit for the writer thread.
type WriteJob = Box<dyn FnOnce() + Send>;

/// What a merge of conflict copies reports: each playlist merged into with
/// its result, and whether copies are left (not due yet, or failed).
type Merged = (Vec<(String, io::Result<()>)>, bool);

/// The thread that reads, edits and writes playlist files, one job at a time.
fn writer_thread() -> std::sync::mpsc::Sender<WriteJob> {
    let (tx, rx) = std::sync::mpsc::channel::<WriteJob>();
    std::thread::Builder::new()
        .name("playlist-writer".into())
        .spawn(move || {
            for job in rx {
                job();
            }
        })
        .expect("spawn playlist writer");
    tx
}

/// How long quitting waits for queued playlist edits to reach the disk.
pub const FLUSH_WAIT: Duration = Duration::from_secs(5);

pub struct Store {
    me: Weak<Store>,
    root: RefCell<PathBuf>,
    /// Built and compared on library threads: the main loop only swaps it.
    lib: RefCell<Arc<Library>>,
    /// Playlists other than Liked Songs, as summaries: a long playlist is
    /// only held entry by entry while its page is open.
    playlists: RefCell<Vec<Summary>>,
    liked: RefCell<Playlist>,
    liked_set: RefCell<HashSet<String>>,
    /// Playlists over the contract's size limits (Liked Songs included):
    /// listed, never opened or rewritten.
    too_large: RefCell<HashSet<String>>,
    /// Size and modification time of each playlist file as last read.
    playlist_stamps: RefCell<HashMap<String, Stamp>>,
    playlists_loading: Cell<bool>,
    playlists_again: Cell<bool>,
    /// Counts playlist writes whose result was applied here: a reload that
    /// started before the latest one may hold older files, so it is
    /// dropped and done again.
    writes: Cell<u64>,
    /// Tests: makes the next reload's thread report that it has read the
    /// files, then wait to be released before it hands them over.
    #[cfg(test)]
    reload_pause: RefCell<Option<ReloadPause>>,
    listeners: RefCell<Vec<Listener>>,
    watcher: RefCell<Option<RecommendedWatcher>>,
    scanning: Cell<bool>,
    rescan_again: Cell<bool>,
    generation: Cell<u64>,
    writer: std::sync::mpsc::Sender<WriteJob>,
    /// This device's copies of the playlists as last seen, for merging
    /// Syncthing's conflict copies (outside the library, never synced).
    bases: RefCell<Bases>,
    /// This device's Syncthing device ID, once Syncthing told it: the
    /// conflict copies that name it are this device's to merge at once.
    device_id: Arc<Mutex<Option<String>>>,
    /// The next try at conflict copies that were not due yet.
    merge_retry: RefCell<Option<glib::SourceId>>,
}

impl Store {
    pub fn new(root: PathBuf) -> Rc<Self> {
        let store = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            bases: RefCell::new(Bases::in_dir(&config::state_dir(), &root)),
            root: RefCell::new(root),
            lib: RefCell::new(Arc::new(Library::default())),
            playlists: RefCell::new(Vec::new()),
            liked: RefCell::new(Playlist::new(LIKED_SONGS)),
            liked_set: RefCell::new(HashSet::new()),
            too_large: RefCell::new(HashSet::new()),
            playlist_stamps: RefCell::new(HashMap::new()),
            playlists_loading: Cell::new(false),
            playlists_again: Cell::new(false),
            writes: Cell::new(0),
            #[cfg(test)]
            reload_pause: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
            watcher: RefCell::new(None),
            scanning: Cell::new(false),
            rescan_again: Cell::new(false),
            generation: Cell::new(0),
            writer: writer_thread(),
            device_id: Arc::new(Mutex::new(None)),
            merge_retry: RefCell::new(None),
        });
        store.find_device_id();
        store.open();
        store
    }

    /// Asks Syncthing for this device's ID on a thread, then merges the
    /// conflict copies that name it. Tests never ask the developer's own
    /// Syncthing (`set_device_id`).
    fn find_device_id(&self) {
        #[cfg(not(test))]
        {
            let (tx, rx) = async_channel::bounded(1);
            std::thread::Builder::new()
                .name("syncthing-id".into())
                .spawn(move || {
                    let _ = tx.send_blocking(crate::syncthing::local_device_id());
                })
                .expect("spawn syncthing-id thread");
            let weak = self.me.clone();
            glib::spawn_future_local(async move {
                let Ok(Some(id)) = rx.recv().await else {
                    return;
                };
                let Some(store) = weak.upgrade() else { return };
                store.set_device_id(id);
            });
        }
    }

    /// Records this device's Syncthing ID and merges what it names.
    pub fn set_device_id(&self, id: String) {
        *self.device_id.lock().expect("device id") = Some(id);
        self.merge_conflicts();
    }

    /// Loads the cache and playlists for the current root, then watches and rescans.
    fn open(&self) {
        let root = self.root();
        // Syncthing must never sync TuneMesh's temporary files; add the
        // contract's rules to the library's .stignore (never through a
        // symlink, keeping the user's own rules). A missing folder is left
        // alone.
        if root.is_dir()
            && let Err(e) = tunemesh_core::stignore::ensure(&root)
        {
            eprintln!("tunemesh: cannot update {}/.stignore: {e}", root.display());
        }
        // The scan thread shows the cached library first, then the scanned one.
        *self.lib.borrow_mut() = Arc::new(Library::default());
        self.load_playlists();
        self.merge_conflicts();
        self.watch();
        self.rescan();
    }

    pub fn set_root(&self, root: PathBuf) {
        self.generation.set(self.generation.get() + 1);
        self.scanning.set(false);
        self.rescan_again.set(false);
        *self.bases.borrow_mut() = Bases::in_dir(&config::state_dir(), &root);
        *self.root.borrow_mut() = root;
        if let Some(id) = self.merge_retry.borrow_mut().take() {
            id.remove();
        }
        self.watcher.borrow_mut().take();
        self.playlists.borrow_mut().clear();
        self.too_large.borrow_mut().clear();
        self.playlist_stamps.borrow_mut().clear();
        self.set_liked(Playlist::new(LIKED_SONGS));
        self.open();
        self.emit(StoreChange::Library);
        self.emit(StoreChange::Playlists);
        self.emit(StoreChange::Liked);
        self.emit(StoreChange::Downloads);
    }

    pub fn root(&self) -> PathBuf {
        self.root.borrow().clone()
    }

    pub fn connect(&self, f: impl Fn(StoreChange) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn emit(&self, change: StoreChange) {
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for l in listeners {
            l(change);
        }
    }

    pub fn library(&self) -> Arc<Library> {
        self.lib.borrow().clone()
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.get()
    }

    /// Tests: runs the main loop until the library and playlist threads
    /// have reported.
    #[cfg(test)]
    fn settle(&self) {
        self.settle_for(Duration::from_secs(10));
    }

    /// Tests: `settle`, waiting up to `max`.
    #[cfg(test)]
    pub fn settle_for(&self, max: Duration) {
        let ctx = glib::MainContext::ref_thread_default();
        let start = std::time::Instant::now();
        while (self.scanning.get() || self.playlists_loading.get()) && start.elapsed() < max {
            ctx.iteration(true);
        }
    }

    /// Tests: starts a reload whose thread, once it has read the files,
    /// waits to hand them over. Returns a receiver that gets a message
    /// when the files are read and a sender that releases the reload.
    #[cfg(test)]
    pub fn start_paused_reload(
        &self,
    ) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
        let (read_tx, read_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        *self.reload_pause.borrow_mut() = Some((read_tx, go_rx));
        self.reload_playlists();
        (read_rx, go_tx)
    }

    /// Tests: stops watching the folder, so only the test starts reloads.
    #[cfg(test)]
    pub fn stop_watching(&self) {
        let ctx = glib::MainContext::ref_thread_default();
        let start = std::time::Instant::now();
        while self.watcher.borrow().is_none() && start.elapsed() < Duration::from_secs(10) {
            ctx.iteration(true);
        }
        self.watcher.borrow_mut().take();
    }

    // ---- scanning and watching ----------------------------------------

    pub fn rescan(&self) {
        if self.scanning.get() {
            self.rescan_again.set(true);
            return;
        }
        self.scanning.set(true);
        self.emit(StoreChange::Scanning);
        let root = self.root();
        let previous = self.library();
        let (tx, rx) = async_channel::unbounded::<Built>();
        std::thread::Builder::new()
            .name("library-scan".into())
            .spawn(move || {
                // Everything O(library) happens here, never on the main loop.
                let mut previous = previous;
                if previous.tracks.is_empty() {
                    let cached = fs::read(cache_file())
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
                        .filter(|c| c.root == root)
                        .map(|c| c.tracks)
                        .unwrap_or_default();
                    if !cached.is_empty() {
                        previous = Arc::new(Library::new(cached));
                        let _ = tx.send_blocking(Built::Library(previous.clone()));
                    }
                }
                let by_path: HashMap<String, Track> = previous
                    .tracks
                    .iter()
                    .map(|t| (t.path.clone(), t.clone()))
                    .collect();
                let tracks = library::scan(&root, &by_path);
                drop(by_path);
                if tracks != previous.tracks {
                    let cache = CacheRef {
                        root: &root,
                        tracks: &tracks,
                    };
                    if let Ok(json) = serde_json::to_vec(&cache) {
                        let _ = tunemesh_core::atomic::write(&cache_file(), &json);
                    }
                    let _ = tx.send_blocking(Built::Library(Arc::new(Library::new(tracks))));
                }
                let _ = tx.send_blocking(Built::Done);
            })
            .expect("spawn scan thread");
        let weak = self.me.clone();
        let generation = self.generation.get();
        glib::spawn_future_local(async move {
            while let Ok(built) = rx.recv().await {
                let Some(store) = weak.upgrade() else { return };
                if store.generation.get() != generation {
                    return;
                }
                match built {
                    Built::Library(lib) => {
                        *store.lib.borrow_mut() = lib;
                        store.emit(StoreChange::Library);
                    }
                    Built::Done => {
                        store.scanning.set(false);
                        store.emit(StoreChange::Scanning);
                        if store.rescan_again.replace(false) {
                            store.rescan();
                        }
                    }
                }
            }
        });
    }

    fn watch(&self) {
        let root = self.root();
        if !root.is_dir() {
            return;
        }
        let (tx, rx) = async_channel::unbounded::<Vec<PathBuf>>();
        let generation = self.generation.get();
        // Watching adds an inotify watch per folder: a second or more for a
        // library of thousands of albums, so it happens on a thread.
        let (wtx, wrx) = async_channel::bounded::<notify::Result<RecommendedWatcher>>(1);
        let watch_root = root.clone();
        std::thread::Builder::new()
            .name("library-watch".into())
            .spawn(move || {
                let _ = wtx.send_blocking(watch_library(&watch_root, move |paths| {
                    let _ = tx.send_blocking(paths);
                }));
            })
            .expect("spawn watch thread");
        let weak = self.me.clone();
        let watch_root = root.clone();
        glib::spawn_future_local(async move {
            let Ok(res) = wrx.recv().await else { return };
            let Some(store) = weak.upgrade() else { return };
            // A watcher for a folder no longer shown is dropped here.
            if store.generation.get() != generation {
                return;
            }
            match res {
                Ok(w) => *store.watcher.borrow_mut() = Some(w),
                Err(e) => eprintln!("tunemesh: cannot watch {}: {e}", watch_root.display()),
            }
        });
        let weak = self.me.clone();
        glib::spawn_future_local(async move {
            // Changes arrive in bursts (a sync, a download): handle each burst
            // once, DEBOUNCE after its first event.
            while let Ok(first) = rx.recv().await {
                glib::timeout_future(DEBOUNCE).await;
                let mut paths: HashSet<PathBuf> = first.into_iter().collect();
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                let Some(store) = weak.upgrade() else { break };
                if store.generation.get() != generation {
                    break;
                }
                store.on_fs_events(&root, &paths.into_iter().collect::<Vec<_>>());
            }
        });
    }

    fn on_fs_events(&self, root: &Path, paths: &[PathBuf]) {
        let (mut tracks, mut playlists, mut downloads) = (false, false, false);
        for p in paths {
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let mut comps = rel.iter().filter_map(|c| c.to_str());
            match comps.next() {
                Some(PLAYLISTS_DIR) => playlists = true,
                Some(TUNEMESH_DIR) => downloads = true,
                Some(first) => {
                    let hidden = first.starts_with('.')
                        || rel.iter().any(|c| c.to_string_lossy().starts_with('.'));
                    if !hidden {
                        tracks = true;
                    }
                }
                None => tracks = true,
            }
        }
        if playlists {
            self.merge_conflicts();
            self.reload_playlists();
        }
        if downloads {
            self.emit(StoreChange::Downloads);
        }
        if tracks {
            self.rescan();
        }
    }

    // ---- playlists ---------------------------------------------------

    fn load_playlists(&self) {
        self.reload_playlists();
    }

    fn set_liked(&self, liked: Playlist) {
        *self.liked_set.borrow_mut() = liked_paths(&liked);
        *self.liked.borrow_mut() = liked;
    }

    /// Re-reads the playlists on a thread: only files whose size or
    /// modification time changed are read again, and nothing proportional to
    /// a playlist's length runs on the main loop.
    fn reload_playlists(&self) {
        if self.playlists_loading.replace(true) {
            self.playlists_again.set(true);
            return;
        }
        let root = self.root();
        let known = PlaylistsRead {
            lists: self.playlists.borrow().clone(),
            liked: None,
            too_large: self.too_large.borrow().clone(),
            stamps: self.playlist_stamps.borrow().clone(),
        };
        #[cfg(test)]
        let pause = self.reload_pause.borrow_mut().take();
        let (tx, rx) = async_channel::bounded::<PlaylistsRead>(1);
        std::thread::Builder::new()
            .name("playlists".into())
            .spawn(move || {
                let read = read_playlists(&root, known);
                #[cfg(test)]
                if let Some((read_done, release)) = pause {
                    let _ = read_done.send(());
                    let _ = release.recv();
                }
                let _ = tx.send_blocking(read);
            })
            .expect("spawn playlist thread");
        let weak = self.me.clone();
        let (generation, writes) = (self.generation.get(), self.writes.get());
        glib::spawn_future_local(async move {
            let read = rx.recv().await;
            let Some(store) = weak.upgrade() else { return };
            store.playlists_loading.set(false);
            if store.generation.get() != generation || store.writes.get() != writes {
                // The library folder changed, or an edit was applied
                // meanwhile: what was read may be older than what is shown.
                store.reload_playlists();
                return;
            }
            if let Ok(read) = read {
                // A summary does not show every change (a reorder further
                // down), so any change refreshes what shows playlists.
                *store.playlists.borrow_mut() = read.lists;
                *store.too_large.borrow_mut() = read.too_large;
                *store.playlist_stamps.borrow_mut() = read.stamps;
                store.emit(StoreChange::Playlists);
                if let Some((liked, set)) = read.liked {
                    *store.liked_set.borrow_mut() = set;
                    *store.liked.borrow_mut() = liked;
                    store.emit(StoreChange::Liked);
                }
            }
            if store.playlists_again.replace(false) {
                store.reload_playlists();
            }
        });
    }

    /// Records that a write to these playlist files was applied: running
    /// reloads are redone, and the files' stamps are forgotten so the next
    /// reload reads them again rather than trust a summary of an older
    /// version.
    fn wrote(&self, names: &[&str]) {
        self.writes.set(self.writes.get() + 1);
        let mut stamps = self.playlist_stamps.borrow_mut();
        for name in names {
            stamps.remove(*name);
        }
    }

    /// Merges Syncthing's conflict copies of playlists into the playlists
    /// (contract: "Playlist conflict copies") on the writer thread, then
    /// shows the result. While copies are left that were not due yet, it
    /// tries again a minute later.
    fn merge_conflicts(&self) {
        let (root, bases) = (self.root(), self.bases.borrow().clone());
        let device_id = self.device_id.clone();
        let (weak, generation) = (self.me.clone(), self.generation.get());
        self.write_job(
            move || {
                let id = device_id.lock().expect("device id").clone();
                let merged =
                    merge::merge_conflicts(&root, &bases, id.as_deref(), SystemTime::now());
                Ok((merged, has_conflict_copies(&root)))
            },
            move |result: io::Result<Merged>| {
                let Some(store) = weak.upgrade() else { return };
                let Ok((merged, left)) = result else { return };
                if store.generation.get() != generation {
                    return;
                }
                let mut names = Vec::new();
                for (name, r) in &merged {
                    match r {
                        Ok(()) => names.push(name.as_str()),
                        Err(e) => eprintln!(
                            "tunemesh: cannot merge the conflict copies of playlist {name:?}: {e}"
                        ),
                    }
                }
                if !names.is_empty() {
                    store.wrote(&names);
                    store.reload_playlists();
                }
                if let Some(id) = store.merge_retry.borrow_mut().take() {
                    id.remove();
                }
                if left {
                    let weak = store.me.clone();
                    let id = glib::timeout_add_seconds_local_once(60, move || {
                        if let Some(store) = weak.upgrade() {
                            store.merge_retry.borrow_mut().take();
                            store.merge_conflicts();
                        }
                    });
                    *store.merge_retry.borrow_mut() = Some(id);
                }
            },
        );
    }

    /// Waits up to `max` until every playlist edit queued so far is on
    /// disk; false when that took longer. Called before quitting: the
    /// writer thread does not outlive the process.
    pub fn flush(&self, max: Duration) -> bool {
        let (tx, rx) = std::sync::mpsc::channel();
        let job: WriteJob = Box::new(move || {
            let _ = tx.send(());
        });
        if self.writer.send(job).is_err() {
            return true;
        }
        let flushed = rx.recv_timeout(max).is_ok();
        if !flushed {
            eprintln!("tunemesh: playlist edits were still being written after {max:?}");
        }
        flushed
    }

    /// All playlists except Liked Songs, sorted by name.
    pub fn playlists(&self) -> Vec<Summary> {
        self.playlists.borrow().clone()
    }

    /// Playlists songs can be added to: too large ones are left out.
    pub fn playlist_names(&self) -> Vec<String> {
        let too_large = self.too_large.borrow();
        self.playlists
            .borrow()
            .iter()
            .filter(|p| !too_large.contains(&p.name))
            .map(|p| p.name.clone())
            .collect()
    }

    /// Whether a playlist (other than Liked Songs) of this name is listed.
    pub fn has_playlist(&self, name: &str) -> bool {
        self.playlists.borrow().iter().any(|p| p.name == name)
    }

    /// Whether the playlist is over the size limits: it is listed as "Too
    /// large to open", and nothing opens, plays or rewrites it.
    pub fn is_too_large(&self, name: &str) -> bool {
        self.too_large.borrow().contains(name)
    }

    pub fn liked(&self) -> Playlist {
        self.liked.borrow().clone()
    }

    pub fn liked_summary(&self) -> Summary {
        summarize(&self.liked.borrow())
    }

    pub fn liked_count(&self) -> usize {
        self.liked.borrow().entries.len()
    }

    /// A playlist with all its entries, read from its file now.
    pub fn playlist(&self, name: &str) -> Option<Playlist> {
        if self.is_too_large(name) {
            return None;
        }
        if name == LIKED_SONGS {
            return Some(self.liked());
        }
        if !self.has_playlist(name) {
            return None;
        }
        Playlist::read(&self.root(), name)
            .inspect_err(|e| eprintln!("tunemesh: cannot read playlist {name:?}: {e}"))
            .ok()
    }

    pub fn is_liked(&self, path: &str) -> bool {
        self.liked_set.borrow().contains(path)
    }

    fn track_info(&self) -> impl Fn(&str) -> Option<TrackInfo> + Send + use<> {
        let lib = self.library();
        move |path: &str| {
            lib.track(path).map(|t| TrackInfo {
                duration_secs: t.duration_secs(),
                label: format!("{} - {}", t.artist, t.title),
            })
        }
    }

    /// Runs `work` on the playlist writer thread, then `then` with its
    /// result on the main loop. Jobs run one at a time, in order, so two
    /// edits of one playlist never read the same old file.
    fn write_job<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> io::Result<T> + Send + 'static,
        then: impl FnOnce(io::Result<T>) + 'static,
    ) {
        let (tx, rx) = async_channel::bounded(1);
        let job: WriteJob = Box::new(move || {
            let _ = tx.send_blocking(work());
        });
        if self.writer.send(job).is_err() {
            then(Err(io::Error::other("the playlist writer stopped")));
            return;
        }
        glib::spawn_future_local(async move {
            if let Ok(result) = rx.recv().await {
                then(result);
            }
        });
    }

    /// Likes or unlikes `paths`; `done` runs once Liked Songs is written.
    pub fn set_liked_paths(
        &self,
        paths: &[String],
        like: bool,
        done: impl FnOnce(io::Result<()>) + 'static,
    ) {
        let paths = paths.to_vec();
        self.modify_playlist(
            LIKED_SONGS,
            move |liked| {
                if like {
                    liked.like_all(paths.iter().map(String::as_str));
                } else {
                    liked.unlike_all(paths.iter().map(String::as_str));
                }
                Ok(())
            },
            done,
        );
    }

    pub fn toggle_like(&self, path: &str, done: impl FnOnce(io::Result<()>) + 'static) {
        let like = !self.is_liked(path);
        self.set_liked_paths(&[path.to_owned()], like, done);
    }

    /// Applies `f` to a freshly read playlist and writes it back, off the
    /// main loop: a playlist may hold 250,000 entries, and every write
    /// waits for the disk. `done` gets the result on the main loop.
    fn modify_playlist(
        &self,
        name: &str,
        f: impl FnOnce(&mut Playlist) -> io::Result<()> + Send + 'static,
        done: impl FnOnce(io::Result<()>) + 'static,
    ) {
        let (root, n, info) = (self.root(), name.to_owned(), self.track_info());
        let bases = self.bases.borrow().clone();
        let (weak, generation, name) = (self.me.clone(), self.generation.get(), name.to_owned());
        self.write_job(
            move || {
                // Re-read under the `Playlists/` lock so changes made on
                // other devices, and by a worker on this machine, are kept.
                Playlist::edit(&root, &bases, &n, &info, f)
            },
            move |result| {
                let p = match result {
                    Ok(p) => p,
                    Err(e) => return done(Err(e)),
                };
                if let Some(store) = weak.upgrade()
                    && store.generation.get() == generation
                {
                    store.wrote(&[&name]);
                    if name == LIKED_SONGS {
                        store.set_liked(p);
                        store.emit(StoreChange::Liked);
                    } else {
                        let mut lists = store.playlists.borrow_mut();
                        match lists.iter_mut().find(|x| x.name == name) {
                            Some(x) => *x = summarize(&p),
                            None => lists.push(summarize(&p)),
                        }
                        drop(lists);
                        store.emit(StoreChange::Playlists);
                    }
                }
                done(Ok(()));
            },
        );
    }

    /// Creates a playlist on the writer thread (the file is synced to disk);
    /// `done` gets the sanitised name.
    pub fn create_playlist(&self, name: &str, done: impl FnOnce(io::Result<String>) + 'static) {
        let (root, n) = (self.root(), name.to_owned());
        let (weak, generation) = (self.me.clone(), self.generation.get());
        self.write_job(
            move || Playlist::create(&root, &n),
            move |result| {
                let p = match result {
                    Ok(p) => p,
                    Err(e) => return done(Err(e)),
                };
                if let Some(store) = weak.upgrade()
                    && store.generation.get() == generation
                {
                    store.wrote(&[&p.name]);
                    let mut lists = store.playlists.borrow_mut();
                    lists.push(summarize(&p));
                    lists.sort_by_cached_key(|p| tunemesh_core::search::fold(&p.name));
                    drop(lists);
                    store.emit(StoreChange::Playlists);
                }
                done(Ok(p.name));
            },
        );
    }

    /// Renames a playlist on the writer thread; `done` gets the new name.
    pub fn rename_playlist(
        &self,
        old: &str,
        new: &str,
        done: impl FnOnce(io::Result<String>) + 'static,
    ) {
        if self.is_too_large(old) {
            return done(Err(io::Error::other(
                "This playlist is too large to change.",
            )));
        }
        let (root, o, n) = (self.root(), old.to_owned(), new.to_owned());
        let bases = self.bases.borrow().clone();
        let (weak, generation, old) = (self.me.clone(), self.generation.get(), old.to_owned());
        self.write_job(
            move || Playlist::rename(&root, &bases, &o, &n),
            move |result| {
                let new = match result {
                    Ok(new) => new,
                    Err(e) => return done(Err(e)),
                };
                if let Some(store) = weak.upgrade()
                    && store.generation.get() == generation
                {
                    store.wrote(&[&old, &new]);
                    let mut lists = store.playlists.borrow_mut();
                    if let Some(p) = lists.iter_mut().find(|p| p.name == old) {
                        p.name = new.clone();
                    }
                    lists.sort_by_cached_key(|p| tunemesh_core::search::fold(&p.name));
                    drop(lists);
                    store.emit(StoreChange::Playlists);
                }
                done(Ok(new));
            },
        );
    }

    /// Deletes a playlist file on the writer thread.
    pub fn delete_playlist(&self, name: &str, done: impl FnOnce(io::Result<()>) + 'static) {
        let (root, n) = (self.root(), name.to_owned());
        let bases = self.bases.borrow().clone();
        let (weak, generation, name) = (self.me.clone(), self.generation.get(), name.to_owned());
        self.write_job(
            move || Playlist::delete(&root, &bases, &n),
            move |result| {
                if result.is_ok()
                    && let Some(store) = weak.upgrade()
                    && store.generation.get() == generation
                {
                    store.wrote(&[&name]);
                    store.playlists.borrow_mut().retain(|p| p.name != name);
                    store.emit(StoreChange::Playlists);
                }
                done(result);
            },
        );
    }

    pub fn add_to_playlist(
        &self,
        name: &str,
        paths: &[String],
        done: impl FnOnce(io::Result<()>) + 'static,
    ) {
        if name == LIKED_SONGS {
            return self.set_liked_paths(paths, true, done);
        }
        let paths = paths.to_vec();
        self.modify_playlist(
            name,
            move |p| {
                p.append(paths.iter().map(String::as_str));
                Ok(())
            },
            done,
        );
    }

    /// Removes the entries the user saw at these positions. The file is read
    /// again first, so entries another device moved are found by their path.
    pub fn remove_from_playlist(
        &self,
        name: &str,
        refs: &[EntryRef],
        done: impl FnOnce(io::Result<()>) + 'static,
    ) {
        let refs = refs.to_vec();
        self.modify_playlist(
            name,
            move |p| {
                let positions = locate(p, &refs)?;
                p.remove_positions(&positions);
                Ok(())
            },
            done,
        );
    }

    /// Moves the entry the user dragged to where the drop target is now.
    pub fn move_in_playlist(
        &self,
        name: &str,
        from: &EntryRef,
        to: &EntryRef,
        done: impl FnOnce(io::Result<()>) + 'static,
    ) {
        let refs = [from.clone(), to.clone()];
        self.modify_playlist(
            name,
            move |p| {
                let found = locate(p, &refs)?;
                p.move_entry(found[0], found[1]);
                Ok(())
            },
            done,
        );
    }

    // ---- tracks ------------------------------------------------------

    /// Deletes track files (this syncs to every device) and empty parent
    /// directories, then drops them from the in-memory library.
    pub fn delete_tracks(&self, paths: &[String]) -> io::Result<()> {
        let (gone, result) = delete_files(&self.root(), paths);
        if gone.is_empty() {
            return result;
        }
        // Only tracks whose file is really gone leave the library; one that
        // could not be deleted stays listed. The new library is built off
        // the main loop.
        let previous = self.library();
        let (tx, rx) = async_channel::bounded::<Arc<Library>>(1);
        std::thread::Builder::new()
            .name("library-update".into())
            .spawn(move || {
                let gone: HashSet<String> = gone.into_iter().collect();
                let remaining: Vec<Track> = previous
                    .tracks
                    .iter()
                    .filter(|t| !gone.contains(&t.path))
                    .cloned()
                    .collect();
                let _ = tx.send_blocking(Arc::new(Library::new(remaining)));
            })
            .expect("spawn library thread");
        let weak = self.me.clone();
        let generation = self.generation.get();
        glib::spawn_future_local(async move {
            let Ok(lib) = rx.recv().await else { return };
            let Some(store) = weak.upgrade() else { return };
            if store.generation.get() == generation {
                *store.lib.borrow_mut() = lib;
                store.emit(StoreChange::Library);
            }
        });
        result
    }
}

/// Whether `Playlists/` holds a Syncthing conflict copy of a playlist.
/// Reads names only, never through a symlinked folder.
fn has_conflict_copies(root: &Path) -> bool {
    Dir::open(root)
        .and_then(|d| d.sub(PLAYLISTS_DIR, false))
        .and_then(|d| d.entries())
        .is_ok_and(|names| names.iter().any(|(n, _)| merge::conflict_copy(n).is_some()))
}

/// Deletes library files and their emptied parent directories. Paths are
/// opened below `root` without following symlinks, so a synced symlinked
/// folder cannot redirect a delete outside the library.
fn delete_files(root: &Path, paths: &[String]) -> (Vec<String>, io::Result<()>) {
    let top = match Dir::open(root) {
        Ok(d) => d,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let mut gone = Vec::new();
    let mut result = Ok(());
    for rel in paths {
        let (parent, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let removed = if parent.is_empty() {
            top.try_clone()
        } else {
            top.sub(parent, false)
        }
        .and_then(|d| d.remove_file(name));
        if let Err(e) = removed
            && e.kind() != io::ErrorKind::NotFound
        {
            result = Err(e);
            continue;
        }
        gone.push(rel.clone());
        // Emptied parent folders go too (never through a symlink).
        let mut dir = parent;
        while !dir.is_empty() {
            let (up, name) = dir.rsplit_once('/').unwrap_or(("", dir));
            let removed = if up.is_empty() {
                top.try_clone()
            } else {
                top.sub(up, false)
            }
            .and_then(|d| d.remove_dir(name));
            if removed.is_err() {
                break;
            }
            dir = up;
        }
    }
    (gone, result)
}

/// What identifies a version of a playlist file without reading it.
type Stamp = (u64, std::time::SystemTime);

/// Tests: the reload thread sends on the first when it has read the files
/// and waits on the second before it hands them over.
#[cfg(test)]
type ReloadPause = (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);

fn stamp(root: &Path, name: &str) -> Option<Stamp> {
    let meta = fs::symlink_metadata(playlist::playlist_path(root, name)).ok()?;
    meta.is_file()
        .then(|| meta.modified().ok().map(|m| (meta.len(), m)))?
}

fn liked_paths(liked: &Playlist) -> HashSet<String> {
    liked
        .entries
        .iter()
        .filter_map(|e| e.track().map(str::to_owned))
        .collect()
}

/// The playlists as read by `read_playlists`.
struct PlaylistsRead {
    lists: Vec<Summary>,
    /// Liked Songs and its track set, when it changed.
    liked: Option<(Playlist, HashSet<String>)>,
    too_large: HashSet<String>,
    stamps: HashMap<String, Stamp>,
}

/// Reads the playlist summaries and Liked Songs below `root`, reusing what
/// `known` holds for files that did not change. Runs off the main loop.
fn read_playlists(root: &Path, known: PlaylistsRead) -> PlaylistsRead {
    let mut out = PlaylistsRead {
        lists: Vec::new(),
        liked: None,
        too_large: HashSet::new(),
        stamps: HashMap::new(),
    };
    let unchanged =
        |name: &str, now: Option<Stamp>| now.is_some() && known.stamps.get(name).copied() == now;
    let summaries: HashMap<&str, &Summary> =
        known.lists.iter().map(|s| (s.name.as_str(), s)).collect();
    for name in playlist::list_names(root) {
        let now = stamp(root, &name);
        if unchanged(&name, now)
            && let Some(s) = summaries.get(name.as_str())
        {
            if known.too_large.contains(&name) {
                out.too_large.insert(name.clone());
            }
            out.lists.push((*s).clone());
            out.stamps.insert(name, now.expect("checked"));
            continue;
        }
        match Playlist::summary(root, &name, SUMMARY_TRACKS) {
            Ok(s) => out.lists.push(s),
            Err(e) if playlist::is_too_large(&e) => {
                out.lists.push(Summary {
                    name: name.clone(),
                    count: 0,
                    first_tracks: Vec::new(),
                });
                out.too_large.insert(name.clone());
            }
            Err(e) => {
                eprintln!("tunemesh: cannot read playlist {name:?}: {e}");
                continue;
            }
        }
        if let Some(now) = now {
            out.stamps.insert(name, now);
        }
    }
    let now = stamp(root, LIKED_SONGS);
    if unchanged(LIKED_SONGS, now) {
        if known.too_large.contains(LIKED_SONGS) {
            out.too_large.insert(LIKED_SONGS.to_owned());
        }
    } else {
        let liked = match Playlist::read(root, LIKED_SONGS) {
            Ok(p) => p,
            Err(e) => {
                if playlist::is_too_large(&e) {
                    out.too_large.insert(LIKED_SONGS.to_owned());
                } else if e.kind() != io::ErrorKind::NotFound {
                    eprintln!("tunemesh: cannot read Liked Songs: {e}");
                }
                Playlist::new(LIKED_SONGS)
            }
        };
        let set = liked_paths(&liked);
        out.liked = Some((liked, set));
    }
    if let Some(now) = now {
        out.stamps.insert(LIKED_SONGS.to_owned(), now);
    }
    out
}

/// Tracks a playlist summary keeps for its cover.
const SUMMARY_TRACKS: usize = 8;

fn summarize(p: &Playlist) -> Summary {
    Summary {
        name: p.name.clone(),
        count: p.entries.len(),
        first_tracks: p
            .entries
            .iter()
            .filter_map(|e| e.track().map(str::to_owned))
            .take(SUMMARY_TRACKS)
            .collect(),
    }
}

/// A playlist entry as shown: its position and its identity (the track path,
/// or the line as written when it points outside the library).
pub type EntryRef = (usize, String);

pub fn entry_key<'a>(e: tunemesh_core::playlist::PlaylistEntry<'a>) -> &'a str {
    e.key()
}

/// Finds the shown entries in a freshly read playlist: at their position if
/// it still holds the same entry, else the first unclaimed entry with that
/// identity. Fails when one is gone, so nothing wrong is changed.
fn locate(p: &Playlist, refs: &[EntryRef]) -> io::Result<Vec<usize>> {
    let changed = || {
        io::Error::other("The playlist changed on another device. Nothing was changed; try again.")
    };
    let mut found: Vec<usize> = Vec::with_capacity(refs.len());
    let mut taken: HashSet<usize> = HashSet::with_capacity(refs.len());
    // Positions of each identity, built once, only when an entry moved.
    let mut by_key: Option<HashMap<&str, std::collections::VecDeque<usize>>> = None;
    for (pos, key) in refs {
        let in_place = p.entries.get(*pos).is_some_and(|e| entry_key(e) == key);
        let at = if in_place && !taken.contains(pos) {
            Some(*pos)
        } else {
            let index = by_key.get_or_insert_with(|| {
                let mut m: HashMap<&str, std::collections::VecDeque<usize>> = HashMap::new();
                for (i, e) in p.entries.iter().enumerate() {
                    m.entry(entry_key(e)).or_default().push_back(i);
                }
                m
            });
            index.get_mut(key.as_str()).and_then(|q| {
                while let Some(&i) = q.front() {
                    if taken.contains(&i) {
                        q.pop_front();
                    } else {
                        return Some(i);
                    }
                }
                None
            })
        };
        let i = at.ok_or_else(changed)?;
        taken.insert(i);
        found.push(i);
    }
    Ok(found)
}

/// Watches `root` recursively and passes the paths of every change that is
/// not a read to `on_change`. Symlinks are never followed: a synced link
/// must not make the app watch (and walk, on the main thread) folders
/// outside the library, or loop through a "symlink bomb".
fn watch_library(
    root: &Path,
    on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            // Reads (open/close-nowrite) are ignored: the app's own rescans
            // and playlist reads would otherwise trigger the next rescan,
            // forever.
            if let Ok(ev) = res
                && !matches!(ev.kind, EventKind::Access(_))
            {
                on_change(ev.paths);
            }
        },
        notify::Config::default().with_follow_symlinks(false),
    )?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// Tests: starts a background playlist edit with its `done` callback and
/// runs the main loop until it reports.
#[cfg(test)]
pub fn wait_for<T: 'static>(start: impl FnOnce(Box<dyn FnOnce(io::Result<T>)>)) -> io::Result<T> {
    let out: Rc<RefCell<Option<io::Result<T>>>> = Rc::default();
    let o = out.clone();
    start(Box::new(move |r| *o.borrow_mut() = Some(r)));
    let ctx = glib::MainContext::ref_thread_default();
    while out.borrow().is_none() {
        ctx.iteration(true);
    }
    out.take().expect("result")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `f` with a main context of its own as the thread default, so
    /// the futures a `Store` spawns never meet another test's thread.
    fn on_own_context(f: impl FnOnce()) {
        let ctx = glib::MainContext::new();
        let _owner = ctx.acquire().expect("main context");
        ctx.with_thread_default(f).expect("thread default");
    }

    #[test]
    fn deleting_never_follows_a_symlinked_folder() {
        let base =
            std::env::temp_dir().join(format!("tunemesh-delete-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        fs::create_dir_all(root.join("Artist/Album")).unwrap();
        fs::create_dir_all(outside.join("Album")).unwrap();
        fs::write(root.join("Artist/Album/Song [a].opus"), b"x").unwrap();
        fs::write(outside.join("Album/Keep [b].opus"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Linked")).unwrap();

        let (gone, r) = delete_files(
            &root,
            &[
                "Linked/Album/Keep [b].opus".into(),
                "Artist/Album/Song [a].opus".into(),
            ],
        );
        assert!(r.is_err(), "the symlinked path must be refused");
        assert_eq!(
            gone,
            ["Artist/Album/Song [a].opus"],
            "only the real track is gone"
        );
        assert!(outside.join("Album/Keep [b].opus").exists());
        assert!(
            !root.join("Artist").exists(),
            "the real track and its empty folders are gone"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn opening_a_library_adds_missing_stignore_rules() {
        on_own_context(|| {
            let base =
                std::env::temp_dir().join(format!("tunemesh-stignore-test-{}", std::process::id()));
            let (lib, other) = (base.join("lib"), base.join("other"));
            fs::create_dir_all(&lib).unwrap();
            fs::create_dir_all(&other).unwrap();
            fs::write(lib.join(".stignore"), "// mine\n*.tmp\n").unwrap();
            let store = Store::new(lib.clone());
            store.settle();
            let text = fs::read_to_string(lib.join(".stignore")).unwrap();
            assert!(
                text.starts_with("// mine\n*.tmp\n"),
                "user rules kept: {text:?}"
            );
            assert!(
                text.lines().any(|l| l.trim() == "(?d).~tunemesh-*"),
                "{text:?}"
            );

            // A symlinked .stignore is neither followed nor replaced.
            fs::write(other.join("target"), "keep\n").unwrap();
            std::os::unix::fs::symlink(other.join("target"), other.join(".stignore")).unwrap();
            store.set_root(other.clone());
            store.settle();
            assert_eq!(fs::read_to_string(other.join("target")).unwrap(), "keep\n");
            assert!(
                fs::symlink_metadata(other.join(".stignore"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    /// This process's inotify file descriptors.
    fn inotify_fds() -> Vec<String> {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .filter(|e| {
                fs::read_link(e.path()).is_ok_and(|l| l.to_string_lossy().contains("inotify"))
            })
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn watches(fd: &str) -> usize {
        fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
            .map(|s| s.lines().filter(|l| l.starts_with("inotify wd:")).count())
            .unwrap_or(0)
    }

    #[test]
    fn the_watcher_never_follows_symlinks() {
        let base = std::env::temp_dir().join(format!("tunemesh-watch-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        fs::create_dir_all(root.join("Artist/Album")).unwrap();
        for i in 0..300 {
            fs::create_dir_all(outside.join(format!("d{i}"))).unwrap();
        }
        std::os::unix::fs::symlink(&outside, root.join("Linked")).unwrap();
        // A symlink bomb: 7 levels, each holding 60 links to the next.
        for level in 0..7 {
            let dir = root.join(format!(".b/B{level}"));
            fs::create_dir_all(&dir).unwrap();
            if level < 6 {
                for n in 0..60 {
                    std::os::unix::fs::symlink(
                        format!("../B{}", level + 1),
                        dir.join(format!("l{n}")),
                    )
                    .unwrap();
                }
            }
        }

        let before = inotify_fds();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        let watcher = watch_library(&root, move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        let took = started.elapsed();
        // Other tests' stores start watchers on threads at any time: count
        // only the inotify instance that watches this test's library.
        let root_ino = format!(
            " ino:{:x} ",
            std::os::unix::fs::MetadataExt::ino(&fs::metadata(&root).unwrap())
        );
        let count: usize = inotify_fds()
            .iter()
            .filter(|fd| !before.contains(fd))
            .filter(|fd| {
                fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
                    .is_ok_and(|s| s.contains(&root_ino))
            })
            .map(|fd| watches(fd))
            .sum();

        // lib, Artist, Album, .b and its 7 levels.
        assert!(
            count <= 11,
            "{count} inotify watches: symlinks were followed"
        );
        assert!(took < Duration::from_secs(1), "watching took {took:?}");

        // A change outside the library is not seen; one inside is.
        fs::write(outside.join("d1/new"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            rx.try_recv().is_err(),
            "a change outside the library was reported"
        );
        fs::write(root.join("Artist/Album/new"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        drop(watcher);
        fs::remove_dir_all(&base).unwrap();
    }

    fn playlist_lib(tag: &str, lines: &[&str]) -> (PathBuf, Rc<Store>, Vec<EntryRef>) {
        let base = std::env::temp_dir().join(format!("tunemesh-pl-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join(PLAYLISTS_DIR)).unwrap();
        write_list(&base, lines);
        let store = Store::new(base.clone());
        store.settle();
        let shown = Playlist::read(&base, "P").unwrap();
        let refs = shown
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, entry_key(e).to_owned()))
            .collect();
        (base, store, refs)
    }

    fn write_list(base: &Path, lines: &[&str]) {
        let body: String = lines.iter().map(|l| format!("../{l}.opus\n")).collect();
        fs::write(
            base.join(PLAYLISTS_DIR).join("P.m3u8"),
            format!("#EXTM3U\n{body}"),
        )
        .unwrap();
    }

    fn names(base: &Path) -> Vec<String> {
        Playlist::read(base, "P")
            .unwrap()
            .entries
            .iter()
            .map(|e| entry_key(e).trim_end_matches(".opus").to_owned())
            .collect()
    }

    #[test]
    fn removing_finds_an_entry_another_device_moved() {
        on_own_context(|| {
            let (base, store, refs) = playlist_lib("rm", &["A", "B", "C"]);
            // Another device moves C to the top; the user removes C as shown (position 2).
            write_list(&base, &["C", "A", "B"]);
            wait_for(|done| store.remove_from_playlist("P", &[refs[2].clone()], done)).unwrap();
            assert_eq!(names(&base), ["A", "B"]);

            // An entry that is gone: nothing changes, the user is told.
            write_list(&base, &["A", "B"]);
            let e = wait_for(|done| store.remove_from_playlist("P", &[(2, "C.opus".into())], done))
                .unwrap_err();
            assert!(e.to_string().contains("changed"), "{e}");
            assert_eq!(names(&base), ["A", "B"]);
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    #[test]
    fn moving_finds_entries_after_another_device_removed_one() {
        on_own_context(|| {
            let (base, store, refs) = playlist_lib("mv", &["A", "B", "C"]);
            // Another device removes A; the user drags C (shown at 2) onto A (shown at 0).
            write_list(&base, &["B", "C"]);
            let e =
                wait_for(|done| store.move_in_playlist("P", &refs[2], &refs[0], done)).unwrap_err();
            assert!(e.to_string().contains("changed"), "{e}");
            assert_eq!(names(&base), ["B", "C"]);
            // Dragging C onto B (shown at 1, now at 0) moves it there.
            wait_for(|done| store.move_in_playlist("P", &refs[2], &refs[1], done)).unwrap();
            assert_eq!(names(&base), ["C", "B"]);
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    #[test]
    fn too_large_playlists_are_listed_but_never_rewritten() {
        on_own_context(|| {
            let base = std::env::temp_dir().join(format!("tunemesh-big-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(base.join(PLAYLISTS_DIR)).unwrap();
            let big = base.join(PLAYLISTS_DIR).join("Big.m3u8");
            let text = format!(
                "#EXTM3U\n{}",
                "../missing.wav\n".repeat(tunemesh_core::limits::MAX_PLAYLIST_ENTRIES + 1)
            );
            fs::write(&big, &text).unwrap();
            fs::write(
                base.join(PLAYLISTS_DIR).join("Small.m3u8"),
                "#EXTM3U\n../a.opus\n",
            )
            .unwrap();
            let store = Store::new(base.clone());
            store.settle();

            assert!(store.is_too_large("Big"));
            assert!(
                store.playlists().iter().any(|p| p.name == "Big"),
                "still listed"
            );
            assert_eq!(store.playlist_names(), ["Small"], "not offered as a target");
            assert!(store.playlist("Big").is_none(), "never opened");
            assert!(
                wait_for(|done| store.add_to_playlist("Big", &["a.opus".into()], done)).is_err()
            );
            assert!(
                wait_for(|done| store.remove_from_playlist(
                    "Big",
                    &[(0, "missing.wav".into())],
                    done
                ))
                .is_err()
            );
            assert!(
                wait_for(|done| store.move_in_playlist(
                    "Big",
                    &(0, "missing.wav".into()),
                    &(1, "missing.wav".into()),
                    done
                ))
                .is_err()
            );
            assert!(wait_for(|done| store.rename_playlist("Big", "Other", done)).is_err());
            assert_eq!(
                fs::read_to_string(&big).unwrap(),
                text,
                "the file is untouched"
            );
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    #[test]
    fn playlist_reloads_read_only_changed_files() {
        use std::os::unix::fs::PermissionsExt;
        on_own_context(|| {
            let base = std::env::temp_dir().join(format!("tunemesh-reload-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            let dir = base.join(PLAYLISTS_DIR);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("Same.m3u8"), "#EXTM3U\n../a.opus\n").unwrap();
            fs::write(dir.join("Edit.m3u8"), "#EXTM3U\n../a.opus\n").unwrap();
            let store = Store::new(base.clone());
            store.settle();
            let count = |name: &str| {
                store
                    .playlists()
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.count)
            };
            assert_eq!((count("Same"), count("Edit")), (Some(1), Some(1)));

            // An unchanged file is not read again (it could not be now).
            fs::set_permissions(dir.join("Same.m3u8"), fs::Permissions::from_mode(0o000)).unwrap();
            fs::write(dir.join("Edit.m3u8"), "#EXTM3U\n../a.opus\n../b.opus\n").unwrap();
            store.reload_playlists();
            store.settle();
            assert_eq!((count("Same"), count("Edit")), (Some(1), Some(2)));

            fs::set_permissions(dir.join("Same.m3u8"), fs::Permissions::from_mode(0o644)).unwrap();
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    #[test]
    fn locating_many_moved_entries_is_linear() {
        let n = 100_000;
        let names: Vec<String> = (0..n).map(|i| format!("A/{i}.opus")).collect();
        // Shown in this order; the file now has every entry one place later.
        let mut p = Playlist::new("P");
        p.append(["A/new.opus"]);
        p.append(names.iter().map(String::as_str));
        let refs: Vec<EntryRef> = names.iter().cloned().enumerate().collect();
        let start = std::time::Instant::now();
        let found = locate(&p, &refs).unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert_eq!(found[0], 1);
        assert_eq!(found[n - 1], n);
    }

    /// R3-Opus-5: edits still queued on the writer thread when the app
    /// quits are written, and no temporary file is left in `Playlists/`.
    #[test]
    fn flushing_writes_queued_edits_before_quitting() {
        on_own_context(|| {
            let (base, store, _) = playlist_lib("flush", &["A"]);
            let liked: String = (0..200_000).map(|i| format!("../L/{i}.opus\n")).collect();
            fs::write(
                base.join(PLAYLISTS_DIR).join(format!("{LIKED_SONGS}.m3u8")),
                format!("#EXTM3U\n{liked}"),
            )
            .unwrap();
            store.reload_playlists();
            store.settle();
            assert_eq!(store.liked_count(), 200_000);

            store.add_to_playlist("P", &["B.opus".into()], |r| r.unwrap());
            store.toggle_like("New.opus", |r| r.unwrap());
            // As when quitting: the main loop does not run again.
            assert!(store.flush(FLUSH_WAIT));
            assert_eq!(names(&base), ["A", "B"]);
            let liked = Playlist::read(&base, LIKED_SONGS).unwrap();
            assert!(liked.entries.iter().any(|e| e.track() == Some("New.opus")));
            let temps: Vec<_> = fs::read_dir(base.join(PLAYLISTS_DIR))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".~tunemesh-"))
                .collect();
            assert!(temps.is_empty(), "left behind: {temps:?}");
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    /// R3-Opus-6: a reload that read the files before an edit was applied
    /// never replaces the edit's result; it is done again.
    #[test]
    fn a_reload_older_than_an_edit_is_redone() {
        on_own_context(|| {
            let (base, store, _) = playlist_lib("stale", &["A"]);
            // Only this test's reloads run.
            store.stop_watching();
            let (read, go) = store.start_paused_reload();
            read.recv().unwrap();
            // The reload holds P with one entry and no playlist "New".
            wait_for(|done| store.add_to_playlist("P", &["B.opus".into()], done)).unwrap();
            let new = wait_for(|done| store.create_playlist("New", done)).unwrap();
            go.send(()).unwrap();
            store.settle();

            let count = store
                .playlists()
                .iter()
                .find(|p| p.name == "P")
                .map(|p| p.count);
            assert_eq!(count, Some(2), "the edit is still shown");
            assert!(store.has_playlist(&new), "the new playlist is still shown");
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    /// A renamed playlist keeps no stamp under either name: a playlist that
    /// appears under the old name again is read, not taken for the
    /// renamed one.
    #[test]
    fn renaming_forgets_both_stamps() {
        on_own_context(|| {
            let (base, store, _) = playlist_lib("rename", &["A"]);
            assert!(store.playlist_stamps.borrow().contains_key("P"));
            let new = wait_for(|done| store.rename_playlist("P", "Q", done)).unwrap();
            let stamps = store.playlist_stamps.borrow();
            assert!(!stamps.contains_key("P") && !stamps.contains_key(&new));
            drop(stamps);
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    /// Runs the main loop until `done` holds, at most 10 s.
    fn until(done: impl Fn() -> bool) {
        let ctx = glib::MainContext::ref_thread_default();
        let start = std::time::Instant::now();
        while !done() && start.elapsed() < Duration::from_secs(10) {
            ctx.iteration(true);
        }
    }

    const COPY: &str = "Liked Songs.sync-conflict-20261026-120000-ABCDEFG.m3u8";

    /// Unliked here, another song liked on a device whose edit became a
    /// conflict copy: the copy naming this device is merged as soon as
    /// Syncthing tells the device ID, the unlike stays and the like is kept.
    #[test]
    fn a_conflict_copy_naming_this_device_is_merged_without_undoing_an_unlike() {
        on_own_context(|| {
            let (base, store, _) = playlist_lib("merge", &["A"]);
            let lists = base.join(PLAYLISTS_DIR);
            fs::write(
                lists.join(format!("{LIKED_SONGS}.m3u8")),
                "#EXTM3U\n../A.opus\n../B.opus\n",
            )
            .unwrap();
            store.reload_playlists();
            store.settle();
            wait_for(|done| store.toggle_like("A.opus", done)).unwrap();
            assert!(!store.is_liked("A.opus"));
            // The other device liked C in the version both started from.
            fs::write(
                lists.join(COPY),
                "#EXTM3U\n../A.opus\n../B.opus\n../C.opus\n",
            )
            .unwrap();

            store.set_device_id(
                "ABCDEFG-HIJKLMN-OPQRSTU-VWXYZ23-4567ABC-DEFGHIJ-KLMNOPQ-RSTUVWX".into(),
            );
            until(|| !lists.join(COPY).exists() && store.is_liked("C.opus"));
            store.settle();
            assert!(!lists.join(COPY).exists(), "the copy is gone once merged");
            let liked = Playlist::read(&base, LIKED_SONGS).unwrap();
            let mut tracks: Vec<&str> = liked.entries.iter().filter_map(|e| e.track()).collect();
            tracks.sort_unstable();
            assert_eq!(tracks, ["B.opus", "C.opus"], "the unlike stays");
            assert!(store.is_liked("C.opus") && !store.is_liked("A.opus"));
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }

    /// A fresh copy naming another device is left to that device (until it
    /// is `MERGE_AFTER` old), and nothing is merged before the ID is known.
    #[test]
    fn a_fresh_conflict_copy_of_another_device_is_left_alone() {
        on_own_context(|| {
            let (base, store, _) = playlist_lib("merge-other", &["A"]);
            let copy = base.join(PLAYLISTS_DIR).join(COPY);
            fs::write(&copy, "#EXTM3U\n../C.opus\n").unwrap();
            store.reload_playlists();
            store.settle();
            store.set_device_id(
                "ZZZZZZZ-HIJKLMN-OPQRSTU-VWXYZ23-4567ABC-DEFGHIJ-KLMNOPQ-RSTUVWX".into(),
            );
            assert!(store.flush(FLUSH_WAIT));
            until(|| store.merge_retry.borrow().is_some());
            assert!(copy.exists());
            assert!(!store.is_liked("C.opus"));
            assert!(store.merge_retry.borrow().is_some(), "tried again later");
            drop(store);
            fs::remove_dir_all(&base).unwrap();
        });
    }
}
